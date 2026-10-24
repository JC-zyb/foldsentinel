use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const PREFIX: &str = ".foldsentinel-job-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptedJob {
    pub id: String,
    pub input: PathBuf,
    pub output: PathBuf,
    pub staging: PathBuf,
    pub decrypt: bool,
    pub version: u8,
}

pub fn default_recovery_dir() -> Result<PathBuf> {
    Ok(crate::default_state_path()?
        .parent()
        .context("缺少用户数据目录")?
        .join("jobs"))
}

pub(crate) struct Job {
    record: InterruptedJob,
    registry: PathBuf,
    _lock: File,
}

impl Job {
    pub(crate) fn create(
        input: &Path,
        output: &Path,
        decrypt: bool,
        registry_dir: &Path,
    ) -> Result<Self> {
        fs::create_dir_all(registry_dir).context("创建中断恢复记录目录失败")?;
        let temporary = tempfile::Builder::new()
            .prefix(PREFIX)
            .tempdir_in(output.parent().context("缺少输出父目录")?)?;
        let staging = temporary.path().to_path_buf();
        let id = staging
            .file_name()
            .and_then(|s| s.to_str())
            .context("任务标识无效")?
            .to_string();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(staging.join("active.lock"))?;
        lock.try_lock()
            .map_err(|e| anyhow::anyhow!("锁定任务失败: {e}"))?;
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.join("owner"))?;
        marker.write_all(id.as_bytes())?;
        marker.sync_all()?;
        let record = InterruptedJob {
            id: id.clone(),
            input: input.to_path_buf(),
            output: output.to_path_buf(),
            staging,
            decrypt,
            version: 1,
        };
        let registry = registry_dir.join(format!("{id}.json"));
        let data = serde_json::to_vec(&record)?;
        ensure!(data.len() <= 16 * 1024, "路径过长，恢复记录超过 16 KiB");
        let mut file = tempfile::NamedTempFile::new_in(registry_dir)?;
        file.write_all(&data)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&registry)
            .map_err(|e| anyhow::anyhow!("登记任务失败: {}", e.error))?;
        // 登记落盘后才允许写 payload；故障或 panic 时保留目录供下次恢复。
        let kept = temporary.keep();
        debug_assert_eq!(kept, record.staging);
        Ok(Self {
            record,
            registry,
            _lock: lock,
        })
    }

    pub(crate) fn payload(&self) -> PathBuf {
        self.record.staging.join("payload")
    }

    pub(crate) fn conclude<T>(self, result: Result<T>) -> Result<T> {
        match (result, self.cleanup()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => {
                Err(error).context("输出已提交，但残留清理失败；请在中断任务列表中清理")
            }
            (Err(error), Err(cleanup)) => {
                Err(error).context(format!("清理失败，已保留中断记录: {cleanup:#}"))
            }
        }
    }

    fn cleanup(&self) -> Result<()> {
        remove_staging(&self.record)?;
        fs::remove_file(&self.registry).context("删除中断任务记录失败")?;
        Ok(())
    }
}

fn validate_id(id: &str) -> Result<()> {
    let suffix = id.strip_prefix(PREFIX).context("中断任务前缀无效")?;
    ensure!(
        id.starts_with(PREFIX)
            && id.len() > PREFIX.len()
            && id.len() < 100
            && suffix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "中断任务标识无效"
    );
    Ok(())
}

fn load_record(registry: &Path, id: &str) -> Result<InterruptedJob> {
    validate_id(id)?;
    let file = File::open(registry.join(format!("{id}.json")))?;
    let mut data = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= 16 * 1024, "恢复记录超长");
    let record: InterruptedJob =
        serde_json::from_slice(&data).context("中断任务记录损坏，未执行清理")?;
    ensure!(
        record.id == id
            && record.version == 1
            && record.input.is_absolute()
            && record.output.is_absolute()
            && record.staging.is_absolute()
            && record.output != record.staging,
        "恢复记录不合法"
    );
    ensure!(
        record.staging.file_name().and_then(|s| s.to_str()) == Some(id),
        "临时目录标识不匹配"
    );
    let output_parent = fs::canonicalize(record.output.parent().context("缺少输出父目录")?)?;
    let staging_parent = fs::canonicalize(record.staging.parent().context("缺少临时父目录")?)?;
    ensure!(
        output_parent == staging_parent,
        "临时目录不属于输出父目录，拒绝清理"
    );
    ensure!(
        !record
            .output
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case(id)),
        "输出与临时目录重叠，拒绝清理"
    );
    Ok(record)
}

enum JobLock {
    Missing,
    Active,
    Idle(File),
}

fn lock_interrupted(record: &InterruptedJob) -> Result<JobLock> {
    match fs::symlink_metadata(&record.staging) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(JobLock::Missing),
        Err(e) => return Err(e).context("检查中断临时目录失败"),
        Ok(_) => {}
    }
    super::reject_link(&record.staging)?;
    let marker =
        read_marker(&record.staging.join("owner")).context("临时目录缺少所有者标记，拒绝清理")?;
    ensure!(marker == record.id.as_bytes(), "临时目录所有者标记不匹配");
    let lock_path = record.staging.join("active.lock");
    super::reject_link(&lock_path)?;
    let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
    match lock.try_lock() {
        Ok(()) => Ok(JobLock::Idle(lock)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(JobLock::Active),
        Err(error) => bail!("无法验证任务锁，未执行清理: {error}"),
    }
}

fn remove_staging(record: &InterruptedJob) -> Result<()> {
    match fs::symlink_metadata(&record.staging) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("读取任务目录失败"),
        Ok(_) => {}
    }
    super::reject_link(&record.staging)?;
    // 最终 output 永远不参与删除；只清理本任务拥有的 staging。
    ensure!(
        read_marker(&record.staging.join("owner"))? == record.id.as_bytes(),
        "所有者标记无效，拒绝清理"
    );
    fs::remove_dir_all(&record.staging).context("删除任务临时目录失败")
}

fn read_marker(path: &Path) -> Result<Vec<u8>> {
    super::reject_link(path)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(101).read_to_end(&mut bytes)?;
    ensure!(bytes.len() < 100, "任务标记超长");
    Ok(bytes)
}

pub fn interrupted_jobs(registry: &Path) -> Result<Vec<InterruptedJob>> {
    match fs::read_dir(registry) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).context("读取中断任务列表失败"),
        Ok(entries) => {
            let mut records = Vec::new();
            for (index, entry) in entries.enumerate() {
                ensure!(index < 1000, "中断记录过多，请检查恢复目录");
                let path = entry?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("恢复记录名称无效")?;
                let record = load_record(registry, id)?;
                // 活跃任务不显示为中断；其他锁错误必须显式暴露。
                if matches!(lock_interrupted(&record)?, JobLock::Active) {
                    continue;
                }
                records.push(record);
            }
            Ok(records)
        }
    }
}

pub fn cleanup_interrupted(registry: &Path, id: &str) -> Result<InterruptedJob> {
    let record = load_record(registry, id)?;
    let _lock = match lock_interrupted(&record)? {
        JobLock::Active => bail!("任务仍在运行，不能清理"),
        JobLock::Idle(lock) => Some(lock),
        JobLock::Missing => None,
    };
    remove_staging(&record)?;
    fs::remove_file(registry.join(format!("{id}.json"))).context("清理中断记录失败")?;
    Ok(record)
}
