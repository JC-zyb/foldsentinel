//! 桌面偏好、托盘和固定内容通知；不保存任务路径或密码。
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{
    AppHandle, Emitter, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

const TRAY_ID: &str = "foldsentinel-desktop";
const PREFERENCES_FILE: &str = "window-preferences.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowPreferences {
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub language: String,
}

impl Default for WindowPreferences {
    fn default() -> Self {
        Self {
            close_to_tray: false,
            start_minimized: false,
            language: "zh-CN".into(),
        }
    }
}

impl WindowPreferences {
    fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.language.as_str(), "zh-CN" | "en"),
            "不支持的桌面菜单语言"
        );
        Ok(())
    }
}

pub struct DesktopState {
    preferences: Mutex<Result<WindowPreferences, String>>,
    busy: AtomicBool,
    operations: Arc<AtomicUsize>,
    action_error: Mutex<Option<DesktopActionError>>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopActionError {
    Hide,
    Show,
    StartupHide,
}

pub fn take_action_error(app: &AppHandle) -> Result<Option<DesktopActionError>> {
    Ok(app
        .state::<DesktopState>()
        .action_error
        .lock()
        .map_err(|_| anyhow::anyhow!("桌面错误状态锁异常"))?
        .take())
}

fn report_action_error(app: &AppHandle, action: DesktopActionError, error: &anyhow::Error) {
    eprintln!("桌面动作 {action:?} 失败: {error:#}");
    match app.state::<DesktopState>().action_error.lock() {
        Ok(mut pending) => *pending = Some(action),
        Err(error) => eprintln!("保存桌面错误状态失败: {error}"),
    }
    // 启动时 UI 可能尚未监听；take_desktop_action_error 在 UI 初始化后补读。
    if let Err(error) = app.emit("desktop-action-error", action) {
        eprintln!("发送桌面动作错误失败: {error}");
    }
}

pub struct OperationGuard(Arc<AtomicUsize>);
impl OperationGuard {
    fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn operation(app: &AppHandle) -> OperationGuard {
    OperationGuard::new(app.state::<DesktopState>().operations.clone())
}

#[derive(Serialize)]
pub struct DataLocations {
    pub app_data: PathBuf,
    pub recovery: PathBuf,
    pub protection: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataLocation {
    AppData,
    Recovery,
    Protection,
}

pub fn data_locations(app: &AppHandle) -> Result<DataLocations> {
    Ok(DataLocations {
        app_data: app.path().app_local_data_dir()?,
        recovery: foldsentinel::vault::recovery::default_recovery_dir()?,
        protection: foldsentinel::lifecycle::default_data_dir()?,
    })
}

pub fn open_data_location(app: &AppHandle, kind: DataLocation) -> Result<()> {
    let locations = data_locations(app)?;
    let path = match kind {
        DataLocation::AppData => locations.app_data,
        DataLocation::Recovery => locations.recovery,
        DataLocation::Protection => locations.protection,
    };
    ensure!(fs::metadata(&path)?.is_dir(), "数据位置不是文件夹");
    crate::support::open_output_folder(&path)
}

fn preferences_path() -> Result<PathBuf> {
    Ok(foldsentinel::lifecycle::default_data_dir()?.join(PREFERENCES_FILE))
}

fn read_preferences(path: &Path) -> Result<WindowPreferences> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WindowPreferences::default());
        }
        Err(error) => return Err(error).context("读取窗口偏好失败"),
    };
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 4096, "窗口偏好文件过大");
    let value: WindowPreferences = serde_json::from_slice(&bytes).context("窗口偏好格式无效")?;
    value.validate()?;
    Ok(value)
}

