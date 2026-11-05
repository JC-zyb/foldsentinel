use anyhow::Result;

#[cfg(windows)]
use anyhow::{Context, ensure};

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "FoldSentinel";

#[cfg(windows)]
pub fn autostart_enabled() -> Result<bool> {
    use std::{io, ptr};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_SZ, RegOpenKeyExW, RegQueryValueExW,
        },
    };

    let key_name = wide(RUN_KEY);
    let value_name = wide(RUN_VALUE);
    let mut key: HKEY = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_name.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    ensure!(
        status == ERROR_SUCCESS,
        "打开 Windows 自启动登记失败: {}",
        io::Error::from_raw_os_error(status as i32)
    );
    let key = RegistryKey(key);

    let mut value_type = 0;
    let mut byte_length = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            value_name.as_ptr(),
            ptr::null(),
            &mut value_type,
            ptr::null_mut(),
            &mut byte_length,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    ensure!(
        status == ERROR_SUCCESS,
        "读取 Windows 自启动登记失败: {}",
        io::Error::from_raw_os_error(status as i32)
    );
    ensure!(
        value_type == REG_SZ && (2..=64 * 1024).contains(&byte_length) && byte_length % 2 == 0,
        "Windows 自启动登记格式无效"
    );

    let mut value = vec![0u16; byte_length as usize / 2];
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            value_name.as_ptr(),
            ptr::null(),
            &mut value_type,
            value.as_mut_ptr().cast(),
            &mut byte_length,
        )
    };
    ensure!(
        status == ERROR_SUCCESS,
        "读取 Windows 自启动登记失败: {}",
        io::Error::from_raw_os_error(status as i32)
    );
    ensure!(
        value_type == REG_SZ && value.last() == Some(&0),
        "Windows 自启动登记未正确结束"
    );
    Ok(value.first().is_some_and(|character| *character != 0))
}

#[cfg(windows)]
pub fn set_autostart_enabled(enabled: bool) -> Result<()> {
    use std::{io, mem, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
            RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
        },
    };

    let key_name = wide(RUN_KEY);
    let value_name = wide(RUN_VALUE);
    if !enabled {
        let mut key: HKEY = ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                key_name.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        ensure!(
            status == ERROR_SUCCESS,
            "打开 Windows 自启动登记失败: {}",
            io::Error::from_raw_os_error(status as i32)
        );
        let key = RegistryKey(key);
        let status = unsafe { RegDeleteValueW(key.0, value_name.as_ptr()) };
        ensure!(
            status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND,
            "移除 Windows 自启动登记失败: {}",
            io::Error::from_raw_os_error(status as i32)
        );
        return Ok(());
    }

    let mut key: HKEY = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_name.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    ensure!(
        status == ERROR_SUCCESS,
        "打开 Windows 自启动登记失败: {}",
        io::Error::from_raw_os_error(status as i32)
    );
    let key = RegistryKey(key);
    let executable = std::env::current_exe().context("读取当前程序路径失败")?;
    let mut command = Vec::with_capacity(executable.as_os_str().encode_wide().count() + 3);
    command.push('"' as u16);
    command.extend(executable.as_os_str().encode_wide());
    command.extend(['"' as u16, 0]);
    let byte_length = u32::try_from(command.len() * mem::size_of::<u16>())
        .context("自启动命令长度超出 Windows 限制")?;
    let status = unsafe {
        RegSetValueExW(
            key.0,
            value_name.as_ptr(),
            0,
            REG_SZ,
            command.as_ptr().cast(),
            byte_length,
        )
    };
    ensure!(
        status == ERROR_SUCCESS,
        "写入 Windows 自启动登记失败: {}",
        io::Error::from_raw_os_error(status as i32)
    );
    Ok(())
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
struct RegistryKey(windows_sys::Win32::System::Registry::HKEY);

#[cfg(windows)]
impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::Registry::RegCloseKey(self.0) };
    }
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> Result<bool> {
    Err(anyhow::anyhow!("Windows 自启动设置仅在桌面程序中可用"))
}

#[cfg(not(windows))]
pub fn set_autostart_enabled(_enabled: bool) -> Result<()> {
    Err(anyhow::anyhow!("Windows 自启动设置仅在桌面程序中可用"))
}
