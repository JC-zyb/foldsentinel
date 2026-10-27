use anyhow::{Context, Result, bail};
use foldsentinel::{
    OperationReport, ProtectionCheck, ProtectionMode, StateStore, vault::ErrorCode,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const PAGE_SIZE: usize = 50;

#[derive(Serialize)]
pub struct RecoveryItem {
    id: String,
    name: String,
    mode: Option<ProtectionMode>,
    note: String,
    duplicate_name: bool,
}

#[derive(Serialize)]
pub struct Snapshot {
    roots: Vec<RecoveryItem>,
    total: usize,
    offset: usize,
    overall_total: usize,
}

#[derive(Serialize)]
pub struct Verification {
    state: ProtectionCheck,
    checked_at: u64,
}

#[derive(Debug, Serialize)]
pub struct AddPreview {
    index: usize,
    name: String,
    duplicate_name: bool,
}

pub fn preview_add(store: &StateStore, paths: &[String]) -> Result<Vec<AddPreview>> {
    if paths.len() > 200 {
        bail!(ErrorCode::ProtectionInput);
    }
    let mut counts = BTreeMap::new();
    for root in store.roots() {
        *counts
            .entry(outer_name(&root.path).to_lowercase())
            .or_insert(0usize) += 1;
    }
    for path in paths {
        let name = outer_name(path);
        if name.is_empty() || name.chars().any(char::is_control) {
            bail!(ErrorCode::ProtectionInput);
        }
        *counts.entry(name.to_lowercase()).or_insert(0) += 1;
    }
    Ok(paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let name = outer_name(path);
            AddPreview {
                index,
                name: name.to_owned(),
                duplicate_name: counts[&name.to_lowercase()] > 1,
            }
        })
        .collect())
}

