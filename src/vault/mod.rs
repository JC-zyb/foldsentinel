mod control;
mod error;
pub mod inventory;
mod legacy;
mod preview;
pub mod recovery;
mod space;
mod stream;
pub mod workflow;

use anyhow::{Context, Result, ensure};
pub use control::{Control, Progress};
pub use error::{ErrorCode, error_code};
pub(crate) use legacy::reject_link;
pub use preview::{MAX_PREVIEW_ENTRIES, PackagePreview, PreviewEntry, VaultInspection};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use stream::{DecryptReader, EncryptWriter, HEADER_LEN, MAGIC};
use walkdir::WalkDir;

const MAX_CONTENT: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_NAMES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct VaultReport {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub directory: bool,
}

pub fn encrypt_file(input: &Path, output: &Path, password: &str) -> Result<VaultReport> {
    encrypt_with_control(input, output, password, &Control::new(|_| {})?)
}
pub fn decrypt_file(input: &Path, output: &Path, password: &str) -> Result<VaultReport> {
    decrypt_with_control(input, output, password, &Control::new(|_| {})?)
}

/// 只读验证：完整认证内容及归档结构，不创建临时明文或输出文件；成功后登记资产位置。
pub fn verify_with_control(input: &Path, password: &str, control: &Control) -> Result<VaultReport> {
    let data_dir = control.recovery_dir.parent().context("缺少用户数据目录")?;
    let _guard = crate::lifecycle::OperationGuard::acquire(data_dir)?;
    let (report, _) = verify_content(input, password, control, None)?;
    inventory::observe(data_dir, &fs::canonicalize(input)?)?;
    Ok(report)
}

/// 完整认证成功后返回有界元数据；不创建输出或临时明文，但会登记资产位置。
pub fn inspect_with_control(
    input: &Path,
    password: &str,
    control: &Control,
) -> Result<VaultInspection> {
    let data_dir = control.recovery_dir.parent().context("缺少用户数据目录")?;
    let _guard = crate::lifecycle::OperationGuard::acquire(data_dir)?;
    let mut preview = preview::PreviewBuilder::default();
    let (report, version) = verify_content(input, password, control, Some(&mut preview))?;
    inventory::observe(data_dir, &fs::canonicalize(input)?)?;
    Ok(preview.finish(version, report))
}

fn verify_content(
    input: &Path,
    password: &str,
    control: &Control,
    mut preview: Option<&mut preview::PreviewBuilder>,
) -> Result<(VaultReport, u8)> {
    legacy::validate_password(password)?;
    control.check()?;
    reject_link(input)?;
    let input = fs::canonicalize(input)?;
    let mut source = open_stable(&input)?;
    let wire_size = source.metadata()?.len();
    let mut magic = [0; 8];
    source.read_exact(&mut magic)?;
    if &magic == b"FSEAL001" {
        let report = legacy::decode_with_preview(&input, None, password, control, preview)?;
        control.check()?;
        control.begin_commit()?;
        control.phase("completed", "");
        return Ok((
            VaultReport {
                input_bytes: report.input_bytes as u64,
                output_bytes: report.output_bytes as u64,
                directory: report.directory,
            },
            1,
        ));
    }
    ensure!(&magic == MAGIC, ErrorCode::UnsupportedFormat);
    let mut header = [0; HEADER_LEN];
    header[..8].copy_from_slice(&magic);
    source.read_exact(&mut header[8..])?;
    let size = u64::from_le_bytes(header[34..42].try_into()?);
    let entries = u32::from_le_bytes(header[42..46].try_into()?) as usize;
    ensure!(
        size <= MAX_CONTENT && entries <= MAX_ENTRIES && (header[9] != 0 || entries == 0),
        ErrorCode::InvalidPackage
    );
    control.total(wire_size);
    control.phase("deriving", "");
    let mut stream = DecryptReader::new(source, password, header, control.clone())?;
    control.advance(HEADER_LEN as u64);
    control.phase("checking", "");
    let directory = header[9] == 1;
    if directory {
        process_directory(&mut stream, None, Some((size, entries)), control, preview)
            .context(ErrorCode::InvalidPackage)?;
    } else {
        let copied = std::io::copy(&mut stream.by_ref().take(size + 1), &mut std::io::sink())?;
        ensure!(copied == size, ErrorCode::InvalidPackage);
        if let Some(preview) = preview.as_mut() {
            preview.record(None, false, size);
        }
    }
    control.check()?;
    control.begin_commit()?;
    control.phase("completed", "");
    Ok((
        VaultReport {
            input_bytes: wire_size,
            output_bytes: size,
            directory,
        },
        2,
    ))
}

