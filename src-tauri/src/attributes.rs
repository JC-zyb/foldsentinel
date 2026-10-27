use anyhow::{Result, bail};
use foldsentinel::{OperationReport, StateStore};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

const PAGE_SIZE: usize = 50;

#[derive(Serialize)]
pub struct RecoveryItem {
    id: String,
    name: String,
}

#[derive(Serialize)]
pub struct Snapshot {
    roots: Vec<RecoveryItem>,
    total: usize,
    offset: usize,
}

// 此标识只用于后台定位记录，不是密码或访问控制凭据。
fn root_id(path: &str) -> String {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

pub fn snapshot(store: &StateStore, offset: usize) -> Snapshot {
    let roots = store.roots();
    let total = roots.len();
    let offset = offset.min(total.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE);
    Snapshot {
        roots: roots
            .into_iter()
            .skip(offset)
            .take(PAGE_SIZE)
            .map(|root| RecoveryItem {
                id: root_id(&root.path),
                name: root
                    .path
                    .trim_end_matches(['\\', '/'])
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect(),
        total,
        offset,
    }
}

pub fn restore(store: &mut StateStore, id: &str) -> Result<OperationReport> {
    let roots = store.roots();
    let mut matches = roots.iter().filter(|root| root_id(&root.path) == id);
    let Some(root) = matches.next() else {
        bail!("恢复记录已变化，请刷新列表后重试");
    };
    if matches.next().is_some() {
        bail!("恢复记录标识冲突，无法确定恢复目标");
    }
    store.unprotect(Path::new(&root.path))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use foldsentinel::ProtectionMode;
    use std::fs;

    #[test]
    fn recovery_payload_keeps_only_outer_names_and_restores_exact_root_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state.json");
        let first = temp.path().join("private-parent-a").join("资料");
        let second = temp.path().join("private-parent-b").join("资料");
        for root in [&first, &second] {
            fs::create_dir_all(root).unwrap();
            fs::write(root.join("private-child.txt"), b"keep").unwrap();
        }
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&first, ProtectionMode::Hidden).unwrap();
        store.protect(&second, ProtectionMode::Hidden).unwrap();
        let original_state = fs::read(&state).unwrap();
        let payload = serde_json::to_value(snapshot(&store, 0)).unwrap();
        assert_eq!(payload["total"], 2);
        for item in payload["roots"].as_array().unwrap() {
            assert_eq!(item.as_object().unwrap().len(), 2);
            assert_eq!(item["name"], "资料");
        }
        let json = payload.to_string();
        for secret in [
            "private-parent",
            "private-child",
            "path",
            "attributes",
            "state.json",
        ] {
            assert!(!json.contains(secret), "unexpected metadata: {secret}");
        }
        assert_eq!(fs::read(&state).unwrap(), original_state);
        let first_id = root_id(&fs::canonicalize(&first).unwrap().display().to_string());
        let second_id = root_id(&fs::canonicalize(&second).unwrap().display().to_string());
        assert_ne!(first_id, second_id);
        drop(store);
        let mut store = StateStore::load_from(state).unwrap();
        assert!(restore(&mut store, "stale-id").is_err());
        assert_eq!(store.roots().len(), 2);
        restore(&mut store, &first_id).unwrap();
        assert_eq!(store.roots().len(), 1);
        assert_eq!(snapshot(&store, 0).roots[0].id, second_id);
        restore(&mut store, &second_id).unwrap();
        assert!(store.roots().is_empty());
        for root in [first, second] {
            assert_eq!(fs::read(root.join("private-child.txt")).unwrap(), b"keep");
        }
    }
}
