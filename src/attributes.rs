use anyhow::{Context, Result};
use std::path::Path;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::iter::once;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

pub const HIDDEN: u32 = 0x0000_0002;
pub const SYSTEM: u32 = 0x0000_0004;
pub const READONLY: u32 = 0x0000_0001;

pub fn read(path: &Path) -> Result<u32> {
    #[cfg(windows)]
    {
        let wide = wide_path(path);
        let attributes =
            unsafe { windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(wide.as_ptr()) };
        if attributes == windows_sys::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("读取文件属性失败: {}", path.display()));
        }
        Ok(attributes)
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        anyhow::bail!("FoldSentinel 当前只支持 Windows 文件属性操作")
    }
}

pub fn write(path: &Path, attributes: u32) -> Result<()> {
    #[cfg(windows)]
    {
        let wide = wide_path(path);
        let success = unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(wide.as_ptr(), attributes)
        };
        if success == 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("写入文件属性失败: {}", path.display()));
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (path, attributes);
        anyhow::bail!("FoldSentinel 当前只支持 Windows 文件属性操作")
    }
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(once(0)).collect()
}
