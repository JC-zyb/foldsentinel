mod attributes;
pub mod vault;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const STATE_VERSION: u32 = 1;
const MANAGED_ATTRIBUTES: u32 = attributes::HIDDEN | attributes::READONLY;

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
            Self::Hidden => original | attributes::HIDDEN,
            Self::Readonly => original | attributes::READONLY,
            Self::HiddenReadonly => original | attributes::HIDDEN | attributes::READONLY,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedEntry {
    pub path: String,
    pub mode: ProtectionMode,
    pub original_attributes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFile {
    version: u32,
    entries: BTreeMap<String, ManagedEntry>,
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OperationReport {
    pub changed: usize,
    pub missing: usize,
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
                let data: StateFile = serde_json::from_slice(&bytes)
                    .with_context(|| format!("状态文件格式无效: {}", path.display()))?;
                if data.version != STATE_VERSION {
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
                }
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
            let original = match self.data.entries.get(&key) {
                Some(entry) => entry.original_attributes,
                None => attributes::read(&target)?,
            };
            let current = attributes::read(&target)?;
            let desired =
                (current & !MANAGED_ATTRIBUTES) | (mode.apply(original) & MANAGED_ATTRIBUTES);
            next.entries.insert(
                key.clone(),
                ManagedEntry {
                    path: key,
                    mode,
                    original_attributes: original,
                },
            );
            planned.push((target, current, desired));
        }
        // 先持久化所有原始属性；中途失败或进程中断后仍可以 unprotect。
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
            let restored =
                (current & !MANAGED_ATTRIBUTES) | (entry.original_attributes & MANAGED_ATTRIBUTES);
            if current != restored {
                attributes::write(&path, restored)?;
                report.changed += 1;
            }
            completed.push(key);
        }

        for key in completed {
            self.data.entries.remove(&key);
        }
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
        assert_eq!(ProtectionMode::Hidden.apply(original), 0x22);
        assert_eq!(ProtectionMode::Readonly.apply(original), 0x21);
        assert_eq!(ProtectionMode::HiddenReadonly.apply(original), 0x23);
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
            attributes::HIDDEN
        );
        store.unprotect(&source).unwrap();
        assert_eq!(attributes::read(&file).unwrap(), original);
        assert_eq!(store.entries().count(), 0);
    }
}
