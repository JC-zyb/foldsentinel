//! FSEAL v1 的认证头部、固定 KDF 参数和边界见 docs/FORMAT.md。
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
#[cfg(test)]
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
#[cfg(test)]
use std::collections::HashSet;
#[cfg(test)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(test)]
use walkdir::WalkDir;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"FSEAL001";
const HEADER_LEN: usize = 38;
const TAG_LEN: usize = 16;
pub const MAX_PLAINTEXT: usize = 64 * 1024 * 1024;
#[cfg(test)]
const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VaultReport {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub directory: bool,
}

/// 源数据从不自动删除。目录归档包括相对名称和空目录。
#[cfg(test)]
pub fn encrypt_file(input: &Path, output: &Path, password: &str) -> Result<VaultReport> {
    validate_password(password)?;
    reject_link(input)?;
    let input = fs::canonicalize(input).context("解析输入路径失败")?;
    let output = output_path(output)?;
    ensure!(
        !PathBuf::from(output.to_string_lossy().to_lowercase())
            .starts_with(PathBuf::from(input.to_string_lossy().to_lowercase())),
        "加密包必须保存在源文件夹之外"
    );
    let directory = input.is_dir();
    let plaintext = if directory {
        pack_directory(&input)?
    } else {
        read_limited(&input, MAX_PLAINTEXT)?
    };
    let mut header = [0u8; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8] = 1;
    header[9] = u8::from(directory);
    OsRng
        .try_fill_bytes(&mut header[10..])
        .context("系统随机数生成失败")?;
    let key = derive_key(password, &header[10..26])?;
    let cipher =
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| anyhow::anyhow!("初始化加密失败"))?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&header[26..]),
            Payload {
                msg: &plaintext,
                aad: &header,
            },
        )
        .map_err(|_| anyhow::anyhow!("加密失败"))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(output.parent().context("输出位置无父目录")?)?;
    temporary.write_all(&header)?;
    temporary.write_all(&ciphertext)?;
    temporary.as_file().sync_all().context("刷新加密文件失败")?;
    temporary
        .persist_noclobber(&output)
        .map_err(|e| anyhow::anyhow!("提交输出失败（不会覆盖已有数据）: {}", e.error))?;
    Ok(VaultReport {
        input_bytes: plaintext.len(),
        output_bytes: HEADER_LEN + ciphertext.len(),
        directory,
    })
}

/// 认证通过前不落盘明文；目录只恢复到新的位置。
#[cfg(test)]
pub fn decrypt_file(input: &Path, output: &Path, password: &str) -> Result<VaultReport> {
    decode(input, Some(output), password, &super::Control::new(|_| {})?)
}

pub(super) fn decode(
    input: &Path,
    output: Option<&Path>,
    password: &str,
    control: &super::Control,
) -> Result<VaultReport> {
    decode_with_preview(input, output, password, control, None)
}

pub(super) fn decode_with_preview(
    input: &Path,
    output: Option<&Path>,
    password: &str,
    control: &super::Control,
    preview: Option<&mut super::preview::PreviewBuilder>,
) -> Result<VaultReport> {
    validate_password(password)?;
    let output = output.map(output_path).transpose()?;
    control.check()?;
    let mut file = super::open_stable(input)?;
    let length = file.metadata()?.len();
    ensure!(
        length <= (MAX_PLAINTEXT + HEADER_LEN + TAG_LEN) as u64,
        super::ErrorCode::InvalidPackage
    );
    control.total(length);
    control.phase("legacy", "");
    let mut payload = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new(vec![0; 128 * 1024]);
    loop {
        control.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        ensure!(
            payload.len() + count <= MAX_PLAINTEXT + HEADER_LEN + TAG_LEN,
            super::ErrorCode::InvalidPackage
        );
        payload.extend_from_slice(&buffer[..count]);
        control.advance(count as u64);
    }
    ensure!(
        payload.len() >= HEADER_LEN + TAG_LEN && &payload[..8] == MAGIC,
        super::ErrorCode::InvalidPackage
    );
    ensure!(
        payload[8] == 1 && payload[9] <= 1,
        super::ErrorCode::UnsupportedFormat
    );
    control.phase("deriving", "");
    let key = derive_key(password, &payload[10..26])?;
    let cipher =
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| anyhow::anyhow!("初始化解密失败"))?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&payload[26..HEADER_LEN]),
                Payload {
                    msg: &payload[HEADER_LEN..],
                    aad: &payload[..HEADER_LEN],
                },
            )
            .map_err(|_| anyhow::anyhow!(super::ErrorCode::Authentication))?,
    );
    let directory = payload[9] == 1;
    control.check()?;
    control.phase(
        if output.is_some() {
            "decrypting"
        } else {
            "checking"
        },
        "",
    );
    if let Some(output) = &output {
        super::space::preflight(output, plaintext.len() as u64 + 10_000 * 4096)?;
    }
    if directory {
        // 验证和恢复共用有界 raw tar 解析器。
        super::process_directory(
            &mut &plaintext[..],
            output.as_deref(),
            None,
            control,
            preview,
        )
        .context(super::ErrorCode::InvalidPackage)?;
    } else if let Some(output) = output {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .context(super::ErrorCode::WriteFailed)?;
        for bytes in plaintext.chunks(128 * 1024) {
            control.check()?;
            file.write_all(bytes)
                .context(super::ErrorCode::WriteFailed)?;
        }
        file.sync_all().context(super::ErrorCode::WriteFailed)?;
    } else if let Some(preview) = preview {
        preview.record(None, false, plaintext.len() as u64);
    }
    Ok(VaultReport {
        input_bytes: payload.len(),
        output_bytes: plaintext.len(),
        directory,
    })
}

