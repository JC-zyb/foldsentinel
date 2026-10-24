#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use foldsentinel::{ManagedEntry, OperationReport, ProtectionMode, StateStore, vault};
use serde::Serialize;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Serialize)]
struct Snapshot {
    entries: Vec<ManagedEntry>,
    state_path: String,
}

#[tauri::command]
async fn vault_operation(
    input: String,
    output: String,
    password: String,
    decrypt: bool,
) -> Result<vault::VaultReport, String> {
    let password = Zeroizing::new(password);
    tauri::async_runtime::spawn_blocking(move || {
        let operation = if decrypt {
            vault::decrypt_file
        } else {
            vault::encrypt_file
        };
        operation(&PathBuf::from(input), &PathBuf::from(output), &password)
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| format!("后台任务失败: {e}"))?
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            vault_operation,
            attribute_operation,
            protection_status
        ])
        .run(tauri::generate_context!())
        .expect("FoldSentinel 桌面程序启动失败");
}
