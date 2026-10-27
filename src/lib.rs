mod attributes;
pub mod vault;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const STATE_VERSION: u32 = 2;
const LEGACY_MANAGED_ATTRIBUTES: u32 = attributes::HIDDEN | attributes::READONLY;
const MANAGED_ATTRIBUTES: u32 = attributes::HIDDEN | attributes::SYSTEM | attributes::READONLY;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectionMode {
    Hidden,
    Readonly,
    HiddenReadonly,
}

impl ProtectionMode {
    pub fn apply(self, original: u32) -> u32 {
        match self {
            // Explorer 的“显示隐藏的项目”只绕过 HIDDEN；同时设置 SYSTEM，
            // 让默认的“隐藏受保护的操作系统文件”仍然生效。
            Self::Hidden => original | attributes::HIDDEN | attributes::SYSTEM,
            Self::Readonly => original | attributes::READONLY,
            Self::HiddenReadonly => {
                original | attributes::HIDDEN | attributes::SYSTEM | attributes::READONLY
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedEntry {
    pub path: String,
    pub mode: ProtectionMode,
    pub original_attributes: u32,
    #[serde(default = "legacy_managed_attributes")]
    pub managed_attributes: u32,
}

fn legacy_managed_attributes() -> u32 {
    LEGACY_MANAGED_ATTRIBUTES
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFile {
    version: u32,
    entries: BTreeMap<String, ManagedEntry>,
    #[serde(default)]
    roots: BTreeSet<String>,
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            entries: BTreeMap::new(),
            roots: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OperationReport {
    pub changed: usize,
    pub missing: usize,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AttributeState {
    Consistent,
    Changed,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagedStatus {
    pub entry: ManagedEntry,
    pub current_attributes: Option<u32>,
    pub expected_attributes: u32,
    pub state: AttributeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagedRoot {
    pub path: String,
    pub entry_count: usize,
}

pub struct StateStore {
    path: PathBuf,
    data: StateFile,
    _lock: File,
}

impl StateStore {
    pub fn load_default() -> Result<Self> {
        Self::load_from(default_state_path()?)
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).context("创建恢复记录目录失败")?;
        let path = fs::canonicalize(parent)?.join(path.file_name().context("状态文件缺少名称")?);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock()
            .map_err(|e| anyhow::anyhow!("恢复记录正在使用或无法锁定，请稍后重试: {e}"))?;
        let data = match fs::read(&path) {
            Ok(bytes) => {
                let mut data: StateFile = serde_json::from_slice(&bytes)
                    .with_context(|| format!("状态文件格式无效: {}", path.display()))?;
                if ![1, STATE_VERSION].contains(&data.version) {
                    bail!(
                        "不支持的状态文件版本 {}，当前版本为 {}",
                        data.version,
                        STATE_VERSION
                    );
                }
                for (key, entry) in &data.entries {
                    if key != &entry.path || !Path::new(&entry.path).is_absolute() {
                        bail!("状态文件包含无效路径记录");
                    }
                    if ![LEGACY_MANAGED_ATTRIBUTES, MANAGED_ATTRIBUTES]
                        .contains(&entry.managed_attributes)
                    {
                        bail!("状态文件包含不支持的托管属性位");
                    }
                }
                let roots = root_counts(&data.entries).into_keys().collect();
                if !data.roots.is_empty() && data.roots != roots {
                    bail!("状态文件的最外层路径索引与恢复记录不一致");
                }
                // 旧记录从已保存的路径推导索引，不扫描磁盘，也不因路径离线删除记录。
                data.roots = roots;
                // 只读检查不落盘；下一次属性操作保存 v2，旧程序会明确拒绝读取。
                data.version = STATE_VERSION;
                data
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StateFile::default(),
            Err(e) => return Err(e).context("读取状态文件失败"),
        };

        Ok(Self {
            path,
            data,
            _lock: lock,
        })
    }

    pub fn state_path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> impl Iterator<Item = &ManagedEntry> {
        self.data.entries.values()
    }

    pub fn roots(&self) -> Vec<ManagedRoot> {
        root_counts(&self.data.entries)
            .into_iter()
            .map(|(path, entry_count)| ManagedRoot { path, entry_count })
            .collect()
    }

    pub fn protect(&mut self, input: &Path, mode: ProtectionMode) -> Result<OperationReport> {
        let root = canonical_root(input)?;
        if is_same_or_child(&path_key(&self.path), &path_key(&root)) {
            bail!("不能保护包含 FoldSentinel 恢复记录的目录");
        }
        let mut report = OperationReport::default();
        let mut planned = Vec::new();
        let existing: BTreeMap<_, _> = self
            .data
            .entries
            .keys()
            .map(|key| (comparable_path(key), key.clone()))
            .collect();
        let mut next = self.data.clone();
        for target in targets(&root)? {
            let new_key = path_key(&target);
            let key = existing
                .get(&comparable_path(&new_key))
                .cloned()
                .unwrap_or(new_key);
            let current = attributes::read(&target)?;
            let prior = self.data.entries.get(&key);
            let prior_mask =
                prior.map_or(LEGACY_MANAGED_ATTRIBUTES, |entry| entry.managed_attributes);
            let mask = if mode != ProtectionMode::Readonly {
                MANAGED_ATTRIBUTES
            } else {
                prior_mask
            };
            let original = prior.map_or(current, |entry| {
                // 升级旧记录时，新增托管位以此刻的属性为原值，保留外部变更。
                (entry.original_attributes & prior_mask) | (current & !prior_mask)
            });
            let desired = (current & !mask) | (mode.apply(original) & mask);
            next.entries.insert(
                key.clone(),
                ManagedEntry {
                    path: key,
                    mode,
                    original_attributes: original,
                    managed_attributes: mask,
                },
            );
            planned.push((target, current, desired));
        }
        next.roots = root_counts(&next.entries).into_keys().collect();
        // 最外层路径与原始属性一同先持久化；中断后仍可以从列表恢复。
        let previous = std::mem::replace(&mut self.data, next);
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        for (target, current, desired) in planned {
            vault::reject_link(&target)?;
            if current != desired {
                attributes::write(&target, desired)
                    .context("应用中途停止；恢复记录已保存，请使用恢复原始属性")?;
                report.changed += 1;
            }
        }
        Ok(report)
    }

    pub fn unprotect(&mut self, input: &Path) -> Result<OperationReport> {
        let root = lookup_root(input)?;
        let root_key = path_key(&root);
        let matching: Vec<String> = self
            .data
            .entries
            .keys()
            .filter(|key| is_same_or_child(key, &root_key))
            .cloned()
            .collect();

        if matching.is_empty() {
            bail!("没有找到受 FoldSentinel 管理的保护记录: {}", root.display());
        }

        let mut report = OperationReport::default();
        let mut completed = Vec::new();
        for key in matching {
            let entry = self
                .data
                .entries
                .get(&key)
                .cloned()
                .expect("matching key must exist");
            let path = PathBuf::from(&entry.path);
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    report.missing += 1;
                    completed.push(key);
                    continue;
                }
                Err(e) => return Err(e).context("检查待恢复路径失败，恢复记录已保留"),
                Ok(_) => {}
            }
            vault::reject_link(&path)?;
            let current = attributes::read(&path)?;
            let restored = (current & !entry.managed_attributes)
                | (entry.original_attributes & entry.managed_attributes);
            if current != restored {
                attributes::write(&path, restored)?;
                report.changed += 1;
            }
            completed.push(key);
        }

        for key in completed {
            self.data.entries.remove(&key);
        }
        self.data.roots = root_counts(&self.data.entries).into_keys().collect();
        self.save()?;
        Ok(report)
    }

    pub fn matching_entries(&self, input: Option<&Path>) -> Result<Vec<&ManagedEntry>> {
        let Some(input) = input else {
            return Ok(self.entries().collect());
        };
        let root = lookup_root(input)?;
        let root_key = path_key(&root);
        Ok(self
            .entries()
            .filter(|entry| is_same_or_child(&entry.path, &root_key))
            .collect())
    }

    pub fn statuses(
        &self,
        input: Option<&Path>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ManagedStatus>> {
        Ok(self
            .matching_entries(input)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(inspect_entry)
            .collect())
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建状态目录失败: {}", parent.display()))?;
        }
        let payload = serde_json::to_vec_pretty(&self.data).context("序列化状态失败")?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.path.parent().context("状态文件无父目录")?)?;
        temporary.write_all(&payload)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|e| anyhow::anyhow!("替换恢复记录失败: {}", e.error))?;
        Ok(())
    }
}

fn root_counts(entries: &BTreeMap<String, ManagedEntry>) -> BTreeMap<String, usize> {
    let paths: BTreeMap<_, _> = entries
        .keys()
        .map(|path| {
            (
                comparable_path(path).trim_end_matches('\\').to_owned(),
                path,
            )
        })
        .collect();
    let mut counts = BTreeMap::new();
    for path in entries.keys() {
        let comparable = comparable_path(path);
        let mut ancestor = comparable.as_str();
        let mut root = path;
        loop {
            if let Some(saved) = paths.get(ancestor) {
                root = saved;
            }
            let Some(index) = ancestor.rfind('\\') else {
                break;
            };
            ancestor = &ancestor[..index];
        }
        *counts.entry(root.clone()).or_default() += 1;
    }
    counts
}

fn first_os_code(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.raw_os_error())
    })
}

fn is_missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

fn inspect_entry(entry: &ManagedEntry) -> ManagedStatus {
    let expected = entry.mode.apply(entry.original_attributes) & entry.managed_attributes;
    let current = vault::reject_link(Path::new(&entry.path))
        .and_then(|()| attributes::read(Path::new(&entry.path)));
    let (current_attributes, state, os_code) = match current {
        Ok(value) => (
            Some(value),
            if value & entry.managed_attributes == expected {
                AttributeState::Consistent
            } else {
                AttributeState::Changed
            },
            None,
        ),
        Err(error) => (
            None,
            if is_missing(&error) {
                AttributeState::Missing
            } else {
                AttributeState::Unreadable
            },
            first_os_code(&error),
        ),
    };
    ManagedStatus {
        entry: entry.clone(),
        current_attributes,
        expected_attributes: expected,
        state,
        os_code,
    }
}

pub fn default_state_path() -> Result<PathBuf> {
    if let Some(app_data) = env::var_os("APPDATA") {
        return Ok(PathBuf::from(app_data)
            .join("FoldSentinel")
            .join("state.json"));
    }
    if let Some(home) = env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(home).join(".foldsentinel").join("state.json"));
    }
    bail!("无法确定 Windows 用户配置目录")
}

