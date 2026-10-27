#![cfg(windows)]

use foldsentinel::{ProtectionCheck, ProtectionMode, StateStore};
use std::fs;
use std::path::PathBuf;

struct RecoveryOnDrop {
    state: PathBuf,
    root: PathBuf,
    directory: Option<tempfile::TempDir>,
}

impl Drop for RecoveryOnDrop {
    fn drop(&mut self) {
        let recovery = StateStore::load_from(self.state.clone()).and_then(|mut store| {
            if !store.roots().is_empty() {
                store.unprotect(&self.root)?;
            }
            Ok(())
        });
        if let Err(error) = recovery
            && let Some(directory) = self.directory.take()
        {
            let retained = directory.keep();
            eprintln!(
                "隔离夹具恢复失败，已保留目录 {}：{error:#}",
                retained.display()
            );
        }
    }
}

#[test]
fn permission_readonly_roundtrips_modes_and_backup_after_reload() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("权限只读核心");
    fs::create_dir(&root).unwrap();
    let file = root.join("内容.txt");
    fs::write(&file, b"original-content").unwrap();
    let state_path = temp.path().join("state.json");
    let backup = temp.path().join("readonly.fsbackup.json");
    let imported_path = temp.path().join("restored.json");
    let _recovery = RecoveryOnDrop {
        state: state_path.clone(),
        root: root.clone(),
        directory: Some(temp),
    };
    let mut store = StateStore::load_from(state_path.clone()).unwrap();
    store
        .protect_with_note(&root, ProtectionMode::ReadonlyAcl, Some("只读项目"))
        .unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"original-content");
    assert!(fs::read_dir(&root).is_ok());
    assert!(fs::write(&file, b"blocked").is_err());
    assert!(fs::write(root.join("new.txt"), b"blocked").is_err());
    assert!(fs::remove_file(&file).is_err());
    for overlapping in [&root, &file, root.parent().unwrap()] {
        assert_eq!(
            foldsentinel::vault::error_code(
                &store
                    .ensure_service_registration_available(overlapping)
                    .unwrap_err()
            ),
            foldsentinel::vault::ErrorCode::ProtectionLocalOverlap
        );
    }
    let root_key = store.roots()[0].path.clone();
    assert_eq!(
        store.verify_root(&root_key).unwrap(),
        ProtectionCheck::Consistent
    );
    drop(store);
    let mut store = StateStore::load_from(state_path).unwrap();
    assert_eq!(
        store.root_mode(&root_key),
        Some(ProtectionMode::ReadonlyAcl)
    );
    store.protect(&root, ProtectionMode::HiddenLocked).unwrap();
    assert!(fs::read(&file).is_err());
    store.protect(&root, ProtectionMode::ReadonlyAcl).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"original-content");
    assert!(fs::write(&file, b"blocked-again").is_err());
    store.export_backup(&backup).unwrap();
    drop(store);
    let mut imported = StateStore::load_from(imported_path).unwrap();
    imported.import_backup(&backup).unwrap();
    assert_eq!(
        imported.root_mode(&root_key),
        Some(ProtectionMode::ReadonlyAcl)
    );
    imported.unprotect(&root).unwrap();
    imported
        .ensure_service_registration_available(&root)
        .unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"original-content");
    fs::write(&file, b"restored-writable").unwrap();
    fs::write(root.join("new.txt"), b"restored-create").unwrap();
}
