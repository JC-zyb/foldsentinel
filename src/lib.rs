mod attributes;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const STATE_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationReport {
    pub changed: usize,
    pub missing: usize,
}

pub struct StateStore {
    path: PathBuf,
    data: StateFile,
}

impl StateStore {
    pub fn load_default() -> Result<Self> {
        Self::load_from(default_state_path()?)
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        let data = if path.exists() {
            let bytes =
                fs::read(&path).with_context(|| format!("读取状态文件失败: {}", path.display()))?;
            let data: StateFile = serde_json::from_slice(&bytes)
                .with_context(|| format!("状态文件格式无效: {}", path.display()))?;
            if data.version != STATE_VERSION {
                bail!(
                    "不支持的状态文件版本 {}，当前版本为 {}",
                    data.version,
                    STATE_VERSION
                );
            }
            data
        } else {
            StateFile::default()
        };

        Ok(Self { path, data })
    }

    pub fn state_path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> impl Iterator<Item = &ManagedEntry> {
        self.data.entries.values()
    }

    pub fn protect(&mut self, input: &Path, mode: ProtectionMode) -> Result<OperationReport> {
        let root = canonical_root(input)?;
        let mut report = OperationReport::default();

        for target in targets(&root) {
            let key = path_key(&target);
            let original = match self.data.entries.get(&key) {
                Some(entry) => entry.original_attributes,
                None => attributes::read(&target)?,
            };
            let desired = mode.apply(original);
            if attributes::read(&target)? != desired {
                attributes::write(&target, desired)?;
                report.changed += 1;
            }
            self.data.entries.insert(
                key.clone(),
                ManagedEntry {
                    path: key,
                    mode,
                    original_attributes: original,
                },
            );
        }

        self.save()?;
        Ok(report)
    }

    pub fn unprotect(&mut self, input: &Path) -> Result<OperationReport> {
        let root = canonical_root(input)?;
        let root_key = path_key(&root);
        let separator = format!("{}\\", root_key.trim_end_matches('\\'));
        let matching: Vec<String> = self
            .data
            .entries
            .keys()
            .filter(|key| *key == &root_key || key.starts_with(&separator))
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
            if !path.exists() {
                report.missing += 1;
                completed.push(key);
                continue;
            }
            let current = attributes::read(&path)?;
            if current != entry.original_attributes {
                attributes::write(&path, entry.original_attributes)?;
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
        let root = canonical_root(input)?;
        let root_key = path_key(&root);
        let separator = format!("{}\\", root_key.trim_end_matches('\\'));
        Ok(self
            .entries()
            .filter(|entry| entry.path == root_key || entry.path.starts_with(&separator))
            .collect())
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建状态目录失败: {}", parent.display()))?;
        }
        let payload = serde_json::to_vec_pretty(&self.data).context("序列化状态失败")?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, payload)
            .with_context(|| format!("写入临时状态文件失败: {}", temporary.display()))?;
        fs::rename(&temporary, &self.path)
            .with_context(|| format!("替换状态文件失败: {}", self.path.display()))?;
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
    if !path.exists() {
        bail!("路径不存在: {}", path.display());
    }
    fs::canonicalize(path).with_context(|| format!("解析路径失败: {}", path.display()))
}

fn targets(root: &Path) -> Vec<PathBuf> {
    if root.is_dir() {
        WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path().to_path_buf())
            .collect()
    } else {
        vec![root.to_path_buf()]
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
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
        let loaded = StateStore::load_from(path).expect("reload state");
        let entries: Vec<_> = loaded.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mode, ProtectionMode::Hidden);
    }
}
