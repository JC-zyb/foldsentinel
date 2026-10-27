#[cfg(windows)]
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::Result;
#[cfg(windows)]
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};

#[cfg(windows)]
const ENTROPY: &[u8] = b"FoldSentinel recovery backup v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sealed {
    key: Vec<u8>,
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

#[cfg(windows)]
pub fn seal(input: &[u8]) -> Result<Sealed> {
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key.as_mut());
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).expect("fixed key length");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: input,
                aad: ENTROPY,
            },
        )
        .map_err(|_| anyhow::anyhow!("备份加密失败"))?;
    Ok(Sealed {
        key: crypt(key.as_ref(), true)?,
        nonce,
        ciphertext,
    })
}

#[cfg(windows)]
pub fn open(sealed: &Sealed) -> Result<Vec<u8>> {
    let key = zeroize::Zeroizing::new(crypt(&sealed.key, false)?);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("备份密钥无效"))?;
    cipher
        .decrypt(
            Nonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad: ENTROPY,
            },
        )
        .map_err(|_| anyhow::anyhow!("备份完整性验证失败"))
}

#[cfg(windows)]
fn crypt(input: &[u8], protect: bool) -> Result<Vec<u8>> {
    use anyhow::Context;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into().context("备份内容过大")?,
        pbData: input.as_ptr() as _,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as _,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // DPAPI 在本机当前账户下提供完整性验证与加密；不依赖环境变量绑定账户。
    let succeeded = unsafe {
        if protect {
            CryptProtectData(
                &source,
                null(),
                &entropy,
                null_mut(),
                null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                null_mut(),
                &entropy,
                null_mut(),
                null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let bytes = unsafe {
        let data = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let bytes = data.to_vec();
        if !protect {
            zeroize::Zeroize::zeroize(data);
        }
        LocalFree(output.pbData.cast());
        bytes
    };
    Ok(bytes)
}

#[cfg(windows)]
pub fn identity(path: &std::path::Path) -> Result<[u64; 3]> {
    use std::{
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::*;
    crate::vault::reject_link(path)?;
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        anyhow::bail!(crate::vault::ErrorCode::ProtectionConflict);
    }
    Ok([
        info.dwVolumeSerialNumber as u64,
        (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
        (info.ftCreationTime.dwHighDateTime as u64) << 32
            | info.ftCreationTime.dwLowDateTime as u64,
    ])
}

#[cfg(not(windows))]
pub fn seal(_: &[u8]) -> Result<Sealed> {
    anyhow::bail!(crate::vault::ErrorCode::ProtectionUnsupported)
}

#[cfg(not(windows))]
pub fn open(_: &Sealed) -> Result<Vec<u8>> {
    anyhow::bail!(crate::vault::ErrorCode::ProtectionUnsupported)
}

#[cfg(not(windows))]
pub fn identity(_: &std::path::Path) -> Result<[u64; 3]> {
    anyhow::bail!(crate::vault::ErrorCode::ProtectionUnsupported)
}