#[derive(Debug, PartialEq, Eq)]
struct Item {
    relative: PathBuf,
    directory: bool,
    size: u64,
    modified: SystemTime,
}

fn scan(root: &Path, control: &Control) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    let mut names = 0;
    let mut total = 0u64;
    let mut seen = HashSet::new();
    for entry in WalkDir::new(root).min_depth(1).follow_links(false) {
        control.check()?;
        ensure!(items.len() < MAX_ENTRIES, "文件夹最多 100000 项");
        let entry = entry.context("扫描文件夹失败")?;
        reject_link(entry.path())?;
        ensure!(
            entry.file_type().is_file() || entry.file_type().is_dir(),
            "不支持特殊文件"
        );
        let relative = entry.path().strip_prefix(root)?.to_path_buf();
        legacy::validate_archive_path(&relative)?;
        let name = relative.to_str().context("非 Unicode 路径不受支持")?;
        names += name.len();
        ensure!(
            name.len() <= 4096 && names <= MAX_NAMES,
            "路径清单超出限制（单路径 4096 字节，总路径 16 MiB）"
        );
        ensure!(seen.insert(name.to_lowercase()), "文件夹包含重复路径");
        let metadata = entry.metadata()?;
        let directory = metadata.is_dir();
        let size = if directory { 0 } else { metadata.len() };
        total = total.checked_add(size).context("文件夹大小溢出")?;
        ensure!(total <= MAX_CONTENT, "内容超过 1 TiB 限制");
        control.current(name);
        items.push(Item {
            relative,
            directory,
            size,
            modified: metadata.modified()?,
        });
    }
    items.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(items)
}

fn open_stable(path: &Path) -> Result<File> {
    reject_link(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // 读取期间拒绝其他写入与删除句柄。
    }
    let file = options
        .open(path)
        .with_context(|| format!("打开源文件失败（可能正在被写入）: {}", path.display()))?;
    ensure!(file.metadata()?.is_file(), "输入不是普通文件");
    Ok(file)
}

fn copy_content(
    file: &mut File,
    target: &mut impl Write,
    expected: u64,
    modified: SystemTime,
    control: &Control,
) -> Result<()> {
    ensure!(
        file.metadata()?.len() == expected && file.metadata()?.modified()? == modified,
        ErrorCode::SourceChanged
    );
    let mut buffer = zeroize::Zeroizing::new(vec![0; 128 * 1024]);
    let mut copied = 0u64;
    loop {
        control.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        ensure!(copied <= expected, ErrorCode::SourceChanged);
        target
            .write_all(&buffer[..count])
            .context(ErrorCode::WriteFailed)?;
        control.advance(count as u64);
    }
    ensure!(
        copied == expected && file.metadata()?.modified()? == modified,
        ErrorCode::SourceChanged
    );
    Ok(())
}

struct TrackedReader<'a> {
    file: &'a mut File,
    control: &'a Control,
    read: u64,
}
impl Read for TrackedReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.control.check()?;
        let count = self.file.read(bytes)?;
        self.read += count as u64;
        self.control.advance(count as u64);
        Ok(count)
    }
}

