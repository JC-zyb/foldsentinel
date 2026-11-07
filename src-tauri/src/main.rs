#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use foldsentinel::{OperationReport, ProtectionMode, StateStore, vault};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State, ipc::Channel};
use zeroize::Zeroizing;
mod attributes;
mod desktop;
mod service_setup;
mod settings;
mod support;
mod uninstall;
mod updates;

#[derive(Default)]
struct ActiveJob(Mutex<Option<vault::Control>>);

#[derive(Serialize)]
struct TaskReport {
    #[serde(flatten)]
    report: vault::VaultReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<vault::PackagePreview>,
}

impl From<vault::VaultReport> for TaskReport {
    fn from(report: vault::VaultReport) -> Self {
        Self {
            report,
            preview: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct UiError {
    code: vault::ErrorCode,
    os_codes: Vec<i32>,
}

impl UiError {
    fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("后台或本地状态错误: {error}");
        Self {
            code: vault::ErrorCode::Internal,
            os_codes: Vec::new(),
        }
    }
    fn vault(error: anyhow::Error) -> Self {
        eprintln!("文件操作错误: {error:#}");
        Self {
            code: vault::error_code(&error),
            os_codes: error
                .chain()
                .filter_map(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .and_then(|io| io.raw_os_error())
                })
                .take(8)
                .collect(),
        }
    }
}

#[tauri::command]
async fn prepare_tasks(
    inputs: Vec<PathBuf>,
    operation: vault::workflow::Operation,
    directory: Option<PathBuf>,
    reserved: Vec<PathBuf>,
) -> Result<Vec<vault::workflow::PreparedTask>, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        vault::workflow::prepare(&inputs, operation, directory.as_deref(), &reserved)
            .map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn vault_operation(
    input: String,
    output: String,
    password: String,
    operation: vault::workflow::Operation,
    on_progress: Channel<vault::Progress>,
    active: State<'_, ActiveJob>,
) -> Result<TaskReport, UiError> {
    let password = Zeroizing::new(password);
    let delivery = std::sync::Arc::new(Mutex::new(None::<vault::Control>));
    let delivery_cancel = std::sync::Arc::downgrade(&delivery);
    let control = vault::Control::new(move |progress| {
        if let Err(error) = on_progress.send(progress) {
            eprintln!("进度通道已断开，尝试取消任务: {error}");
            if let Some(delivery) = delivery_cancel.upgrade()
                && let Some(control) = delivery.lock().expect("delivery mutex poisoned").as_ref()
            {
                control.cancel();
            }
        }
    })
    .map_err(UiError::vault)?;
    *delivery.lock().map_err(UiError::internal)? = Some(control.clone());
    {
        let mut current = active.0.lock().map_err(UiError::internal)?;
        if current.is_some() {
            return Err(UiError {
                code: vault::ErrorCode::Busy,
                os_codes: Vec::new(),
            });
        }
        *current = Some(control.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        use vault::workflow::Operation;
        let input = PathBuf::from(input);
        let output = PathBuf::from(output);
        match operation {
            Operation::Encrypt => vault::encrypt_with_control(&input, &output, &password, &control)
                .map(TaskReport::from),
            Operation::Decrypt => vault::decrypt_with_control(&input, &output, &password, &control)
                .map(TaskReport::from),
            Operation::Verify => {
                vault::inspect_with_control(&input, &password, &control).map(|inspection| {
                    TaskReport {
                        report: inspection.report,
                        preview: Some(inspection.preview),
                    }
                })
            }
        }
        .map_err(UiError::vault)
    })
    .await;
    active.0.lock().map_err(UiError::internal)?.take();
    drop(delivery);
    result.map_err(UiError::internal)?
}

#[tauri::command]
fn cancel_vault(active: State<'_, ActiveJob>) -> Result<bool, UiError> {
    Ok(active
        .0
        .lock()
        .map_err(UiError::internal)?
        .as_ref()
        .is_some_and(|c| c.cancel()))
}

#[tauri::command]
async fn recovery_entries() -> Result<Vec<vault::recovery::RecoveryEntry>, UiError> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = vault::recovery::default_recovery_dir().map_err(UiError::vault)?;
        vault::recovery::inspect_jobs(&root)
            .map_err(|e| UiError::vault(e.context(vault::ErrorCode::Recovery)))
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn cleanup_job(
    app: tauri::AppHandle,
    id: String,
) -> Result<vault::recovery::InterruptedJob, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let root = vault::recovery::default_recovery_dir().map_err(UiError::vault)?;
        vault::recovery::cleanup_interrupted(&root, &id)
            .map_err(|e| UiError::vault(e.context(vault::ErrorCode::Recovery)))
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
fn diagnostic_report(request: support::DiagnosticRequest) -> Result<String, UiError> {
    support::diagnostic_report(&request).map_err(UiError::vault)
}

#[tauri::command]
fn export_diagnostic(path: PathBuf, request: support::DiagnosticRequest) -> Result<(), UiError> {
    support::export_diagnostic(&path, &request).map_err(UiError::vault)
}

#[tauri::command]
fn export_system_diagnostic(path: PathBuf) -> Result<(), UiError> {
    support::export_system_diagnostic(&path).map_err(UiError::vault)
}

#[tauri::command]
fn open_output_folder(path: PathBuf) -> Result<(), UiError> {
    support::open_output_folder(&path).map_err(UiError::vault)
}

#[tauri::command]
async fn attribute_operation(
    app: tauri::AppHandle,
    path: String,
    mode: ProtectionMode,
    restore: bool,
    note: Option<String>,
) -> Result<OperationReport, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        let result = if restore {
            store.unprotect(&PathBuf::from(path))
        } else {
            store.protect_with_note(&PathBuf::from(path), mode, note.as_deref())
        };
        result.map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn preview_protection_add(
    paths: Vec<String>,
) -> Result<Vec<attributes::AddPreview>, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::preview_add(&store, &paths).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn export_protection_backup(
    app: tauri::AppHandle,
    path: PathBuf,
) -> Result<foldsentinel::BackupReport, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let store = StateStore::load_default().map_err(UiError::vault)?;
        store.export_backup(&path).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn import_protection_backup(
    app: tauri::AppHandle,
    path: PathBuf,
) -> Result<foldsentinel::BackupReport, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        store.import_backup(&path).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn protection_status(
    offset: usize,
    query: Option<String>,
) -> Result<attributes::Snapshot, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = StateStore::load_default().map_err(UiError::vault)?;
        Ok(attributes::snapshot(
            &store,
            offset,
            query.as_deref().unwrap_or(""),
        ))
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn verify_protection(id: String) -> Result<attributes::Verification, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::verify(&store, &id).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn set_protection_note(
    app: tauri::AppHandle,
    id: String,
    note: String,
) -> Result<(), UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::set_note(&mut store, &id, &note).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn restore_attributes(app: tauri::AppHandle, id: String) -> Result<OperationReport, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::restore(&mut store, &id).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn set_protection_mode(
    app: tauri::AppHandle,
    id: String,
    mode: ProtectionMode,
) -> Result<OperationReport, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::apply(&mut store, &id, mode).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn review_restore_attributes(id: String) -> Result<attributes::AclRestoreReview, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::review_restore(&store, &id).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn restore_attributes_after_review(
    app: tauri::AppHandle,
    id: String,
    token: String,
) -> Result<OperationReport, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        attributes::restore_after_review(&mut store, &id, &token).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn managed_protection(
    app: tauri::AppHandle,
    command: foldsentinel::service::Command,
) -> Result<foldsentinel::service::Response, UiError> {
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let registration_guard =
            if let foldsentinel::service::Command::Enroll { path, .. } = &command {
                let store = StateStore::load_default().map_err(UiError::vault)?;
                store
                    .ensure_service_registration_available(path)
                    .map_err(UiError::vault)?;
                Some(store)
            } else {
                None
            };
        // 同一账户的本地变更须等服务登记完成，避免检查后交错写入。
        let _registration_guard = registration_guard;
        foldsentinel::service::request(&foldsentinel::service::Request::new(command)).map_err(
            |error| {
                eprintln!("连接本机保护服务失败: {error:#}");
                UiError {
                    code: vault::ErrorCode::ServiceUnavailable,
                    os_codes: Vec::new(),
                }
            },
        )
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn service_install(app: tauri::AppHandle) -> Result<service_setup::InstallReport, UiError> {
    let guard = service_setup::acquire_install().map_err(UiError::vault)?;
    let operation = desktop::operation(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        service_setup::install_from_ui(guard).map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn service_installation_status() -> foldsentinel::service::ServiceInstallationState {
    use foldsentinel::service::ServiceInstallationState;
    match tauri::async_runtime::spawn_blocking(foldsentinel::service::installation_state).await {
        Ok(Ok(state)) => state,
        Ok(Err(error)) => {
            eprintln!("无法读取保护服务安装状态：{error:#}");
            ServiceInstallationState::Inaccessible
        }
        Err(error) => {
            eprintln!("读取保护服务安装状态的后台任务失败：{error}");
            ServiceInstallationState::Inaccessible
        }
    }
}

#[tauri::command]
fn autostart_enabled() -> Result<bool, UiError> {
    settings::autostart_enabled().map_err(UiError::internal)
}

#[tauri::command]
fn set_autostart_enabled(enabled: bool) -> Result<bool, UiError> {
    settings::set_autostart_enabled(enabled)
        .map(|()| enabled)
        .map_err(UiError::internal)
}

#[tauri::command]
async fn check_for_updates() -> Result<updates::UpdateCheck, updates::UpdateError> {
    tauri::async_runtime::spawn_blocking(updates::check_for_updates)
        .await
        .map_err(|_| updates::UpdateError::Unavailable)?
}

#[tauri::command]
async fn open_release_page(url: String) -> Result<(), updates::UpdateError> {
    tauri::async_runtime::spawn_blocking(move || updates::open_release_page(&url))
        .await
        .map_err(|_| updates::UpdateError::Unavailable)?
}

#[tauri::command]
fn get_window_preferences(app: tauri::AppHandle) -> Result<desktop::WindowPreferences, UiError> {
    desktop::get_preferences(&app).map_err(UiError::internal)
}

#[tauri::command]
fn take_desktop_action_error(
    app: tauri::AppHandle,
) -> Result<Option<desktop::DesktopActionError>, UiError> {
    desktop::take_action_error(&app).map_err(UiError::internal)
}

#[tauri::command]
fn set_window_preferences(
    app: tauri::AppHandle,
    preferences: desktop::WindowPreferences,
) -> Result<desktop::WindowPreferences, UiError> {
    desktop::set_preferences(&app, preferences).map_err(UiError::internal)
}

#[tauri::command]
fn set_desktop_busy(app: tauri::AppHandle, busy: bool) {
    desktop::set_busy(&app, busy);
}

#[tauri::command]
fn data_locations(app: tauri::AppHandle) -> Result<desktop::DataLocations, UiError> {
    desktop::data_locations(&app).map_err(UiError::internal)
}

#[tauri::command]
fn open_data_location(app: tauri::AppHandle, kind: desktop::DataLocation) -> Result<(), UiError> {
    desktop::open_data_location(&app, kind).map_err(UiError::internal)
}

#[tauri::command]
fn notify_task(app: tauri::AppHandle, failed: bool, language: String) -> Result<(), UiError> {
    desktop::notify_task(&app, failed, &language).map_err(UiError::internal)
}

fn main() {
    if let Some(code) = service_setup::maybe_run() {
        std::process::exit(code);
    }
    if let Some(code) = uninstall::maybe_run() {
        std::process::exit(code);
    }
    tauri::Builder::default()
        .manage(ActiveJob::default())
        .setup(|app| desktop::setup(app).map_err(Into::into))
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if desktop::close_to_tray(window.app_handle()) {
                    api.prevent_close();
                    if let Err(error) = desktop::hide(window) {
                        eprintln!("隐藏到托盘失败: {error:#}");
                    }
                    return;
                }
                if desktop::busy(window.app_handle()) {
                    api.prevent_close();
                }
                let state = window.state::<ActiveJob>();
                if let Some(control) = state.0.lock().expect("active job mutex poisoned").as_ref() {
                    api.prevent_close();
                    control.cancel();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            prepare_tasks,
            vault_operation,
            cancel_vault,
            recovery_entries,
            cleanup_job,
            diagnostic_report,
            export_diagnostic,
            export_system_diagnostic,
            open_output_folder,
            attribute_operation,
            managed_protection,
            service_install,
            service_installation_status,
            preview_protection_add,
            export_protection_backup,
            import_protection_backup,
            protection_status,
            verify_protection,
            set_protection_note,
            set_protection_mode,
            restore_attributes,
            review_restore_attributes,
            restore_attributes_after_review,
            autostart_enabled,
            set_autostart_enabled,
            get_window_preferences,
            take_desktop_action_error,
            set_window_preferences,
            set_desktop_busy,
            data_locations,
            open_data_location,
            notify_task,
            check_for_updates,
            open_release_page
        ])
        .build(tauri::generate_context!())
        .expect("FoldSentinel 桌面程序启动失败")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event
                && desktop::busy(app)
            {
                api.prevent_exit();
                if let Err(error) = tauri::Emitter::emit(app, "exit-blocked", ()) {
                    eprintln!("发送退出阻止事件失败: {error}");
                }
            }
        });
}