fn canonical_root(path: &Path) -> Result<PathBuf> {
    vault::reject_link(path)?;
    if !path.exists() {
        bail!("路径不存在: {}", path.display());
    }
    fs::canonicalize(path).with_context(|| format!("解析路径失败: {}", path.display()))
}

fn lookup_root(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return canonical_root(path);
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(env::current_dir()?.join(path))
}

fn targets(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_dir() {
        WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .map(|entry| {
                let entry = entry.context("遍历路径失败，未修改文件属性")?;
                vault::reject_link(entry.path())?;
                Ok(entry.path().to_path_buf())
            })
            .collect()
    } else {
        Ok(vec![root.to_path_buf()])
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

fn is_same_or_child(candidate: &str, root: &str) -> bool {
    let candidate = comparable_path(candidate);
    let root = comparable_path(root);
    candidate == root || candidate.starts_with(&format!("{}\\", root.trim_end_matches('\\')))
}

fn comparable_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}").to_lowercase();
    }
    let without_prefix = path.strip_prefix(r"\\?\").unwrap_or(path);
    without_prefix.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_application_preserves_existing_attributes() {
        let original = 0x20;
        assert_eq!(ProtectionMode::Hidden.apply(original), 0x26);
        assert_eq!(ProtectionMode::Readonly.apply(original), 0x21);
        assert_eq!(ProtectionMode::HiddenReadonly.apply(original), 0x27);
    }

    #[test]
    #[cfg(windows)]
    fn failed_state_save_does_not_change_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.json");
        let source = dir.path().join("file.txt");
        fs::write(&source, b"source").unwrap();
        let before = attributes::read(&source).unwrap();
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.save().unwrap();
        let state_before = attributes::read(&state).unwrap();
        attributes::write(&state, state_before | attributes::READONLY).unwrap();
        let result = store.protect(&source, ProtectionMode::HiddenReadonly);
        attributes::write(&state, state_before).unwrap();
        assert!(result.is_err());
        assert_eq!(attributes::read(&source).unwrap(), before);
        assert_eq!(store.entries().count(), 0);
        assert!(store.roots().is_empty());
    }

    #[test]
    fn state_round_trip_uses_versioned_json() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("state.json");
        let mut store = StateStore::load_from(path.clone()).expect("load empty state");
        store.data.entries.insert(
            "C:\\demo".to_string(),
            ManagedEntry {
                path: "C:\\demo".to_string(),
                mode: ProtectionMode::Hidden,
                original_attributes: 0x20,
                managed_attributes: LEGACY_MANAGED_ATTRIBUTES,
            },
        );
        store.save().expect("save state");
        drop(store);
        let loaded = StateStore::load_from(path).expect("reload state");
        let entries: Vec<_> = loaded.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mode, ProtectionMode::Hidden);
    }

    #[test]
    fn path_matching_handles_windows_prefix_and_case() {
        assert!(is_same_or_child(r"\\?\C:\Data\Nested\file.txt", r"c:\data"));
        assert!(!is_same_or_child(r"C:\Database", r"C:\Data"));
        assert!(is_same_or_child(
            r"\\?\UNC\server\share\nested",
            r"\\server\share"
        ));
    }

    #[test]
    fn state_lock_and_corrupt_state_are_not_silently_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load_from(path.clone()).unwrap();
        assert!(StateStore::load_from(path.clone()).is_err());
        drop(store);
        fs::write(&path, b"not json").unwrap();
        assert!(StateStore::load_from(path).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn attributes_restore_original_bits_after_repeated_protection() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("nested")).unwrap();
        let file = source.join("nested/file.txt");
        fs::write(&file, b"test").unwrap();
        let original = attributes::read(&file).unwrap();
        let mut store = StateStore::load_from(dir.path().join("state.json")).unwrap();
        store
            .protect(&source, ProtectionMode::HiddenReadonly)
            .unwrap();
        assert_eq!(
            attributes::read(&file).unwrap() & MANAGED_ATTRIBUTES,
            MANAGED_ATTRIBUTES
        );
        store.protect(&source, ProtectionMode::Hidden).unwrap();
        assert_eq!(
            attributes::read(&file).unwrap() & MANAGED_ATTRIBUTES,
            attributes::HIDDEN | attributes::SYSTEM
        );
        store.unprotect(&source).unwrap();
        assert_eq!(attributes::read(&file).unwrap(), original);
        assert_eq!(store.entries().count(), 0);
    }

    #[test]
    #[cfg(windows)]
    fn status_checks_are_readonly_and_isolate_missing_and_invalid_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("资料.txt");
        fs::write(&file, b"status contents").unwrap();
        let state_path = dir.path().join("state.json");
        let mut store = StateStore::load_from(state_path.clone()).unwrap();
        store
            .protect(&file, ProtectionMode::HiddenReadonly)
            .unwrap();
        let saved = fs::read(&state_path).unwrap();
        let original = store.entries().next().unwrap().original_attributes;
        assert_eq!(
            store.statuses(None, 0, 50).unwrap()[0].state,
            AttributeState::Consistent
        );

        // Archive 不归本工具管理；只有托管位差异才标记不一致。
        attributes::write(&file, attributes::read(&file).unwrap() ^ 0x20).unwrap();
        assert_eq!(
            store.statuses(None, 0, 50).unwrap()[0].state,
            AttributeState::Consistent
        );
        attributes::write(
            &file,
            attributes::read(&file).unwrap() & !attributes::SYSTEM,
        )
        .unwrap();
        let status = store.statuses(Some(&file), 0, 50).unwrap();
        assert_eq!(status[0].state, AttributeState::Changed);
        assert_eq!(
            status[0].current_attributes.unwrap() & attributes::SYSTEM,
            0
        );
        assert_eq!(fs::read(&state_path).unwrap(), saved);

        let entry = store.entries().next().unwrap().clone();
        for name in ["missing.txt", "invalid?.txt"] {
            let path = path_key(&dir.path().join(name));
            store.data.entries.insert(
                path.clone(),
                ManagedEntry {
                    path,
                    ..entry.clone()
                },
            );
        }
        let statuses = store.statuses(None, 0, 50).unwrap();
        assert_eq!(statuses.len(), 3);
        assert!(statuses.iter().any(|s| s.state == AttributeState::Missing));
        assert!(
            statuses
                .iter()
                .any(|s| s.state == AttributeState::Unreadable)
        );
        assert_eq!(store.statuses(None, 1, 1).unwrap().len(), 1);
        assert!(store.statuses(None, 3, 50).unwrap().is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"status contents");
        attributes::write(&file, original).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn legacy_records_preserve_external_system_flags_and_upgrade_only_on_apply() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("old.txt");
        fs::write(&file, b"legacy source").unwrap();
        let original = attributes::read(&file).unwrap();
        let path = path_key(&fs::canonicalize(&file).unwrap());
        let state_path = dir.path().join("state.json");
        let legacy = serde_json::json!({ "version": 1, "entries": {
            &path: { "path": &path, "mode": "hidden", "original_attributes": original }
        }});
        fs::write(&state_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        attributes::write(&file, original | attributes::HIDDEN | attributes::SYSTEM).unwrap();
        let mut store = StateStore::load_from(state_path.clone()).unwrap();
        assert_eq!(
            store.entries().next().unwrap().managed_attributes,
            LEGACY_MANAGED_ATTRIBUTES
        );
        assert_eq!(
            store.statuses(None, 0, 1).unwrap()[0].state,
            AttributeState::Consistent
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&state_path).unwrap()).unwrap()["version"],
            1
        );
        store.unprotect(&file).unwrap();
        assert_eq!(
            attributes::read(&file).unwrap(),
            original | attributes::SYSTEM
        );

        // 全新只读操作不取得 System 所有权；重新应用隐藏才增加托管位。
        store.protect(&file, ProtectionMode::Readonly).unwrap();
        assert_eq!(
            store.entries().next().unwrap().managed_attributes,
            LEGACY_MANAGED_ATTRIBUTES
        );
        store.protect(&file, ProtectionMode::Hidden).unwrap();
        assert_eq!(
            store.entries().next().unwrap().managed_attributes,
            MANAGED_ATTRIBUTES
        );
        store.protect(&file, ProtectionMode::Readonly).unwrap();
        assert_eq!(
            attributes::read(&file).unwrap() & MANAGED_ATTRIBUTES,
            attributes::READONLY | attributes::SYSTEM
        );
        store.unprotect(&file).unwrap();
        assert_eq!(
            attributes::read(&file).unwrap(),
            original | attributes::SYSTEM
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&state_path).unwrap()).unwrap()["version"],
            STATE_VERSION
        );
        attributes::write(&file, original).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn saved_roots_survive_restart_merge_parents_and_restore_independently() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("资料");
        let child = root.join("子目录");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(root.join("空目录")).unwrap();
        let file = child.join("内容.txt");
        fs::write(&file, b"saved-root source").unwrap();
        let other = dir.path().join("资料副本.txt");
        fs::write(&other, b"independent source").unwrap();
        let original = attributes::read(&file).unwrap();
        let state_path = dir.path().join("state.json");
        let mut store = StateStore::load_from(state_path.clone()).unwrap();
        store.protect(&child, ProtectionMode::Hidden).unwrap();
        store
            .protect(&root, ProtectionMode::HiddenReadonly)
            .unwrap();
        store.protect(&other, ProtectionMode::Hidden).unwrap();
        store.protect(&child, ProtectionMode::Hidden).unwrap();
        let roots = store.roots();
        assert_eq!(roots.len(), 2);
        let root_key = path_key(&fs::canonicalize(&root).unwrap());
        assert!(roots.contains(&ManagedRoot {
            path: root_key.clone(),
            entry_count: 4
        }));
        let saved = fs::read(&state_path).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(json["roots"].as_array().unwrap().len(), 2);
        drop(store);

        let mut restarted = StateStore::load_from(state_path.clone()).unwrap();
        assert_eq!(restarted.roots(), roots);
        assert_eq!(fs::read(&state_path).unwrap(), saved);
        assert_ne!(attributes::read(&file).unwrap() & attributes::HIDDEN, 0);
        restarted.unprotect(&child).unwrap();
        assert!(restarted.roots().contains(&ManagedRoot {
            path: root_key,
            entry_count: 2
        }));
        assert_eq!(attributes::read(&file).unwrap(), original);
        restarted.unprotect(&root).unwrap();
        assert_eq!(restarted.roots().len(), 1);
        assert_ne!(attributes::read(&other).unwrap() & attributes::HIDDEN, 0);
        drop(restarted);
        let mut restarted = StateStore::load_from(state_path).unwrap();
        assert_eq!(restarted.roots().len(), 1);
        restarted.unprotect(&other).unwrap();
        assert!(restarted.roots().is_empty());
        assert_eq!(fs::read(file).unwrap(), b"saved-root source");
        assert_eq!(fs::read(other).unwrap(), b"independent source");
    }

    #[test]
    #[cfg(windows)]
    fn legacy_missing_roots_are_inferred_without_disk_scans_or_state_rewrites() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let paths = [
            r"C:\Missing",
            r"c:\missing\child.txt",
            r"C:\Missing-copy.txt",
            r"\\?\UNC\server\share\folder",
            r"\\server\share\folder\child.txt",
        ];
        let entries: BTreeMap<_, _> = paths
            .into_iter()
            .map(|path| {
                (
                    path,
                    serde_json::json!({"path": path, "mode": "hidden", "original_attributes": 32}),
                )
            })
            .collect();
        let saved =
            serde_json::to_vec(&serde_json::json!({"version": 1, "entries": entries})).unwrap();
        fs::write(&state_path, &saved).unwrap();
        let store = StateStore::load_from(state_path.clone()).unwrap();
        let roots = store.roots();
        assert_eq!(roots.len(), 3);
        assert!(
            roots
                .iter()
                .any(|r| r.path == r"C:\Missing" && r.entry_count == 2)
        );
        assert!(
            roots
                .iter()
                .any(|r| r.path == r"\\?\UNC\server\share\folder" && r.entry_count == 2)
        );
        assert_eq!(fs::read(&state_path).unwrap(), saved);
        drop(store);
        let mut invalid: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        invalid["roots"] = serde_json::json!([r"C:\unrelated"]);
        fs::write(&state_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(StateStore::load_from(state_path).is_err());
    }
}