pub fn encrypt_with_control(
    input: &Path,
    output: &Path,
    password: &str,
    control: &Control,
) -> Result<VaultReport> {
    let data_dir = control.recovery_dir.parent().context("缺少用户数据目录")?;
    let _guard = crate::lifecycle::OperationGuard::acquire(data_dir)?;
    legacy::validate_password(password)?;
    control.check()?;
    reject_link(input)?;
    let input = fs::canonicalize(input)?;
    let output = legacy::output_path(output)?;
    ensure!(
        !PathBuf::from(output.to_string_lossy().to_lowercase())
            .starts_with(PathBuf::from(input.to_string_lossy().to_lowercase())),
        "加密包必须在源文件夹之外"
    );
    let directory = input.is_dir();
    control.phase("scanning", "");
    let items = if directory {
        scan(&input, control)?
    } else {
        Vec::new()
    };
    let file_metadata = if directory {
        None
    } else {
        Some(fs::metadata(&input)?)
    };
    let total = if let Some(metadata) = &file_metadata {
        metadata.len()
    } else {
        items.iter().map(|i| i.size).sum()
    };
    ensure!(total <= MAX_CONTENT, "内容超过 1 TiB 限制");
    let archive_bound = if directory {
        items
            .iter()
            .map(|i| {
                i.size.div_ceil(512) * 512
                    + 1024
                    + (i.relative.as_os_str().len() as u64 * 4 + 1).div_ceil(512) * 512
            })
            .sum::<u64>()
            + 1024
    } else {
        total
    };
    space::preflight(
        &output,
        archive_bound + (archive_bound / stream::CHUNK as u64 + 1) * 20 + HEADER_LEN as u64,
    )?;
    control.total(total);
    let mut header = [0; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8] = 2;
    header[9] = u8::from(directory);
    OsRng
        .try_fill_bytes(&mut header[10..33])
        .context("系统随机数生成失败")?;
    header[33] = 20;
    header[34..42].copy_from_slice(&total.to_le_bytes());
    header[42..46].copy_from_slice(&(items.len() as u32).to_le_bytes());
    let job = recovery::Job::create(&input, &output, false, &control.recovery_dir)?;
    let result = (|| {
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(job.payload())
            .context(ErrorCode::WriteFailed)?;
        control.phase("deriving", "");
        let mut stream = EncryptWriter::new(&mut target, password, header, control.clone())?;
        control.phase("encrypting", "");
        if directory {
            let mut archive = tar::Builder::new(&mut stream);
            for item in &items {
                control.check()?;
                control.phase("encrypting", &item.relative.to_string_lossy());
                let mut header = tar::Header::new_gnu();
                header.set_size(item.size);
                header.set_mtime(0);
                header.set_mode(if item.directory { 0o700 } else { 0o600 });
                header.set_entry_type(if item.directory {
                    tar::EntryType::Directory
                } else {
                    tar::EntryType::Regular
                });
                header.set_cksum();
                if item.directory {
                    archive.append_data(&mut header, &item.relative, &[][..])?;
                } else {
                    let mut file = open_stable(&input.join(&item.relative))?;
                    let metadata = file.metadata()?;
                    ensure!(
                        metadata.len() == item.size && metadata.modified()? == item.modified,
                        ErrorCode::SourceChanged
                    );
                    let mut reader = TrackedReader {
                        file: &mut file,
                        control,
                        read: 0,
                    };
                    archive.append_data(&mut header, &item.relative, &mut reader)?;
                    ensure!(
                        reader.read == item.size
                            && file.metadata()?.len() == item.size
                            && file.metadata()?.modified()? == item.modified,
                        ErrorCode::SourceChanged
                    );
                }
            }
            archive.finish()?;
            drop(archive);
            control.phase("verifying", "");
            ensure!(scan(&input, control)? == items, ErrorCode::SourceChanged);
        } else {
            let metadata = file_metadata.as_ref().expect("file metadata");
            copy_content(
                &mut open_stable(&input)?,
                &mut stream,
                total,
                metadata.modified()?,
                control,
            )?;
        }
        let written = stream.finish()?;
        control.phase("syncing", "");
        target.sync_all().context(ErrorCode::WriteFailed)?;
        drop(target);
        let fingerprint = inventory::record_pending(data_dir, &job.payload(), &output)?;
        if let Err(error) = control
            .begin_commit()
            .and_then(|_| commit(&job.payload(), &output))
        {
            return match inventory::rollback_pending(data_dir, &fingerprint, &output) {
                Ok(()) => Err(error),
                Err(rollback) => Err(error).context(format!("资产登记保留待检查: {rollback:#}")),
            };
        }
        inventory::mark_encrypted(data_dir, &fingerprint, &output)?;
        Ok(VaultReport {
            input_bytes: total,
            output_bytes: written,
            directory,
        })
    })();
    let report = job.conclude(result)?;
    control.phase("completed", "");
    Ok(report)
}

