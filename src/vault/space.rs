use super::ErrorCode;
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub(super) fn require_space(available: u64, required: u64) -> Result<()> {
    ensure!(available >= required, ErrorCode::InsufficientSpace);
    Ok(())
}

#[cfg(windows)]
pub(super) fn preflight(output: &Path, required: u64) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let parent = output.parent().context(ErrorCode::InvalidInput)?;
    let path: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // 缓冲区含 NUL，指针在调用期间有效；查询当前用户可用配额。
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error()).context("查询目标磁盘空间失败");
    }
    require_space(available, required)
}

#[cfg(not(windows))]
pub(super) fn preflight(_output: &Path, _required: u64) -> Result<()> {
    anyhow::bail!("空间预检仅支持 Windows")
}
