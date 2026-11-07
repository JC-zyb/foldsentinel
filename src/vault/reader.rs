use super::preview::PreviewBuilder;
use super::stream::{DecryptReader, HEADER_LEN, MAGIC};
use super::{Control, ErrorCode, MAX_CONTENT, MAX_ENTRIES, VaultReport, legacy};
use anyhow::{Context, Result, ensure};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek};
use std::path::Path;

/// 恢复、验证和预览共用的包读取入口；提交与资产登记由调用方负责。
pub(super) struct PackageReader {
    source: File,
    magic: [u8; 8],
}

impl PackageReader {
    pub(super) fn open(input: &Path) -> Result<Self> {
        let mut source = super::open_stable(input)?;
        let mut magic = [0; 8];
        source.read_exact(&mut magic)?;
        Ok(Self { source, magic })
    }

    pub(super) fn decode(
        &mut self,
        output: Option<&Path>,
        password: &str,
        control: &Control,
        mut preview: Option<&mut PreviewBuilder>,
    ) -> Result<(VaultReport, u8)> {
        if &self.magic == b"FSEAL001" {
            // 兼容解码沿用同一稳定句柄，从完整包头开始认证。
            self.source.rewind()?;
            let report =
                legacy::decode_from_file(&mut self.source, output, password, control, preview)?;
            return Ok((
                VaultReport {
                    input_bytes: report.input_bytes as u64,
                    output_bytes: report.output_bytes as u64,
                    directory: report.directory,
                },
                1,
            ));
        }
        ensure!(&self.magic == MAGIC, ErrorCode::UnsupportedFormat);
        let wire_size = self.source.metadata()?.len();
        let mut header = [0; HEADER_LEN];
        header[..8].copy_from_slice(&self.magic);
        self.source.read_exact(&mut header[8..])?;
        let size = u64::from_le_bytes(header[34..42].try_into()?);
        let entries = u32::from_le_bytes(header[42..46].try_into()?) as usize;
        ensure!(
            size <= MAX_CONTENT && entries <= MAX_ENTRIES && (header[9] != 0 || entries == 0),
            ErrorCode::InvalidPackage
        );
        control.total(wire_size);
        control.phase("deriving", "");
        let mut stream = DecryptReader::new(&mut self.source, password, header, control.clone())?;
        if let Some(output) = output {
            super::space::preflight(output, size.saturating_add(entries as u64 * 4096))?;
        }
        control.advance(HEADER_LEN as u64);
        control.phase(
            if output.is_some() {
                "decrypting"
            } else {
                "checking"
            },
            "",
        );
        let directory = header[9] == 1;
        if directory {
            super::process_directory(&mut stream, output, Some((size, entries)), control, preview)
                .context(ErrorCode::InvalidPackage)?;
        } else {
            // 多读一个字节，以验证声明长度之后的终块与物理 EOF。
            let mut content = stream.by_ref().take(size + 1);
            let copied = if let Some(output) = output {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)
                    .context(ErrorCode::WriteFailed)?;
                let copied = super::copy_to_file(&mut content, &mut file)?;
                ensure!(copied == size, ErrorCode::InvalidPackage);
                control.phase("syncing", "");
                file.sync_all().context(ErrorCode::WriteFailed)?;
                copied
            } else {
                std::io::copy(&mut content, &mut std::io::sink())?
            };
            ensure!(copied == size, ErrorCode::InvalidPackage);
            if let Some(preview) = preview.as_mut() {
                preview.record(None, false, size);
            }
        }
        Ok((
            VaultReport {
                input_bytes: wire_size,
                output_bytes: size,
                directory,
            },
            2,
        ))
    }
}