pub fn decrypt_with_control(
    input: &Path,
    output: &Path,
    password: &str,
    control: &Control,
) -> Result<VaultReport> {
    let data_dir = control.recovery_dir.parent().context("缺少用户数据目录")?;
    let _guard = crate::lifecycle::OperationGuard::acquire(data_dir)?;
    legacy::validate_password(password)?;
    control.check()?;
    reject_link(input)?;
    let input = fs::canonicalize(input)?;
    let output = legacy::output_path(output)?;
    let fingerprint = inventory::observe(data_dir, &input)?;
    let mut source = open_stable(&input)?;
    let wire_size = source.metadata()?.len();
    let mut magic = [0; 8];
    source.read_exact(&mut magic)?;
    let job = recovery::Job::create(&input, &output, true, &control.recovery_dir)?;
    let result = (|| {
        if &magic == b"FSEAL001" {
            control.phase("legacy", "");
            let report = legacy::decode(&input, Some(&job.payload()), password, control)?;
            control.check()?;
            control.begin_commit()?;
            commit(&job.payload(), &output)?;
            inventory::mark_restored(data_dir, &fingerprint, &input, &output)?;
            return Ok(VaultReport {
                input_bytes: report.input_bytes as u64,
                output_bytes: report.output_bytes as u64,
                directory: report.directory,
            });
        }
        ensure!(&magic == MAGIC, ErrorCode::UnsupportedFormat);
        let mut header = [0; HEADER_LEN];
        header[..8].copy_from_slice(&magic);
        source.read_exact(&mut header[8..])?;
        let size = u64::from_le_bytes(header[34..42].try_into()?);
        let entries = u32::from_le_bytes(header[42..46].try_into()?) as usize;
        ensure!(
            size <= MAX_CONTENT && entries <= MAX_ENTRIES && (header[9] != 0 || entries == 0),
            ErrorCode::InvalidPackage
        );
        control.total(wire_size);
        control.phase("deriving", "");
        let mut stream = DecryptReader::new(source, password, header, control.clone())?;
        space::preflight(&output, size.saturating_add(entries as u64 * 4096))?;
        control.advance(HEADER_LEN as u64);
        control.phase("decrypting", "");
        let directory = header[9] == 1;
        if directory {
            process_directory(
                &mut stream,
                Some(&job.payload()),
                Some((size, entries)),
                control,
                None,
            )
            .context(ErrorCode::InvalidPackage)?;
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(job.payload())
                .context(ErrorCode::WriteFailed)?;
            let copied = copy_to_file(&mut stream.by_ref().take(size + 1), &mut file)?;
            ensure!(copied == size, ErrorCode::InvalidPackage);
            control.phase("syncing", "");
            file.sync_all().context(ErrorCode::WriteFailed)?;
        }
        control.begin_commit()?;
        commit(&job.payload(), &output)?;
        inventory::mark_restored(data_dir, &fingerprint, &input, &output)?;
        Ok(VaultReport {
            input_bytes: wire_size,
            output_bytes: size,
            directory,
        })
    })();
    let report = job.conclude(result)?;
    control.phase("completed", "");
    Ok(report)
}

// 分开读取与写入的错误上下文，避免磁盘写入失败被归为源读取失败。
fn copy_to_file(source: &mut impl Read, target: &mut File) -> Result<u64> {
    let mut buffer = zeroize::Zeroizing::new(vec![0; 128 * 1024]);
    let mut copied = 0;
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            return Ok(copied);
        }
        target
            .write_all(&buffer[..count])
            .context(ErrorCode::WriteFailed)?;
        copied += count as u64;
    }
}