// 此标识只用于后台定位记录，不是密码或访问控制凭据。
fn root_id(path: &str) -> String {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn outer_name(path: &str) -> &str {
    path.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
}

pub fn snapshot(store: &StateStore, offset: usize, query: &str) -> Snapshot {
    let roots = store.roots();
    let overall_total = roots.len();
    let mut name_counts = BTreeMap::new();
    for root in &roots {
        *name_counts
            .entry(outer_name(&root.path).to_lowercase())
            .or_insert(0) += 1;
    }
    let query = query.trim().to_lowercase();
    let filtered: Vec<_> = roots
        .into_iter()
        .filter(|root| {
            outer_name(&root.path).to_lowercase().contains(&query)
                || store.root_note(&root.path).to_lowercase().contains(&query)
        })
        .collect();
    let total = filtered.len();
    let offset = offset.min(total.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE);
    Snapshot {
        roots: filtered
            .into_iter()
            .skip(offset)
            .take(PAGE_SIZE)
            .map(|root| RecoveryItem {
                id: root_id(&root.path),
                mode: store.root_mode(&root.path),
                name: outer_name(&root.path).to_owned(),
                note: store.root_note(&root.path).to_owned(),
                duplicate_name: name_counts[&outer_name(&root.path).to_lowercase()] > 1,
            })
            .collect(),
        total,
        offset,
        overall_total,
    }
}

pub fn verify(store: &StateStore, id: &str) -> Result<Verification> {
    let path = resolve_root(store, id)?;
    let state = store.verify_root(&path)?;
    let checked_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("无法读取核验时间")?
        .as_millis()
        .try_into()
        .context("核验时间超出范围")?;
    Ok(Verification { state, checked_at })
}

pub fn set_note(store: &mut StateStore, id: &str, note: &str) -> Result<()> {
    let path = resolve_root(store, id)?;
    store.set_root_note(&path, note)
}

pub fn restore(store: &mut StateStore, id: &str) -> Result<OperationReport> {
    let path = resolve_root(store, id)?;
    store.unprotect(Path::new(&path))
}

pub fn apply(store: &mut StateStore, id: &str, mode: ProtectionMode) -> Result<OperationReport> {
    let path = resolve_root(store, id)?;
    store.protect(Path::new(&path), mode)
}

fn resolve_root(store: &StateStore, id: &str) -> Result<String> {
    let roots = store.roots();
    let mut matches = roots.iter().filter(|root| root_id(&root.path) == id);
    let Some(root) = matches.next() else {
        bail!(ErrorCode::ProtectionRecordChanged);
    };
    if matches.next().is_some() {
        bail!(ErrorCode::ProtectionRecordChanged);
    }
    Ok(root.path.clone())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use foldsentinel::ProtectionMode;
    use std::fs;

    #[test]
    fn add_preview_checks_all_records_and_this_batch_without_path_disclosure() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = StateStore::load_from(temp.path().join("state.json")).unwrap();
        for index in 0..51 {
            let root = temp.path().join(format!("root-{index:02}"));
            fs::create_dir(&root).unwrap();
            store.protect(&root, ProtectionMode::Hidden).unwrap();
        }
        let paths = vec![
            r"C:\private-parent\ROOT-50".to_owned(),
            r"C:\private-parent\资料".to_owned(),
            r"C:\other-private-parent\资料".to_owned(),
            r"C:\private-parent\unique".to_owned(),
        ];
        let preview = preview_add(&store, &paths).unwrap();
        assert_eq!(
            preview
                .iter()
                .map(|entry| entry.duplicate_name)
                .collect::<Vec<_>>(),
            [true, true, true, false]
        );
        assert_eq!(
            preview.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        let public = serde_json::to_string(&preview).unwrap();
        assert!(!public.contains("private-parent"));
        assert!(!public.contains("C:"));
        assert!(preview_add(&store, &vec!["x".to_owned(); 201]).is_err());
    }

    #[test]
    fn search_filters_all_roots_before_pagination_and_keeps_duplicate_markers() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        for index in 0..51 {
            let root = temp.path().join(format!("项目{index:02}"));
            fs::create_dir(&root).unwrap();
            store.protect(&root, ProtectionMode::Hidden).unwrap();
        }
        for parent in ["private-parent-a", "private-parent-b"] {
            let root = temp.path().join(parent).join("资料");
            fs::create_dir_all(&root).unwrap();
            store.protect(&root, ProtectionMode::Hidden).unwrap();
        }
        let first_page = snapshot(&store, 0, "");
        assert_eq!(first_page.roots.len(), PAGE_SIZE);
        assert_eq!(first_page.overall_total, 53);
        let last = snapshot(&store, 50, "").roots.pop().unwrap();
        set_note(&mut store, &last.id, "  Research ARCHIVE  ").unwrap();
        let saved = fs::read(&state).unwrap();
        let found = snapshot(&store, 500, " research archive ");
        assert_eq!(found.total, 1);
        assert_eq!(found.offset, 0);
        assert_eq!(found.overall_total, 53);
        assert_eq!(found.roots[0].id, last.id);
        assert_eq!(found.roots[0].note, "Research ARCHIVE");
        let duplicates = snapshot(&store, 0, "资料");
        assert_eq!(duplicates.total, 2);
        assert!(duplicates.roots.iter().all(|root| root.duplicate_name));
        let public = serde_json::to_string(&found).unwrap();
        assert!(!public.contains("private-parent"));
        assert!(!public.contains("state.json"));
        assert_eq!(fs::read(&state).unwrap(), saved);
        drop(store);
        let store = StateStore::load_from(state).unwrap();
        assert_eq!(snapshot(&store, 0, "RESEARCH").roots[0].id, last.id);
    }

    #[test]
    fn verification_returns_only_aggregate_state_and_timestamp() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private-parent").join("资料");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("private-child.txt"), b"keep").unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&root, ProtectionMode::Hidden).unwrap();
        let id = snapshot(&store, 0, "").roots[0].id.clone();
        let saved = fs::read(&state).unwrap();
        let value = serde_json::to_value(verify(&store, &id).unwrap()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert_eq!(value["state"], "consistent");
        assert!(value["checked_at"].as_u64().unwrap() > 0);
        let json = value.to_string();
        for secret in ["private-parent", "private-child", "path", "attributes"] {
            assert!(!json.contains(secret));
        }
        assert_eq!(fs::read(&state).unwrap(), saved);
        let error = verify(&store, "stale-id").err().unwrap();
        assert_eq!(
            foldsentinel::vault::error_code(&error),
            ErrorCode::ProtectionRecordChanged
        );
        store.unprotect(&root).unwrap();
    }

    #[test]
    fn policy_selection_by_id_survives_restart_without_path_disclosure() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state.json");
        let path = temp.path().join("private-parent").join("资料");
        fs::create_dir_all(&path).unwrap();
        let file = path.join("private-child.txt");
        fs::write(&file, b"keep").unwrap();
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&path, ProtectionMode::Hidden).unwrap();
        let id = snapshot(&store, 0, "").roots[0].id.clone();
        apply(&mut store, &id, ProtectionMode::HiddenLocked).unwrap();
        assert!(fs::read(&file).is_err());
        drop(store);
        let mut store = StateStore::load_from(state).unwrap();
        let value = serde_json::to_value(snapshot(&store, 0, "")).unwrap();
        assert_eq!(value["roots"][0]["mode"], "hidden-locked");
        assert!(!value.to_string().contains("private-"));
        apply(&mut store, &id, ProtectionMode::None).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        assert_eq!(snapshot(&store, 0, "").total, 0);
        assert!(apply(&mut store, &id, ProtectionMode::Locked).is_err());
    }

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
        let payload = serde_json::to_value(snapshot(&store, 0, "")).unwrap();
        assert_eq!(payload["total"], 2);
        for item in payload["roots"].as_array().unwrap() {
            assert_eq!(item.as_object().unwrap().len(), 5);
            assert_eq!(item["note"], "");
            assert_eq!(item["duplicate_name"], true);
            assert_eq!(item["mode"], "hidden");
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
        assert_eq!(snapshot(&store, 0, "").roots[0].id, second_id);
        restore(&mut store, &second_id).unwrap();
        assert!(store.roots().is_empty());
        for root in [first, second] {
            assert_eq!(fs::read(root.join("private-child.txt")).unwrap(), b"keep");
        }
    }
}
