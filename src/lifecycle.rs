//! GUI、CLI 与卸载器共用的跨进程操作门；不是同账户攻击者隔离边界。
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const SESSION_FILE: &str = "uninstall-session.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub version: u32,
    pub parent_pid: u32,
    pub parent_created: u64,
    pub forced: bool,
}

pub fn default_data_dir() -> Result<PathBuf> {
    Ok(crate::default_state_path()?
        .parent()
        .context("缺少应用数据目录")?
        .to_path_buf())
}

fn lock_file(data_dir: &Path) -> Result<File> {
    fs::create_dir_all(data_dir).context("创建应用生命周期目录失败")?;
    crate::vault::reject_link(data_dir)?;
    let path = data_dir.join("lifecycle.lock");
    match fs::symlink_metadata(&path) {
        Ok(_) => crate::vault::reject_link(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("检查生命周期锁失败"),
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context("打开应用生命周期锁失败")
}

fn read_session(data_dir: &Path) -> Result<Option<Session>> {
    let path = data_dir.join(SESSION_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => crate::vault::reject_link(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("检查卸载会话位置失败"),
    }
    // 已存在的目录项若随后丢失或是断链，不得伪装成“没有卸载会话”。
    let file = File::open(&path).context("读取卸载会话失败")?;
    crate::vault::reject_link(&path)?;
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 8192, "卸载会话文件过大");
    let session: Session =
        serde_json::from_slice(&bytes).context("卸载会话损坏，请重新运行卸载器")?;
    ensure!(
        session.version == 1 && session.parent_pid != 0,
        "不支持的卸载会话"
    );
    Ok(Some(session))
}

fn remove_session(data_dir: &Path) -> Result<()> {
    match fs::remove_file(data_dir.join(SESSION_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("清理卸载会话失败"),
    }
}

#[cfg(windows)]
fn process_created(pid: u32) -> Result<Option<u64>> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                return Ok(None);
            }
            return Err(error).context("无法核验卸载进程身份");
        }
        let result = (|| {
            let mut exit = 0;
            ensure!(
                GetExitCodeProcess(handle, &mut exit) != 0,
                std::io::Error::last_os_error()
            );
            if exit != 259 {
                return Ok(None);
            }
            let mut created: FILETIME = std::mem::zeroed();
            let mut ended: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            ensure!(
                GetProcessTimes(handle, &mut created, &mut ended, &mut kernel, &mut user) != 0,
                std::io::Error::last_os_error()
            );
            Ok(Some(
                (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
            ))
        })();
        CloseHandle(handle);
        result
    }
}

#[cfg(not(windows))]
fn process_created(pid: u32) -> Result<Option<u64>> {
    if pid == std::process::id() {
        return Ok(Some(0));
    }
    anyhow::bail!("卸载进程身份核验只支持 Windows")
}

fn session_alive(session: &Session) -> Result<bool> {
    Ok(process_created(session.parent_pid)? == Some(session.parent_created))
}

pub struct OperationGuard {
    _lock: File,
}

impl OperationGuard {
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        let lock = lock_file(data_dir)?;
        lock.try_lock_shared()
            .map_err(|error| anyhow::anyhow!("卸载检查正在进行，请稍后重试: {error}"))?;
        if let Some(session) = read_session(data_dir)? {
            ensure!(
                !session_alive(&session)?,
                "程序正在卸载，不能启动新的文件操作"
            );
            // 崩溃或取消的卸载进程已经退出，残留标记不能永久锁住应用。
            remove_session(data_dir)?;
        }
        Ok(Self { _lock: lock })
    }
}

pub struct UninstallGuard {
    data_dir: PathBuf,
    _lock: File,
}

impl UninstallGuard {
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        let guard = Self::lock(data_dir)?;
        if let Some(session) = read_session(data_dir)? {
            ensure!(!session_alive(&session)?, "另一个卸载会话仍在运行");
            remove_session(data_dir)?;
        }
        Ok(guard)
    }

    fn lock(data_dir: &Path) -> Result<Self> {
        let lock = lock_file(data_dir)?;
        lock.try_lock().map_err(|error| {
            anyhow::anyhow!("还有文件操作或保护记录正在使用，请结束任务并关闭软件后重试: {error}")
        })?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            _lock: lock,
        })
    }

    pub fn arm(&self, parent_pid: u32, forced: bool) -> Result<Session> {
        let parent_created = process_created(parent_pid)?.context("卸载进程已经退出")?;
        let session = Session {
            version: 1,
            parent_pid,
            parent_created,
            forced,
        };
        let mut file = tempfile::NamedTempFile::new_in(&self.data_dir)?;
        file.write_all(&serde_json::to_vec(&session)?)?;
        file.as_file().sync_all()?;
        file.persist(self.data_dir.join(SESSION_FILE))
            .map_err(|error| error.error)?;
        Ok(session)
    }

    pub fn resume(data_dir: &Path, parent_pid: u32) -> Result<(Self, Session)> {
        let guard = Self::lock(data_dir)?;
        let session = read_session(data_dir)?.context("缺少卸载会话，拒绝清理应用数据")?;
        ensure!(
            session.parent_pid == parent_pid && session_alive(&session)?,
            "卸载会话或进程身份不匹配"
        );
        Ok((guard, session))
    }

    pub fn finish(&self) -> Result<()> {
        remove_session(&self.data_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_operations_and_exclusive_uninstall_do_not_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let first = OperationGuard::acquire(dir.path()).unwrap();
        let second = OperationGuard::acquire(dir.path()).unwrap();
        assert!(UninstallGuard::acquire(dir.path()).is_err());
        drop((first, second));
        let uninstall = UninstallGuard::acquire(dir.path()).unwrap();
        assert!(OperationGuard::acquire(dir.path()).is_err());
        drop(uninstall);
        assert!(OperationGuard::acquire(dir.path()).is_ok());
    }

    #[test]
    fn live_session_blocks_operations_after_preflight_returns() {
        let dir = tempfile::tempdir().unwrap();
        let guard = UninstallGuard::acquire(dir.path()).unwrap();
        guard.arm(std::process::id(), true).unwrap();
        drop(guard);
        assert!(OperationGuard::acquire(dir.path()).is_err());
        let (guard, session) = UninstallGuard::resume(dir.path(), std::process::id()).unwrap();
        assert!(session.forced);
        guard.finish().unwrap();
        drop(guard);
        assert!(OperationGuard::acquire(dir.path()).is_ok());
    }

    #[test]
    fn malformed_session_is_not_an_empty_safe_state() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SESSION_FILE), b"bad").unwrap();
        assert!(OperationGuard::acquire(dir.path()).is_err());
        assert!(UninstallGuard::acquire(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_session_link_is_not_a_missing_session() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join(SESSION_FILE))
            .unwrap();
        assert!(read_session(dir.path()).is_err());
        assert!(OperationGuard::acquire(dir.path()).is_err());
    }
}
