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

#[tauri::command]
async fn vault_operation(
    input: String,
    output: String,
    password: String,
    decrypt: bool,
    on_progress: Channel<vault::Progress>,
    active: State<'_, ActiveJob>,
) -> Result<vault::VaultReport, String> {
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
    .map_err(|e| format!("{e:#}"))?;
    *delivery.lock().map_err(|e| e.to_string())? = Some(control.clone());
    {
        let mut current = active.0.lock().map_err(|e| e.to_string())?;
        if current.is_some() {
            return Err("已有加密任务正在运行".into());
        }
        *current = Some(control.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let operation = if decrypt {
            vault::decrypt_with_control
        } else {
            vault::encrypt_with_control
        };
        operation(
            &PathBuf::from(input),
            &PathBuf::from(output),
            &password,
            &control,
        )
        .map_err(|e| format!("{e:#}"))
    })
    .await;
    active.0.lock().map_err(|e| e.to_string())?.take();
    drop(delivery);
    result.map_err(|e| format!("后台任务失败，中断记录可用于清理重试: {e}"))?
}

#[tauri::command]
fn cancel_vault(active: State<'_, ActiveJob>) -> Result<bool, String> {
    Ok(active
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .is_some_and(|c| c.cancel()))
}

#[tauri::command]
async fn interrupted_jobs() -> Result<Vec<vault::recovery::InterruptedJob>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = vault::recovery::default_recovery_dir().map_err(|e| format!("{e:#}"))?;
        vault::recovery::interrupted_jobs(&root).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn cleanup_job(id: String) -> Result<vault::recovery::InterruptedJob, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = vault::recovery::default_recovery_dir().map_err(|e| format!("{e:#}"))?;
        vault::recovery::cleanup_interrupted(&root, &id).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn attribute_operation(
    path: String,
    mode: ProtectionMode,
    restore: bool,
) -> Result<OperationReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut store = StateStore::load_default().map_err(|e| format!("{e:#}"))?;
        let result = if restore {
            store.unprotect(&PathBuf::from(path))
        } else {
            store.protect(&PathBuf::from(path), mode)
        };
        result.map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| format!("后台任务失败: {e}"))?
}

#[tauri::command]
async fn protection_status() -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = StateStore::load_default().map_err(|e| format!("{e:#}"))?;
        Ok(Snapshot {
            entries: store.entries().cloned().collect(),
            state_path: store.state_path().display().to_string(),
        })
    })
    .await
    .map_err(|e| format!("后台任务失败: {e}"))?
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