fn write_preferences(path: &Path, preferences: &WindowPreferences) -> Result<()> {
    preferences.validate()?;
    let parent = path.parent().context("窗口偏好没有父目录")?;
    fs::create_dir_all(parent)?;
    let token = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = parent.join(format!(
        ".window-preferences-{}-{token}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(preferences)?)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err()
        && let Err(error) = fs::remove_file(&temporary)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("清理窗口偏好临时文件失败: {error}");
    }
    result
}

pub fn validate_preferences_file(path: &Path) -> Result<()> {
    read_preferences(path).map(|_| ())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    ensure!(
        unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0,
        "保存窗口偏好失败: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination).context("保存窗口偏好失败")
}

pub fn get_preferences(app: &AppHandle) -> Result<WindowPreferences> {
    app.state::<DesktopState>()
        .preferences
        .lock()
        .map_err(|_| anyhow::anyhow!("窗口偏好锁异常"))?
        .clone()
        .map_err(anyhow::Error::msg)
}

pub fn set_preferences(
    app: &AppHandle,
    preferences: WindowPreferences,
) -> Result<WindowPreferences> {
    preferences.validate()?;
    let menu = tray_menu(app, &preferences.language)?;
    let tray = app.tray_by_id(TRAY_ID).context("托盘入口不可用")?;
    tray.set_menu(Some(menu))?;
    let state = app.state::<DesktopState>();
    let mut current = state
        .preferences
        .lock()
        .map_err(|_| anyhow::anyhow!("窗口偏好锁异常"))?;
    if let Err(error) = write_preferences(&preferences_path()?, &preferences) {
        let previous = current.as_ref().ok().cloned();
        drop(current);
        if let Some(previous) = previous {
            match tray_menu(app, &previous.language)
                .and_then(|menu| tray.set_menu(Some(menu)).map_err(Into::into))
            {
                Ok(()) => {}
                Err(restore) => eprintln!("恢复托盘语言失败: {restore:#}"),
            }
        }
        return Err(error);
    }
    *current = Ok(preferences.clone());
    Ok(preferences)
}

fn tray_menu(app: &AppHandle, language: &str) -> Result<Menu<tauri::Wry>> {
    let zh = language == "zh-CN";
    let show = MenuItem::with_id(
        app,
        "desktop-show",
        if zh {
            "显示 FoldSentinel"
        } else {
            "Show FoldSentinel"
        },
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "desktop-quit",
        if zh { "退出" } else { "Quit" },
        true,
        None::<&str>,
    )?;
    Ok(Menu::with_items(app, &[&show, &quit])?)
}

fn restore_visible(
    show: impl FnOnce() -> Result<()>,
    unminimize: impl FnOnce() -> Result<()>,
    focus: impl FnOnce() -> Result<()>,
) -> Result<()> {
    // 每步都尝试，避免恢复尺寸失败时跳过 show 而使托盘窗口不可达。
    let results = [show(), unminimize(), focus()];
    let errors: Vec<String> = results
        .into_iter()
        .filter_map(|result| result.err().map(|error| format!("{error:#}")))
        .collect();
    ensure!(errors.is_empty(), "恢复可见窗口失败: {}", errors.join("; "));
    Ok(())
}

fn reveal_raw(app: &AppHandle) -> Result<()> {
    let window = app.get_webview_window("main").context("主窗口不可用")?;
    restore_visible(
        || window.show().map_err(Into::into),
        || window.unminimize().map_err(Into::into),
        || window.set_focus().map_err(Into::into),
    )
}

fn reveal(app: &AppHandle) -> Result<()> {
    let result = reveal_raw(app);
    if let Err(error) = &result {
        if let Some(window) = app.get_webview_window("main")
            && let Err(retry) = window.show()
        {
            eprintln!("重新显示主窗口失败: {retry}");
        }
        report_action_error(app, DesktopActionError::Show, error);
    }
    result
}

fn report_event(app: &AppHandle, event: &str) {
    if let Err(error) = app.emit(event, ()) {
        eprintln!("发送桌面事件 {event} 失败: {error}");
    }
}

pub fn busy(app: &AppHandle) -> bool {
    let native = app.state::<crate::ActiveJob>();
    let active = match native.0.lock() {
        Ok(current) => current.is_some(),
        Err(_) => {
            eprintln!("活动任务锁异常，拒绝退出");
            true
        }
    };
    let desktop = app.state::<DesktopState>();
    active || desktop.busy.load(Ordering::SeqCst) || desktop.operations.load(Ordering::SeqCst) > 0
}

pub fn set_busy(app: &AppHandle, busy: bool) {
    app.state::<DesktopState>()
        .busy
        .store(busy, Ordering::SeqCst);
}

pub fn close_to_tray(app: &AppHandle) -> bool {
    get_preferences(app).is_ok_and(|value| value.close_to_tray)
}

pub fn hide(window: &tauri::Window) -> Result<()> {
    perform_hide(window.app_handle(), DesktopActionError::Hide, || {
        // 清理敏感输入的事件先于隐藏；前端也在 CloseRequested 中同步清理。
        window.emit("window-hidden", ())?;
        window.hide()?;
        Ok(())
    })
}

fn perform_hide(
    app: &AppHandle,
    action: DesktopActionError,
    hide: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let result = hide();
    if let Err(error) = &result {
        if let Err(recovery) = reveal_raw(app) {
            eprintln!("隐藏失败后恢复可见窗口失败: {recovery:#}");
        }
        report_action_error(app, action, error);
    }
    result
}

pub fn setup(app: &mut tauri::App) -> Result<()> {
    let preferences = read_preferences(&preferences_path()?).map_err(|error| format!("{error:#}"));
    // 配置损坏时保持窗口可见并由 get_window_preferences 明确报告，避免隐藏后失去入口。
    if let Err(error) = &preferences {
        eprintln!("窗口偏好未加载，保留可见窗口: {error}");
    }
    let initial = preferences.as_ref().ok().cloned().unwrap_or_default();
    app.manage(DesktopState {
        preferences: Mutex::new(preferences),
        busy: AtomicBool::new(false),
        operations: Arc::new(AtomicUsize::new(0)),
        action_error: Mutex::new(None),
    });
    let icon = app
        .default_window_icon()
        .context("应用托盘图标不可用")?
        .clone();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("FoldSentinel")
        .menu(&tray_menu(app.handle(), &initial.language)?)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) && let Err(error) = reveal(tray.app_handle())
            {
                eprintln!("显示主窗口失败: {error:#}");
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "desktop-show" => {
                if let Err(error) = reveal(app) {
                    eprintln!("显示主窗口失败: {error:#}");
                }
            }
            "desktop-quit" => {
                if busy(app) {
                    if let Err(error) = reveal(app) {
                        eprintln!("显示忙碌窗口失败: {error:#}");
                    }
                    report_event(app, "exit-blocked");
                } else {
                    app.exit(0);
                }
            }
            _ => {}
        })
        .build(app)?;
    if initial.start_minimized {
        let window = app.get_webview_window("main").context("主窗口不可用")?;
        // 托盘入口先建好，再隐藏；失败保持应用运行并尝试恢复可见窗口。
        if let Err(error) = perform_hide(app.handle(), DesktopActionError::StartupHide, || {
            window.emit("window-hidden", ())?;
            window.hide()?;
            Ok(())
        }) {
            eprintln!("启动隐藏到托盘未完成: {error:#}");
        }
    }
    Ok(())
}

