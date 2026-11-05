//! 安装器与服务页面共用主程序内置入口；仅服务安装阶段请求管理员权限。

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const EXIT_FAILED: i32 = 20;
const EXIT_CANCELLED: i32 = 1223;
static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub struct InstallReport {
    status: InstallStatus,
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum InstallStatus {
    Installed,
    Cancelled,
}

pub struct InstallGuard(&'static AtomicBool);

impl InstallGuard {
    fn acquire(installing: &'static AtomicBool) -> Result<Self> {
        installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| foldsentinel::vault::ErrorCode::Busy)?;
        Ok(Self(installing))
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub fn acquire_install() -> Result<InstallGuard> {
    InstallGuard::acquire(&INSTALLING)
}

fn install_report(code: i32) -> Result<InstallReport> {
    let status = match code {
        0 => InstallStatus::Installed,
        EXIT_CANCELLED => InstallStatus::Cancelled,
        _ => bail!("持续防护服务安装未完成（退出码：{code}）"),
    };
    Ok(InstallReport { status })
}

pub fn install_from_ui(_guard: InstallGuard) -> Result<InstallReport> {
    // Guard 由阻塞任务持有；页面关闭或 IPC future 被取消都不能提前允许再次提权。
    let code = run(Mode::Install).inspect_err(report_failure)?;
    install_report(code).context(foldsentinel::vault::ErrorCode::ServiceUnavailable)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Install,
    Elevated,
}

fn parse(args: &[OsString]) -> Result<Option<Mode>> {
    if !args
        .iter()
        .any(|arg| arg == "--install-service" || arg == "--install-service-elevated")
    {
        return Ok(None);
    }
    if args.len() != 1 {
        bail!("服务安装入口不接受附加参数");
    }
    Ok(Some(if args[0] == "--install-service" {
        Mode::Install
    } else {
        Mode::Elevated
    }))
}

fn payload_path(executable: &Path) -> Result<PathBuf> {
    Ok(executable
        .parent()
        .context("无法定位应用所在目录")?
        .join("foldsentinel-service.exe"))
}

pub fn maybe_run() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match parse(&args) {
        Ok(None) => None,
        Ok(Some(mode)) => Some(match run(mode) {
            Ok(code) => code,
            Err(error) => {
                report_failure(&error);
                EXIT_FAILED
            }
        }),
        Err(error) => {
            report_failure(&error);
            Some(EXIT_FAILED)
        }
    }
}

fn install() -> Result<()> {
    let executable = std::env::current_exe().context("无法定位主程序")?;
    let payload = payload_path(&executable)?;
    // 编译时绑定同批构建服务的散列，不接受命令行传入来源或散列。
    foldsentinel::service::install_from(&payload, env!("FOLDSENTINEL_SERVICE_SHA256"))
}

#[cfg(windows)]
fn run(mode: Mode) -> Result<i32> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    fn checked(value: bool) -> Result<()> {
        if !value {
            return Err(std::io::Error::last_os_error()).context("Windows 安装调用失败");
        }
        Ok(())
    }

    let mut token: HANDLE = null_mut();
    checked(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0 })?;
    let token = Handle(token);
    let mut elevation: TOKEN_ELEVATION = unsafe { std::mem::zeroed() };
    let mut size = 0;
    checked(unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        ) != 0
    })?;
    if mode == Mode::Elevated || elevation.TokenIsElevated != 0 {
        install()?;
        return Ok(0);
    }

    let executable = std::env::current_exe().context("无法定位提权入口")?;
    let file: Vec<_> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let verb: Vec<_> = "runas".encode_utf16().chain(Some(0)).collect();
    let parameters: Vec<_> = "--install-service-elevated"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 0;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            return Ok(ERROR_CANCELLED as i32);
        }
        return Err(error).context("无法请求管理员安装服务");
    }
    if info.hProcess.is_null() {
        bail!("管理员入口未返回可验证的进程句柄");
    }
    let process = Handle(info.hProcess);
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(std::io::Error::last_os_error()).context("等待服务安装结果失败");
    }
    let mut code = 1;
    checked(unsafe { GetExitCodeProcess(process.0, &mut code) != 0 })?;
    Ok(code as i32)
}

#[cfg(not(windows))]
fn run(_mode: Mode) -> Result<i32> {
    bail!("SYSTEM 服务仅适用于 Windows")
}

fn report_failure(error: &anyhow::Error) {
    eprintln!("服务安装未完成：{error:#}");
    #[cfg(windows)]
    {
        use std::ptr::null_mut;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let title: Vec<_> = "FoldSentinel — 服务安装 / Service setup"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let details: String = format!("{error:#}").chars().take(4096).collect();
        let message = format!(
            "持续防护服务安装未完成。请保留恢复资料，核对服务配置后重试。主程序的本地功能仍可使用。\n\nProtection service setup did not complete. Keep recovery data and check the service configuration before retrying. Local app features remain available.\n\n{details}"
        );
        let message: Vec<_> = message.encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_entry_accepts_only_its_fixed_mode() {
        for (flag, mode) in [
            ("--install-service", Mode::Install),
            ("--install-service-elevated", Mode::Elevated),
        ] {
            assert_eq!(parse(&[flag.into()]).unwrap(), Some(mode));
            assert!(parse(&[flag.into(), "--source=anything.exe".into()]).is_err());
            assert!(parse(&[flag.into(), "--uninstall-service-elevated".into()]).is_err());
        }
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&["--uninstall-prepare".into()]).unwrap(), None);
    }

    #[test]
    fn payload_location_is_bound_to_the_application_directory() {
        let directory = PathBuf::from("application");
        assert_eq!(
            payload_path(&directory.join("FoldSentinel.exe")).unwrap(),
            directory.join("foldsentinel-service.exe")
        );
    }

    #[test]
    fn ui_install_reports_only_verified_success_or_explicit_cancel() {
        assert_eq!(
            serde_json::to_value(install_report(0).unwrap()).unwrap(),
            serde_json::json!({ "status": "installed" })
        );
        assert_eq!(
            serde_json::to_value(install_report(EXIT_CANCELLED).unwrap()).unwrap(),
            serde_json::json!({ "status": "cancelled" })
        );
        for code in [EXIT_FAILED, 21, 1, -1] {
            let error = install_report(code).unwrap_err();
            assert!(format!("{error:#}").contains(&code.to_string()));
        }
    }

    #[test]
    fn ui_install_guard_remains_busy_until_detached_worker_finishes() -> Result<()> {
        use std::sync::mpsc;

        static INSTALLING: AtomicBool = AtomicBool::new(false);
        let guard = InstallGuard::acquire(&INSTALLING)?;
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            drop(guard);
            finished_tx.send(()).unwrap();
        });
        started_rx.recv()?;
        // 模拟调用方不再等待，实际阻塞任务仍在运行。
        drop(worker);
        let error = InstallGuard::acquire(&INSTALLING).err().unwrap();
        assert_eq!(
            foldsentinel::vault::error_code(&error),
            foldsentinel::vault::ErrorCode::Busy
        );
        finish_tx.send(())?;
        finished_rx.recv()?;
        let _next = InstallGuard::acquire(&INSTALLING)?;
        Ok(())
    }
}