fn process_directory(
    source: &mut impl Read,
    destination: Option<&Path>,
    expected: Option<(u64, usize)>,
    control: &Control,
    mut preview: Option<&mut preview::PreviewBuilder>,
) -> Result<()> {
    if let Some(destination) = destination {
        fs::create_dir(destination).context(ErrorCode::WriteFailed)?;
    }
    let (limit, entries) = expected.unwrap_or((legacy::MAX_PLAINTEXT as u64, 10_000));
    let mut archive = tar::Archive::new(&mut *source);
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut count = 0;
    let mut names = 0;
    let mut long_name = None;
    // raw 模式避免 tar 在返回条目前无界读取 GNU/PAX 元数据或解析稀疏表。
    for entry in archive.entries()?.raw(true) {
        control.check()?;
        ensure!(count < entries, "归档条目数超出认证声明");
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname() {
            ensure!(long_name.is_none(), "重复的扩展名称");
            ensure!(entry.size() > 0 && entry.size() <= 4097, "归档扩展名称超限");
            let mut bytes = Vec::new();
            entry.by_ref().take(4098).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 == entry.size() && bytes.pop() == Some(0),
                "扩展名称未正确结束"
            );
            long_name = Some(PathBuf::from(String::from_utf8(bytes)?));
            continue;
        }
        ensure!(
            kind.is_dir() || kind.is_file(),
            "归档包含链接、扩展或特殊文件"
        );
        count += 1;
        let relative = if let Some(name) = long_name.take() {
            name
        } else {
            entry.path()?.into_owned()
        };
        legacy::validate_archive_path(&relative)?;
        let name = relative.to_str().context("归档包含非 Unicode 路径")?;
        names += name.len();
        ensure!(name.len() <= 4096 && names <= MAX_NAMES, "归档路径超限");
        let key = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("/");
        ensure!(seen.insert(key.clone()), "归档存在重复路径");
        if kind.is_file() {
            files.push(key);
        }
        if preview.is_none() {
            control.current(name);
        }
        let path = destination.map(|destination| destination.join(&relative));
        if kind.is_dir() {
            ensure!(entry.size() == 0, "目录条目携带数据");
            if let Some(path) = path {
                fs::create_dir_all(path).context(ErrorCode::WriteFailed)?;
            }
        } else {
            ensure!(kind.is_file(), "归档包含链接或特殊文件");
            total = total.checked_add(entry.size()).context("归档大小溢出")?;
            ensure!(total <= limit, "归档文件总量超出认证声明");
            let copied = if let Some(path) = path {
                fs::create_dir_all(path.parent().context("缺少归档父目录")?)
                    .context(ErrorCode::WriteFailed)?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .context(ErrorCode::WriteFailed)?;
                let copied = copy_to_file(&mut entry, &mut file)?;
                file.sync_all().context(ErrorCode::WriteFailed)?;
                copied
            } else {
                std::io::copy(&mut entry, &mut std::io::sink())?
            };
            ensure!(copied == entry.size(), "归档文件截断");
        }
        if let Some(preview) = preview.as_mut() {
            preview.record(Some(name), kind.is_dir(), entry.size());
        }
    }
    for file in files {
        control.check()?;
        let prefix = format!("{file}/");
        ensure!(
            !seen
                .range(prefix.clone()..)
                .next()
                .is_some_and(|name| name.starts_with(&prefix)),
            "归档文件与目录冲突"
        );
    }
    ensure!(
        expected.is_none_or(|(size, entries)| count == entries && total == size)
            && long_name.is_none(),
        "归档条目或文件总量不符"
    );
    // tar 在零头部停止；必须继续消费并验证流的终块，不能把 tar EOF 当成认证 EOF。
    let mut tail = [0; 4096];
    let mut padding = 0;
    loop {
        control.check()?;
        let n = source.read(&mut tail)?;
        if n == 0 {
            break;
        }
        padding += n;
        ensure!(
            padding <= 1024 && tail[..n].iter().all(|b| *b == 0),
            "归档结尾存在多余数据"
        );
    }
    Ok(())
}

#[cfg(windows)]
fn commit(source: &Path, output: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let output: Vec<_> = output.as_os_str().encode_wide().chain(Some(0)).collect();
    // 路径缓冲区含 NUL，调用期间有效；flags=0 禁止替换现有文件或文件夹。
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(source.as_ptr(), output.as_ptr(), 0)
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .context("提交输出失败，未覆盖已有目标")
            .context(ErrorCode::WriteFailed);
    }
    Ok(())
}
#[cfg(not(windows))]
fn commit(_source: &Path, _output: &Path) -> Result<()> {
    anyhow::bail!("仅 Windows 支持输出提交")
}

#[cfg(test)]
mod tests;