pub(super) fn validate_password(password: &str) -> Result<()> {
    ensure!(!password.is_empty(), "密码不能为空");
    ensure!(password.len() <= 1024, "密码最多 1024 个 UTF-8 字节");
    Ok(())
}

pub(super) fn derive_key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    let params =
        Params::new(19_456, 2, 1, Some(32)).map_err(|e| anyhow::anyhow!("KDF 参数错误: {e}"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| anyhow::anyhow!("密钥派生失败: {e}"))?;
    Ok(key)
}

#[cfg(test)]
fn read_limited(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let file = File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "输入不是普通文件");
    ensure!(metadata.len() <= limit as u64, "超过首版 64 MiB 限制");
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "文件在读取期间增大，超过 64 MiB 限制");
    Ok(bytes)
}

pub(super) fn output_path(path: &Path) -> Result<PathBuf> {
    ensure!(path.file_name().is_some(), "请指定完整输出名称");
    match fs::symlink_metadata(path) {
        Ok(_) => bail!(super::ErrorCode::OutputExists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("检查输出位置失败"),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).context("输出父目录不存在或无法访问")?;
    ensure!(parent.is_dir(), "输出父路径不是目录");
    Ok(parent.join(path.file_name().context("缺少输出名称")?))
}

#[cfg(test)]
fn write_new_file(output: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(output.parent().context("缺少输出父目录")?)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(output)
        .map_err(|e| anyhow::anyhow!("提交恢复文件失败（不会覆盖已有数据）: {}", e.error))?;
    Ok(())
}

pub(crate) fn reject_link(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("读取输入元数据失败")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "首版不处理符号链接、目录联接或其他重解析点: {}",
            path.display()
        );
    }
    ensure!(!metadata.file_type().is_symlink(), "首版不处理符号链接");
    Ok(())
}

#[cfg(test)]
struct LimitedBuffer(Zeroizing<Vec<u8>>);
#[cfg(test)]
impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_PLAINTEXT {
            return Err(std::io::Error::other("文件夹归档超过首版 64 MiB 限制"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
fn pack_directory(root: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let mut archive = tar::Builder::new(LimitedBuffer(Zeroizing::new(Vec::new())));
    let mut seen = HashSet::new();
    archive.follow_symlinks(false);
    for (index, entry) in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        ensure!(index < MAX_ENTRIES, "文件夹最多包含 10000 项");
        let entry = entry.context("遍历文件夹失败，未创建加密包")?;
        reject_link(entry.path())?;
        let relative = entry.path().strip_prefix(root)?;
        validate_archive_path(relative)?;
        ensure!(
            seen.insert(relative.to_string_lossy().to_lowercase()),
            "文件夹包含 Windows 下重复的路径"
        );
        if entry.file_type().is_dir() {
            archive.append_dir(relative, entry.path())?;
        } else {
            ensure!(entry.file_type().is_file(), "文件夹包含非普通文件");
            archive.append_file(relative, &mut File::open(entry.path())?)?;
        }
    }
    archive.finish()?;
    Ok(archive.into_inner()?.0)
}

pub(super) fn validate_archive_path(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "归档路径为空");
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("加密包包含不安全的路径");
        };
        let name = name.to_str().context("首版不支持非 Unicode 文件名")?;
        ensure!(
            !name.ends_with(['.', ' '])
                && !name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
            "不支持的 Windows 文件名: {name}"
        );
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        ensure!(
            !reserved.contains(&stem.as_str()),
            "Windows 保留文件名: {name}"
        );
    }
    Ok(())
}

#[cfg(test)]
fn unpack_directory(bytes: &[u8], output: &Path) -> Result<()> {
    let staging = tempfile::tempdir_in(output.parent().context("缺少输出父目录")?)?;
    let destination = staging.path().join("payload");
    super::process_directory(
        &mut &bytes[..],
        Some(&destination),
        None,
        &super::Control::new(|_| {})?,
        None,
    )?;
    move_new_directory(&destination, output)?;
    Ok(())
}