pub fn notification_text(failed: bool, language: &str) -> Result<&'static str> {
    ensure!(matches!(language, "zh-CN" | "en"), "不支持的通知语言");
    Ok(match (failed, language == "zh-CN") {
        (true, true) => "任务未完成，请打开 FoldSentinel 查看详情。",
        (false, true) => "任务已完成，请打开 FoldSentinel 查看结果。",
        (true, false) => "A task did not complete. Open FoldSentinel for details.",
        (false, false) => "A task completed. Open FoldSentinel to review the result.",
    })
}

#[cfg(windows)]
pub fn notify_task(app: &AppHandle, failed: bool, language: &str) -> Result<()> {
    use windows_sys::Win32::UI::{
        Shell::{
            NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_MODIFY,
            NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::{IDI_APPLICATION, LoadIconW},
    };
    let text = notification_text(failed, language)?;
    let window = app.get_webview_window("main").context("主窗口不可用")?;
    let handle = window.hwnd()?.0 as usize;
    let mut notification = NOTIFICATION
        .lock()
        .map_err(|_| anyhow::anyhow!("通知状态锁异常"))?;
    let worker_needed = notification.is_none();
    // 单个短生命周期通知图标使用本应用窗口与独立 ID，不依赖 Tauri 私有托盘 ID。
    // 连续任务只更新这一个图标和清理期限，避免为每个任务积累图标与线程。
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = handle as _;
    data.uID = NOTIFICATION_ID;
    data.uFlags = NIF_INFO | NIF_ICON | NIF_TIP;
    data.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    data.dwInfoFlags = if failed { NIIF_WARNING } else { NIIF_INFO };
    copy_wide("FoldSentinel", &mut data.szInfoTitle);
    copy_wide("FoldSentinel", &mut data.szTip);
    copy_wide(text, &mut data.szInfo);
    let modified = !worker_needed && unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } != 0;
    // Explorer 重启后会丢弃旧图标，修改失败时重新登记同一图标。
    ensure!(
        modified || unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0,
        "请求 Windows 通知失败"
    );
    *notification = Some((
        handle,
        std::time::Instant::now() + std::time::Duration::from_secs(12),
    ));
    if worker_needed
        && let Err(error) = std::thread::Builder::new()
            .name("notification-cleanup".into())
            .spawn(clean_notification)
    {
        remove_notification(handle);
        *notification = None;
        return Err(error).context("启动通知清理任务失败");
    }
    Ok(())
}

#[cfg(windows)]
const NOTIFICATION_ID: u32 = 0xF05E;
#[cfg(windows)]
static NOTIFICATION: Mutex<Option<(usize, std::time::Instant)>> = Mutex::new(None);

