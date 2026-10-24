mod control;
mod legacy;
pub mod recovery;
mod stream;

use anyhow::{Context, Result, ensure};
pub use control::{Control, Progress};
pub(crate) use legacy::reject_link;
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use std::collections::HashSet;
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
        "源文件在扫描后发生变化"
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
        ensure!(copied <= expected, "源文件在读取时增大");
        target.write_all(&buffer[..count])?;
        control.advance(count as u64);
    }
    ensure!(
        copied == expected && file.metadata()?.modified()? == modified,
        "源文件在读取时发生变化"
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
            .open(job.payload())?;
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
                        "源文件在扫描后发生变化"
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
                        "源文件在读取时发生变化"
                    );
                }
            }
            archive.finish()?;
            drop(archive);
            control.phase("verifying", "重新核对源目录");
            ensure!(scan(&input, control)? == items, "源目录在操作期间发生变化");
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
        target.sync_all()?;
        drop(target);
        control.begin_commit()?;
        commit(&job.payload(), &output)?;
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
    legacy::validate_password(password)?;
    control.check()?;
    reject_link(input)?;
    let input = fs::canonicalize(input)?;
    let output = legacy::output_path(output)?;
    let mut source = open_stable(&input)?;
    let wire_size = source.metadata()?.len();
    let mut magic = [0; 8];
    source.read_exact(&mut magic)?;
    let job = recovery::Job::create(&input, &output, true, &control.recovery_dir)?;
    let result = (|| {
        if &magic == b"FSEAL001" {
            control.phase("legacy", "正在验证 v0.1 加密包");
            let report = legacy::decrypt_file(&input, &job.payload(), password)?;
            control.check()?;
            control.begin_commit()?;
            commit(&job.payload(), &output)?;
            return Ok(VaultReport {
                input_bytes: report.input_bytes as u64,
                output_bytes: report.output_bytes as u64,
                directory: report.directory,
            });
        }
        ensure!(&magic == MAGIC, "不是支持的 FoldSentinel 加密包");
        let mut header = [0; HEADER_LEN];
        header[..8].copy_from_slice(&magic);
        source.read_exact(&mut header[8..])?;
        let size = u64::from_le_bytes(header[34..42].try_into()?);
        let entries = u32::from_le_bytes(header[42..46].try_into()?) as usize;
        ensure!(
            size <= MAX_CONTENT && entries <= MAX_ENTRIES && (header[9] != 0 || entries == 0),
            "加密包声明的大小或条目数超限"
        );
        control.total(wire_size);
        control.phase("deriving", "");
        let mut stream = DecryptReader::new(source, password, header, control.clone())?;
        control.advance(HEADER_LEN as u64);
        control.phase("decrypting", "");
        let directory = header[9] == 1;
        if directory {
            restore_directory(&mut stream, &job.payload(), size, entries, control)?;
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(job.payload())?;
            let copied = std::io::copy(&mut stream.by_ref().take(size + 1), &mut file)?;
            ensure!(copied == size, "恢复文件大小与认证头部不符");
            control.phase("syncing", "");
            file.sync_all()?;
        }
        control.begin_commit()?;
        commit(&job.payload(), &output)?;
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

fn restore_directory(
    source: &mut impl Read,
    destination: &Path,
    expected: u64,
    entries: usize,
    control: &Control,
) -> Result<()> {
    fs::create_dir(destination)?;
    let mut archive = tar::Archive::new(&mut *source);
    let mut seen = HashSet::new();
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
        ensure!(seen.insert(name.to_lowercase()), "归档存在重复路径");
        control.phase("decrypting", name);
        let path = destination.join(&relative);
        if kind.is_dir() {
            ensure!(entry.size() == 0, "目录条目携带数据");
            fs::create_dir_all(path)?;
        } else {
            ensure!(kind.is_file(), "归档包含链接或特殊文件");
            total = total.checked_add(entry.size()).context("归档大小溢出")?;
            ensure!(total <= expected, "归档文件总量超出认证声明");
            fs::create_dir_all(path.parent().context("缺少归档父目录")?)?;
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            let copied = std::io::copy(&mut entry, &mut file)?;
            ensure!(copied == entry.size(), "归档文件截断");
            file.sync_all()?;
        }
    }
    ensure!(
        count == entries && total == expected && long_name.is_none(),
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
        return Err(std::io::Error::last_os_error()).context("提交输出失败，未覆盖已有目标");
    }
    Ok(())
}
#[cfg(not(windows))]
fn commit(_source: &Path, _output: &Path) -> Result<()> {
    anyhow::bail!("仅 Windows 支持输出提交")
}

#[cfg(test)]
mod tests;
