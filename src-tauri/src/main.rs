#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use foldsentinel::{ManagedEntry, OperationReport, ProtectionMode, StateStore, vault};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State, ipc::Channel};
use zeroize::Zeroizing;

#[derive(Serialize)]
struct Snapshot {
    entries: Vec<ManagedEntry>,
    state_path: String,
}

#[derive(Default)]
struct ActiveJob(Mutex<Option<vault::Control>>);

#[derive(Debug, Serialize)]
struct UiError {
    code: vault::ErrorCode,
}

impl UiError {
    fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("后台或本地状态错误: {error}");
        Self {
            code: vault::ErrorCode::Internal,
        }
    }
    fn vault(error: anyhow::Error) -> Self {
        eprintln!("文件操作错误: {error:#}");
        Self {
            code: vault::error_code(&error),
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
) -> Result<vault::VaultReport, UiError> {
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
            });
        }
        *current = Some(control.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        use vault::workflow::Operation;
        let input = PathBuf::from(input);
        let output = PathBuf::from(output);
        match operation {
            Operation::Encrypt => vault::encrypt_with_control(&input, &output, &password, &control),
            Operation::Decrypt => vault::decrypt_with_control(&input, &output, &password, &control),
            Operation::Verify => vault::verify_with_control(&input, &password, &control),
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
async fn interrupted_jobs() -> Result<Vec<vault::recovery::InterruptedJob>, UiError> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = vault::recovery::default_recovery_dir().map_err(UiError::vault)?;
        vault::recovery::interrupted_jobs(&root).map_err(|e| {
            eprintln!("中断记录错误: {e:#}");
            UiError {
                code: vault::ErrorCode::Recovery,
            }
        })
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn cleanup_job(id: String) -> Result<vault::recovery::InterruptedJob, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = vault::recovery::default_recovery_dir().map_err(UiError::vault)?;
        vault::recovery::cleanup_interrupted(&root, &id).map_err(|e| {
            eprintln!("中断清理错误: {e:#}");
            UiError {
                code: vault::ErrorCode::Recovery,
            }
        })
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn attribute_operation(
    path: String,
    mode: ProtectionMode,
    restore: bool,
) -> Result<OperationReport, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut store = StateStore::load_default().map_err(UiError::vault)?;
        let result = if restore {
            store.unprotect(&PathBuf::from(path))
        } else {
            store.protect(&PathBuf::from(path), mode)
        };
        result.map_err(UiError::vault)
    })
    .await
    .map_err(UiError::internal)?
}

#[tauri::command]
async fn protection_status() -> Result<Snapshot, UiError> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = StateStore::load_default().map_err(UiError::vault)?;
        Ok(Snapshot {
            entries: store.entries().cloned().collect(),
            state_path: store.state_path().display().to_string(),
        })
    })
    .await
    .map_err(UiError::internal)?
}

fn main() {
    tauri::Builder::default()
        .manage(ActiveJob::default())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
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
            interrupted_jobs,
            cleanup_job,
            attribute_operation,
            protection_status
        ])
        .run(tauri::generate_context!())
        .expect("FoldSentinel 桌面程序启动失败");
}