#[cfg(windows)]
fn remove_notification(handle: usize) {
    use windows_sys::Win32::UI::Shell::{NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = handle as _;
    data.uID = NOTIFICATION_ID;
    if unsafe { Shell_NotifyIconW(NIM_DELETE, &data) } == 0 {
        eprintln!("清理 Windows 通知图标失败，窗口可能已关闭");
    }
}

#[cfg(windows)]
fn clean_notification() {
    loop {
        let mut state = match NOTIFICATION.lock() {
            Ok(state) => state,
            Err(error) => {
                eprintln!("通知清理状态锁异常: {error}");
                return;
            }
        };
        let Some((handle, deadline)) = *state else {
            return;
        };
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            remove_notification(handle);
            *state = None;
            return;
        }
        drop(state);
        std::thread::sleep(remaining);
    }
}

#[cfg(windows)]
fn copy_wide(text: &str, target: &mut [u16]) {
    let limit = target.len().saturating_sub(1);
    for (slot, character) in target.iter_mut().take(limit).zip(text.encode_utf16()) {
        *slot = character;
    }
}

#[cfg(not(windows))]
pub fn notify_task(_app: &AppHandle, _failed: bool, _language: &str) -> Result<()> {
    anyhow::bail!("任务通知仅支持 Windows 桌面程序")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_visible_attempts_every_step_after_a_failure() {
        let calls = std::cell::RefCell::new(Vec::new());
        assert!(
            restore_visible(
                || {
                    calls.borrow_mut().push("show");
                    anyhow::bail!("show failed")
                },
                || {
                    calls.borrow_mut().push("unminimize");
                    Ok(())
                },
                || {
                    calls.borrow_mut().push("focus");
                    Ok(())
                },
            )
            .is_err()
        );
        assert_eq!(*calls.borrow(), vec!["show", "unminimize", "focus"]);
        assert_eq!(
            serde_json::to_string(&DesktopActionError::Hide).unwrap(),
            "\"hide\""
        );
        assert_eq!(
            serde_json::to_string(&DesktopActionError::Show).unwrap(),
            "\"show\""
        );
        assert_eq!(
            serde_json::to_string(&DesktopActionError::StartupHide).unwrap(),
            "\"startup-hide\""
        );
    }
    #[test]
    fn native_operation_guard_stays_busy_until_worker_finishes() {
        let count = Arc::new(AtomicUsize::new(0));
        let guard = OperationGuard::new(count.clone());
        let (release, waiting) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = guard;
            waiting.recv().unwrap();
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn preferences_round_trip_and_reject_unexpected_fields() {
        let temporary = tempfile::tempdir().unwrap();
        let file = temporary.path().join(PREFERENCES_FILE);
        assert_eq!(
            read_preferences(&file).unwrap(),
            WindowPreferences::default()
        );
        let preferences = WindowPreferences {
            close_to_tray: true,
            start_minimized: true,
            language: "en".into(),
        };
        write_preferences(&file, &preferences).unwrap();
        assert_eq!(read_preferences(&file).unwrap(), preferences);
        write_preferences(&file, &WindowPreferences::default()).unwrap();
        assert_eq!(
            read_preferences(&file).unwrap(),
            WindowPreferences::default()
        );
        fs::write(
            &file,
            br#"{"close_to_tray":true,"start_minimized":true,"language":"en","password":"secret"}"#,
        )
        .unwrap();
        assert!(read_preferences(&file).is_err());
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
        assert!(
            WindowPreferences {
                language: "invalid".into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn invalid_preferences_leave_original_file_and_notifications_have_closed_vocabulary() {
        let temporary = tempfile::tempdir().unwrap();
        let file = temporary.path().join(PREFERENCES_FILE);
        fs::write(&file, "original").unwrap();
        assert!(
            write_preferences(
                &file,
                &WindowPreferences {
                    language: "invalid".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
        assert!(read_preferences(&file).is_err());
        for language in ["zh-CN", "en"] {
            for failed in [true, false] {
                assert!(!notification_text(failed, language).unwrap().contains('\\'));
            }
        }
        assert!(notification_text(false, "invalid").is_err());
        assert!(serde_json::from_str::<DataLocation>(r#""C:\\private""#).is_err());
        for kind in ["app_data", "recovery", "protection"] {
            assert!(serde_json::from_value::<DataLocation>(serde_json::json!(kind)).is_ok());
        }
        fs::write(&file, vec![b' '; 4097]).unwrap();
        assert!(read_preferences(&file).is_err());
    }
}
