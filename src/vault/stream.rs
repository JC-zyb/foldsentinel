use super::{Control, ErrorCode, legacy};
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{
    KeyInit, Payload,
    stream::{DecryptorBE32, EncryptorBE32},
};
use anyhow::Context;
use anyhow::{Result, ensure};
use std::io::{self, Read, Write};
use zeroize::{Zeroize, Zeroizing};

pub(crate) const CHUNK: usize = 1024 * 1024;
pub(crate) const HEADER_LEN: usize = 46;
pub(crate) const MAGIC: &[u8; 8] = b"FSEAL002";
const LAST: u32 = 1 << 31;

fn cipher(password: &str, header: &[u8; HEADER_LEN]) -> Result<Aes256Gcm> {
    let key = legacy::derive_key(password, &header[10..26])?;
    Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| anyhow::anyhow!("初始化加密流失败"))
}

pub(crate) struct EncryptWriter<W> {
    target: W,
    encryptor: Option<EncryptorBE32<Aes256Gcm>>,
    buffer: Zeroizing<Vec<u8>>,
    header: [u8; HEADER_LEN],
    control: Control,
    pub(crate) written: u64,
}

impl<W: Write> EncryptWriter<W> {
    pub(crate) fn new(
        mut target: W,
        password: &str,
        header: [u8; HEADER_LEN],
        control: Control,
    ) -> Result<Self> {
        let encryptor = EncryptorBE32::from_aead(cipher(password, &header)?, header[26..33].into());
        control.check()?;
        target.write_all(&header).context(ErrorCode::WriteFailed)?;
        Ok(Self {
            target,
            encryptor: Some(encryptor),
            buffer: Zeroizing::new(Vec::with_capacity(CHUNK)),
            header,
            control,
            written: HEADER_LEN as u64,
        })
    }

    fn frame(&mut self, bytes: &[u8], last: bool) -> io::Result<()> {
        let length = bytes.len() as u32 | if last { LAST } else { 0 };
        self.target.write_all(&length.to_le_bytes())?;
        self.target.write_all(bytes)?;
        self.written += 4 + bytes.len() as u64;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<u64> {
        self.control.check()?;
        let ciphertext = self
            .encryptor
            .take()
            .expect("stream consumed once")
            .encrypt_last(Payload {
                msg: &self.buffer,
                aad: &self.header,
            })
            .map_err(|_| anyhow::anyhow!("加密终块失败"))?;
        self.frame(&ciphertext, true)
            .context(ErrorCode::WriteFailed)?;
        self.target.flush().context(ErrorCode::WriteFailed)?;
        Ok(self.written)
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.control.check()?;
        let count = bytes.len().min(CHUNK - self.buffer.len());
        self.buffer.extend_from_slice(&bytes[..count]);
        if self.buffer.len() == CHUNK {
            let ciphertext = self
                .encryptor
                .as_mut()
                .expect("unfinished stream")
                .encrypt_next(Payload {
                    msg: &self.buffer,
                    aad: &self.header,
                })
                .map_err(|_| io::Error::other("加密分块失败或计数器已耗尽"))?;
            self.frame(&ciphertext, false)?;
            self.buffer.zeroize();
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.target.flush()
    }
}

pub(crate) struct DecryptReader<R> {
    source: R,
    decryptor: Option<DecryptorBE32<Aes256Gcm>>,
    buffer: Zeroizing<Vec<u8>>,
    offset: usize,
    done: bool,
    header: [u8; HEADER_LEN],
    control: Control,
}

impl<R: Read> DecryptReader<R> {
    pub(crate) fn new(
        source: R,
        password: &str,
        header: [u8; HEADER_LEN],
        control: Control,
    ) -> Result<Self> {
        ensure!(
            &header[..8] == MAGIC && header[8] == 2 && header[9] <= 1 && header[33] == 20,
            ErrorCode::UnsupportedFormat
        );
        let decryptor = DecryptorBE32::from_aead(cipher(password, &header)?, header[26..33].into());
        Ok(Self {
            source,
            decryptor: Some(decryptor),
            buffer: Zeroizing::new(Vec::new()),
            offset: 0,
            done: false,
            header,
            control,
        })
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        self.control.check()?;
        if self.offset == self.buffer.len() {
            if self.done {
                return Ok(0);
            }
            let mut frame = [0; 4];
            self.source.read_exact(&mut frame).map_err(|e| {
                io::Error::new(e.kind(), format!("加密包缺少完整分块或认证终块: {e}"))
            })?;
            let frame = u32::from_le_bytes(frame);
            let last = frame & LAST != 0;
            let length = (frame & !LAST) as usize;
            if !(16..=CHUNK + 16).contains(&length) || (!last && length != CHUNK + 16) {
                return Err(io::Error::other(ErrorCode::InvalidPackage));
            }
            let mut ciphertext = vec![0; length];
            self.source.read_exact(&mut ciphertext)?;
            let payload = Payload {
                msg: &ciphertext,
                aad: &self.header,
            };
            let plaintext = if last {
                self.decryptor
                    .take()
                    .expect("one final block")
                    .decrypt_last(payload)
            } else {
                self.decryptor
                    .as_mut()
                    .expect("unfinished stream")
                    .decrypt_next(payload)
            }
            .map_err(|_| io::Error::other(ErrorCode::Authentication))?;
            self.buffer = Zeroizing::new(plaintext);
            self.offset = 0;
            if last {
                let mut extra = [0];
                if self.source.read(&mut extra)? != 0 {
                    return Err(io::Error::other(ErrorCode::InvalidPackage));
                }
                self.done = true;
            }
            self.control.advance(4 + length as u64);
        }
        let count = output.len().min(self.buffer.len() - self.offset);
        output[..count].copy_from_slice(&self.buffer[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}