#[cfg(all(test, windows))]
fn move_new_directory(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // NUL 结尾缓冲区在调用期间有效；flags=0 禁止替换已有目标。
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            0,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error()).context("提交恢复文件夹失败，目标可能已存在");
    }
    Ok(())
}
#[cfg(all(test, not(windows)))]
fn move_new_directory(_source: &Path, _destination: &Path) -> Result<()> {
    bail!("首版仅支持在 Windows 恢复文件夹")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_source_and_commit_race_do_not_overwrite_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("large");
        let output = dir.path().join("output");
        File::create(&input)
            .unwrap()
            .set_len(MAX_PLAINTEXT as u64 + 1)
            .unwrap();
        assert!(encrypt_file(&input, &output, "right").is_err());
        assert!(!output.exists());
        fs::write(&output, b"keep").unwrap();
        assert!(write_new_file(&output, b"replace").is_err());
        assert_eq!(fs::read(&output).unwrap(), b"keep");
    }
    #[test]
    #[cfg(windows)]
    fn archive_traversal_and_links_do_not_create_output() {
        for (name, kind) in [
            ("../outside", tar::EntryType::Regular),
            ("link", tar::EntryType::Symlink),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let output = dir.path().join("output");
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o600);
            header.set_entry_type(kind);
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            let mut archive = tar::Builder::new(Vec::new());
            archive.append(&header, &[][..]).unwrap();
            let bytes = archive.into_inner().unwrap();
            assert!(unpack_directory(&bytes, &output).is_err());
            assert!(!output.exists());
            assert!(!dir.path().join("outside").exists());
        }
    }
    #[test]
    fn binary_and_empty_files_round_trip_without_changing_source() {
        for bytes in [vec![], (0..=255).collect::<Vec<u8>>()] {
            let dir = tempfile::tempdir().unwrap();
            let input = dir.path().join("input");
            let sealed = dir.path().join("sealed");
            let restored = dir.path().join("restored");
            fs::write(&input, &bytes).unwrap();
            encrypt_file(&input, &sealed, "测试密码").unwrap();
            decrypt_file(&sealed, &restored, "测试密码").unwrap();
            assert_eq!(fs::read(input).unwrap(), bytes);
            assert_eq!(fs::read(restored).unwrap(), bytes);
        }
    }
    #[test]
    fn wrong_password_tampering_and_truncation_leave_no_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        let sealed = dir.path().join("sealed");
        let output = dir.path().join("output");
        fs::write(&input, b"private").unwrap();
        encrypt_file(&input, &sealed, "right").unwrap();
        assert!(decrypt_file(&sealed, &output, "wrong").is_err());
        let original = fs::read(&sealed).unwrap();
        for offset in [9, 10, 26, HEADER_LEN, original.len() - 1] {
            let mut changed = original.clone();
            changed[offset] ^= 1;
            fs::write(&sealed, changed).unwrap();
            assert!(decrypt_file(&sealed, &output, "right").is_err());
            assert!(!output.exists());
        }
        fs::write(&sealed, &original[..HEADER_LEN]).unwrap();
        assert!(decrypt_file(&sealed, &output, "right").is_err());
        assert!(!output.exists());
    }
    #[test]
    fn existing_outputs_and_empty_password_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        let output = dir.path().join("output");
        fs::write(&input, b"source").unwrap();
        fs::write(&output, b"keep").unwrap();
        assert!(encrypt_file(&input, &output, "password").is_err());
        assert_eq!(fs::read(&output).unwrap(), b"keep");
        assert!(encrypt_file(&input, &dir.path().join("new"), "").is_err());
        assert!(encrypt_file(&input, &input, "password").is_err());
    }
    #[test]
    fn archive_paths_reject_traversal_and_windows_devices() {
        for path in [
            "../escape",
            "C:\\escape",
            "CON.txt",
            "dir/NUL",
            "file:stream",
            "a.",
            "a ",
        ] {
            assert!(validate_archive_path(Path::new(path)).is_err(), "{path}");
        }
    }
    #[test]
    #[cfg(windows)]
    fn folder_round_trip_includes_unicode_and_empty_directories() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("源目录");
        fs::create_dir_all(source.join("空目录")).unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::write(source.join("nested/文档.txt"), b"folder contents").unwrap();
        let sealed = dir.path().join("folder.fse");
        let output = dir.path().join("恢复");
        assert!(encrypt_file(&source, &source.join("inside.fse"), "right").is_err());
        assert!(encrypt_file(&source, &sealed, "right").unwrap().directory);
        decrypt_file(&sealed, &output, "right").unwrap();
        assert!(output.join("空目录").is_dir());
        assert_eq!(
            fs::read(output.join("nested/文档.txt")).unwrap(),
            b"folder contents"
        );
        assert!(decrypt_file(&sealed, &output, "right").is_err());
    }
}
