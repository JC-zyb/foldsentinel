mod acl;
mod attributes;
mod backup;
pub mod managed;
pub mod service;
pub mod vault;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const STATE_VERSION: u32 = 3;
const LEGACY_MANAGED_ATTRIBUTES: u32 = attributes::HIDDEN | attributes::READONLY;
const MANAGED_ATTRIBUTES: u32 = attributes::HIDDEN | attributes::SYSTEM | attributes::READONLY;
const MAX_BACKUP_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectionMode {
    None,
    Hidden,
    Locked,
    Readonly,
    ReadonlyAcl,
    HiddenReadonly,
    HiddenLocked,
}

impl ProtectionMode {
    pub fn apply(self, original: u32) -> u32 {
        match self {
            Self::None | Self::Locked | Self::ReadonlyAcl => original,
            // Explorer 的“显示隐藏的项目”只绕过 HIDDEN；同时设置 SYSTEM，
            // 让默认的“隐藏受保护的操作系统文件”仍然生效。
            Self::Hidden => original | attributes::HIDDEN | attributes::SYSTEM,
            Self::Readonly => original | attributes::READONLY,
            Self::HiddenReadonly => {
                original | attributes::HIDDEN | attributes::SYSTEM | attributes::READONLY
            }
            Self::HiddenLocked => original | attributes::HIDDEN | attributes::SYSTEM,
        }
    }
}

impl ProtectionMode {
    fn is_locked(self) -> bool {
        matches!(self, Self::Locked | Self::HiddenLocked | Self::ReadonlyAcl)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedEntry {
    pub path: String,
    pub mode: ProtectionMode,
    pub original_attributes: u32,
    #[serde(default = "legacy_managed_attributes")]
    pub managed_attributes: u32,
    #[serde(default)]
    pub lock: Option<acl::LockRecord>,
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
    // 可选展示元数据，不参与权限恢复；旧 v3 记录无需迁移。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    root_notes: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryBackup {
    format: String,
    version: u32,
    protected_state: backup::Sealed,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryBackupPayload {
    state: StateFile,
    identities: BTreeMap<String, [u64; 3]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupReport {
    pub roots: usize,
    pub entries: usize,
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            entries: BTreeMap::new(),
            roots: BTreeSet::new(),
            root_notes: BTreeMap::new(),
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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectionCheck {
    Consistent,
    Changed,
    Unavailable,
}

pub struct StateStore {
    path: PathBuf,
    data: StateFile,
    _lock: File,
    #[cfg(test)]
    local_check: Option<LocalRangeCheck>,
}

#[cfg(test)]
type LocalRangeCheck = Box<dyn Fn(&[PathBuf]) -> Result<()>>;

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
            Ok(bytes) => parse_state(&bytes)
                .with_context(|| format!("状态文件格式无效: {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StateFile::default(),
            Err(e) => return Err(e).context("读取状态文件失败"),
        };

        Ok(Self {
            path,
            data,
            _lock: lock,
            #[cfg(test)]
            local_check: None,
        })
    }

    pub fn state_path(&self) -> &Path {
        &self.path
    }

    fn ensure_local_available(&self, paths: &[PathBuf]) -> Result<()> {
        #[cfg(test)]
        if let Some(check) = &self.local_check {
            return check(paths);
        }
        service::ensure_local_available(paths)
    }

    /// 新服务登记须先解除本地恢复入口的重叠项目，避免两份原始权限互相覆盖。
    pub fn ensure_service_registration_available(&self, input: &Path) -> Result<()> {
        let root = path_key(&canonical_root(input)?);
        if self
            .data
            .entries
            .keys()
            .any(|entry| is_same_or_child(entry, &root) || is_same_or_child(&root, entry))
        {
            bail!(vault::ErrorCode::ProtectionLocalOverlap);
        }
        Ok(())
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

    pub fn root_mode(&self, root: &str) -> Option<ProtectionMode> {
        let mut modes = self
            .entries()
            .filter(|entry| is_same_or_child(&entry.path, root))
            .map(|entry| entry.mode);
        let first = modes.next()?;
        modes.all(|mode| mode == first).then_some(first)
    }

    pub fn root_note(&self, root: &str) -> &str {
        self.data.root_notes.get(root).map_or("", String::as_str)
    }

    pub fn set_root_note(&mut self, root: &str, note: &str) -> Result<()> {
        if !self.data.roots.contains(root) {
            bail!(vault::ErrorCode::ProtectionRecordChanged);
        }
        let note = validated_note(note)?;
        self.ensure_local_available(&[PathBuf::from(root)])?;
        let previous = self.data.root_notes.clone();
        if note.is_empty() {
            self.data.root_notes.remove(root);
        } else {
            self.data
                .root_notes
                .insert(root.to_owned(), note.to_owned());
        }
        if let Err(error) = self.save() {
            self.data.root_notes = previous;
            return Err(error);
        }
        Ok(())
    }

    // 仅核验已登记内容，不枚举目录，也不改变属性、权限或恢复记录。
    pub fn verify_root(&self, root: &str) -> Result<ProtectionCheck> {
        if !self.data.roots.contains(root) {
            bail!(vault::ErrorCode::ProtectionRecordChanged);
        }
        let entries: Vec<_> = self
            .entries()
            .filter(|entry| is_same_or_child(&entry.path, root))
            .collect();
        let mut state = match verify_lock_parents(entries.iter().copied()) {
            Ok(()) => ProtectionCheck::Consistent,
            Err(error) if vault::error_code(&error) == vault::ErrorCode::ProtectionConflict => {
                ProtectionCheck::Changed
            }
            Err(_) => ProtectionCheck::Unavailable,
        };
        for entry in entries {
            match inspect_entry(entry).state {
                AttributeState::Missing | AttributeState::Unreadable => {
                    return Ok(ProtectionCheck::Unavailable);
                }
                AttributeState::Changed if state != ProtectionCheck::Unavailable => {
                    state = ProtectionCheck::Changed;
                }
                _ => {}
            }
        }
        Ok(state)
    }

    pub fn protect(&mut self, input: &Path, mode: ProtectionMode) -> Result<OperationReport> {
        self.protect_with_note(input, mode, None)
    }

    pub fn protect_with_note(
        &mut self,
        input: &Path,
        mode: ProtectionMode,
        note: Option<&str>,
    ) -> Result<OperationReport> {
        let note = note.map(validated_note).transpose()?;
        if mode == ProtectionMode::None {
            if self.matching_entries(Some(input))?.is_empty() {
                return Ok(OperationReport::default());
            }
            return self.unprotect(input);
        }
        let root = canonical_root(input)?;
        self.ensure_local_available(std::slice::from_ref(&root))?;
        self.reject_locked_ancestor(&path_key(&root))?;
        if is_same_or_child(&path_key(&self.path), &path_key(&root)) {
            bail!(vault::ErrorCode::ProtectionLocation);
        }
        let mut report = OperationReport::default();
        if mode.is_locked() {
            validate_lock_root(&root)?;
        }
        let mut planned = Vec::new();
        let existing: BTreeMap<_, _> = self
            .data
            .entries
            .keys()
            .map(|key| (comparable_path(key), key.clone()))
            .collect();
        let mut next = self.data.clone();
        // 已锁定的目录不可枚举；策略切换使用持久化的子项记录。
        let root_key = path_key(&root);
        let has_lock = self
            .data
            .entries
            .values()
            .any(|entry| entry.lock.is_some() && is_same_or_child(&entry.path, &root_key));
        let mut paths = if has_lock {
            let locked: BTreeSet<_> = self
                .entries()
                .filter(|entry| entry.lock.is_some())
                .map(|entry| comparable_path(&entry.path))
                .collect();
            let mut found = BTreeMap::new();
            for entry in WalkDir::new(&root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| !locked.contains(&comparable_path(&path_key(entry.path()))))
            {
                let entry = entry.context("遍历路径失败，未修改保护方式")?;
                vault::reject_link(entry.path())
                    .context(vault::ErrorCode::ProtectionUnsupported)?;
                found.insert(
                    comparable_path(&path_key(entry.path())),
                    entry.path().to_owned(),
                );
            }
            for entry in self.matching_entries(Some(&root))? {
                found.insert(comparable_path(&entry.path), PathBuf::from(&entry.path));
            }
            found.into_values().collect()
        } else {
            targets(&root)?
        };
        paths.sort_by_key(|path| path.components().count());
        for target in paths {
            let new_key = path_key(&target);
            let key = existing
                .get(&comparable_path(&new_key))
                .cloned()
                .unwrap_or(new_key);
            let current = attributes::read(&target)?;
            let prior = self.data.entries.get(&key);
            let prior_mask =
                prior.map_or(LEGACY_MANAGED_ATTRIBUTES, |entry| entry.managed_attributes);
            let mask = match mode {
                ProtectionMode::Hidden
                | ProtectionMode::HiddenReadonly
                | ProtectionMode::HiddenLocked => MANAGED_ATTRIBUTES,
                ProtectionMode::Readonly
                | ProtectionMode::ReadonlyAcl
                | ProtectionMode::Locked
                | ProtectionMode::None => prior_mask,
            };
            let original = prior.map_or(current, |entry| {
                // 升级旧记录时，新增托管位以此刻的属性为原值，保留外部变更。
                (entry.original_attributes & prior_mask) | (current & !prior_mask)
            });
            let desired = if matches!(
                mode,
                ProtectionMode::None | ProtectionMode::Locked | ProtectionMode::ReadonlyAcl
            ) {
                (current & !mask) | (original & mask)
            } else {
                (current & !mask) | (mode.apply(original) & mask)
            };
            let object = if mode.is_locked() || prior.is_some_and(|entry| entry.lock.is_some()) {
                Some(acl::Object::open(&target).context(vault::ErrorCode::ProtectionUnsupported)?)
            } else {
                None
            };
            let lock = if let Some(lock) = prior.and_then(|entry| entry.lock.clone()) {
                object.as_ref().expect("lock handle").verify(&lock)?;
                if mode.is_locked()
                    && prior.is_some_and(|entry| {
                        (entry.mode == ProtectionMode::ReadonlyAcl)
                            != (mode == ProtectionMode::ReadonlyAcl)
                    })
                {
                    Some(
                        object
                            .as_ref()
                            .expect("lock handle")
                            .replace_policy(&lock, mode == ProtectionMode::ReadonlyAcl)?,
                    )
                } else {
                    Some(lock)
                }
            } else if mode.is_locked() {
                let object = object.as_ref().expect("lock handle");
                Some(if mode == ProtectionMode::ReadonlyAcl {
                    object.prepare_readonly(true)?
                } else {
                    object.prepare()?
                })
            } else {
                None
            };
            next.entries.insert(
                key.clone(),
                ManagedEntry {
                    path: key.clone(),
                    mode,
                    original_attributes: original,
                    managed_attributes: mask,
                    lock,
                },
            );
            planned.push((target, key, current, desired, object));
        }
        next.roots = root_counts(&next.entries).into_keys().collect();
        next.root_notes.retain(|root, _| next.roots.contains(root));
        if let Some(note) = note {
            let registered_root = next
                .roots
                .iter()
                .find(|candidate| comparable_path(candidate) == comparable_path(&root_key));
            if let Some(registered_root) = registered_root {
                if note.is_empty() {
                    next.root_notes.remove(registered_root);
                } else {
                    next.root_notes
                        .insert(registered_root.clone(), note.to_owned());
                }
            } else if !note.is_empty() {
                // 添加子项目不能悄悄覆盖既有最外层项目的区分备注。
                bail!(vault::ErrorCode::ProtectionConflict);
            }
        }
        verify_lock_parents(planned.iter().map(|(_, key, _, _, _)| &next.entries[key]))?;
        // 最外层路径与原始属性一同先持久化；中断后仍可以从列表恢复。
        let previous = std::mem::replace(&mut self.data, next);
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        // 锁定自底向上；恢复继承自顶向下，避免父项传播覆盖未恢复的子项。
        if mode.is_locked() {
            planned.reverse();
        }
        for (target, key, current, desired, object) in planned {
            vault::reject_link(&target)?;
            let mut changed = current != desired;
            if let Some(lock) = &self.data.entries[&key].lock {
                let object = object.as_ref().expect("lock handle");
                let current_acl = object.verify(lock)?;
                let desired_acl = if mode.is_locked() {
                    &lock.locked
                } else {
                    &lock.original
                };
                if !acl::equivalent(&current_acl, desired_acl) {
                    object
                        .write(desired_acl)
                        .context("权限应用中途停止，恢复记录已保留")?;
                    changed = true;
                }
            }
            if current != desired {
                attributes::write(&target, desired)
                    .context("应用中途停止；恢复记录已保存，请使用恢复原始属性")?;
            }
            if changed {
                report.changed += 1;
            }
        }
        if !mode.is_locked() {
            let before_cleanup = self.data.clone();
            for entry in self
                .data
                .entries
                .values_mut()
                .filter(|entry| is_same_or_child(&entry.path, &root_key))
            {
                entry.lock = None;
            }
            if let Err(error) = self.save() {
                self.data = before_cleanup;
                return Err(error);
            }
        }
        Ok(report)
    }

    pub fn unprotect(&mut self, input: &Path) -> Result<OperationReport> {
        let root = lookup_root(input)?;
        self.ensure_local_available(std::slice::from_ref(&root))?;
        let root_key = path_key(&root);
        self.reject_locked_ancestor(&root_key)?;
        let mut matching: Vec<String> = self
            .data
            .entries
            .keys()
            .filter(|key| is_same_or_child(key, &root_key))
            .cloned()
            .collect();
        matching.sort_by_key(|key| Path::new(key).components().count());

        if matching.is_empty() {
            bail!("没有找到受 FoldSentinel 管理的保护记录: {}", root.display());
        }
        verify_lock_parents(matching.iter().map(|key| &self.data.entries[key]))?;

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
                    if entry.lock.is_some() {
                        bail!("锁定路径缺失或磁盘离线，恢复记录已保留");
                    }
                    completed.push(key);
                    continue;
                }
                Err(e) => return Err(e).context("检查待恢复路径失败，恢复记录已保留"),
                Ok(_) => {}
            }
            vault::reject_link(&path)?;
            let mut changed = false;
            let object = if let Some(lock) = &entry.lock {
                let object = acl::Object::open(&path)?;
                let current = object.verify(lock)?;
                if !acl::equivalent(&current, &lock.original) {
                    object.write(&lock.original)?;
                    changed = true;
                }
                Some(object)
            } else {
                None
            };
            let current = attributes::read(&path)?;
            let restored = (current & !entry.managed_attributes)
                | (entry.original_attributes & entry.managed_attributes);
            if current != restored {
                attributes::write(&path, restored)?;
                changed = true;
            }
            drop(object);
            if changed {
                report.changed += 1;
            }
            completed.push(key);
        }

        let previous = self.data.clone();
        for key in completed {
            self.data.entries.remove(&key);
        }
        self.data.roots = root_counts(&self.data.entries).into_keys().collect();
        self.data
            .root_notes
            .retain(|root, _| self.data.roots.contains(root));
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
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

    fn reject_locked_ancestor(&self, root: &str) -> Result<()> {
        if self.entries().any(|entry| {
            entry.lock.is_some()
                && comparable_path(root) != comparable_path(&entry.path)
                && is_same_or_child(root, &entry.path)
        }) {
            bail!(vault::ErrorCode::ProtectionConflict);
        }
        Ok(())
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

    pub fn export_backup(&self, destination: &Path) -> Result<BackupReport> {
        let mut identities = BTreeMap::new();
        for entry in self.entries() {
            let identity = backup::identity(Path::new(&entry.path))?;
            if entry
                .lock
                .as_ref()
                .is_some_and(|lock| lock.identity != identity)
            {
                bail!(vault::ErrorCode::ProtectionConflict);
            }
            identities.insert(entry.path.clone(), identity);
        }
        let plaintext = zeroize::Zeroizing::new(serde_json::to_vec(&RecoveryBackupPayload {
            state: self.data.clone(),
            identities,
        })?);
        let backup = RecoveryBackup {
            format: "foldsentinel-recovery".to_owned(),
            version: 1,
            protected_state: backup::seal(&plaintext)?,
        };
        let bytes = serde_json::to_vec(&backup)?;
        if bytes.len() as u64 > MAX_BACKUP_BYTES {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        Ok(BackupReport {
            roots: self.data.roots.len(),
            entries: self.data.entries.len(),
        })
    }

    pub fn import_backup(&mut self, source: &Path) -> Result<BackupReport> {
        if !self.data.entries.is_empty() || !self.data.roots.is_empty() {
            bail!(vault::ErrorCode::ProtectionBackupConflict);
        }
        vault::reject_link(source).context(vault::ErrorCode::ProtectionBackupInvalid)?;
        let mut bytes = Vec::new();
        File::open(source)?
            .take(MAX_BACKUP_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BACKUP_BYTES {
            bail!(vault::ErrorCode::ProtectionBackupInvalid);
        }
        let backup: RecoveryBackup =
            serde_json::from_slice(&bytes).context(vault::ErrorCode::ProtectionBackupInvalid)?;
        if backup.format != "foldsentinel-recovery" || backup.version != 1 {
            bail!(vault::ErrorCode::ProtectionBackupInvalid);
        }
        let plaintext = zeroize::Zeroizing::new(
            backup::open(&backup.protected_state)
                .context(vault::ErrorCode::ProtectionBackupInvalid)?,
        );
        let payload: RecoveryBackupPayload = serde_json::from_slice(&plaintext)
            .context(vault::ErrorCode::ProtectionBackupInvalid)?;
        let data = parse_state(&serde_json::to_vec(&payload.state)?)
            .context(vault::ErrorCode::ProtectionBackupInvalid)?;
        if data.entries.len() != payload.identities.len() {
            bail!(vault::ErrorCode::ProtectionBackupInvalid);
        }
        self.ensure_local_available(&data.roots.iter().map(PathBuf::from).collect::<Vec<_>>())?;
        for root in &data.roots {
            if is_same_or_child(&path_key(&self.path), root) {
                bail!(vault::ErrorCode::ProtectionLocation);
            }
            if data
                .entries
                .values()
                .any(|entry| entry.mode.is_locked() && is_same_or_child(&entry.path, root))
            {
                validate_lock_root(Path::new(root))?;
            }
        }
        // 导入只登记原恢复计划，不修改文件。身份、属性和 ACL 冲突必须先人工核查。
        for entry in data.entries.values() {
            let identity = backup::identity(Path::new(&entry.path))?;
            if payload.identities.get(&entry.path) != Some(&identity) {
                bail!(vault::ErrorCode::ProtectionConflict);
            }
            let current = attributes::read(Path::new(&entry.path))? & entry.managed_attributes;
            let original = entry.original_attributes & entry.managed_attributes;
            let expected = entry.mode.apply(entry.original_attributes) & entry.managed_attributes;
            if current != original && current != expected {
                bail!(vault::ErrorCode::ProtectionConflict);
            }
            if let Some(lock) = &entry.lock {
                acl::Object::open(Path::new(&entry.path))?.verify(lock)?;
            }
        }
        verify_lock_parents(data.entries.values())?;
        let report = BackupReport {
            roots: data.roots.len(),
            entries: data.entries.len(),
        };
        let previous = std::mem::replace(&mut self.data, data);
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(report)
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

fn validated_note(note: &str) -> Result<&str> {
    let note = note.trim();
    if note.chars().count() > 80 || note.chars().any(char::is_control) {
        bail!(vault::ErrorCode::ProtectionInput);
    }
    Ok(note)
}

fn parse_state(bytes: &[u8]) -> Result<StateFile> {
    let mut data: StateFile = serde_json::from_slice(bytes)?;
    if ![1, 2, STATE_VERSION].contains(&data.version) {
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
        if ![LEGACY_MANAGED_ATTRIBUTES, MANAGED_ATTRIBUTES].contains(&entry.managed_attributes) {
            bail!("状态文件包含不支持的托管属性位");
        }
        if entry.lock.as_ref().is_some_and(|lock| lock.version != 1)
            || (entry.mode.is_locked() && entry.lock.is_none())
        {
            bail!("状态文件包含无效的锁定恢复记录");
        }
    }
    let roots = root_counts(&data.entries).into_keys().collect();
    if !data.roots.is_empty() && data.roots != roots {
        bail!("状态文件的最外层路径索引与恢复记录不一致");
    }
    // 旧记录从已保存的路径推导索引，不扫描磁盘，也不因路径离线删除记录。
    data.roots = roots;
    for (root, note) in &data.root_notes {
        if !data.roots.contains(root) || validated_note(note)? != note {
            bail!(vault::ErrorCode::ProtectionInput);
        }
    }
    // 只读检查不落盘；下一次操作保存 v3，旧程序会明确拒绝读取。
    data.version = STATE_VERSION;
    Ok(data)
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

fn verify_lock_parents<'a>(entries: impl Iterator<Item = &'a ManagedEntry>) -> Result<()> {
    let entries: Vec<_> = entries.collect();
    let managed: BTreeMap<_, _> = entries
        .iter()
        .filter(|entry| entry.lock.is_some())
        .map(|entry| {
            (
                comparable_path(&entry.path),
                entry.lock.as_ref().expect("lock record"),
            )
        })
        .collect();
    for entry in entries {
        let Some(lock) = &entry.lock else {
            continue;
        };
        let path = Path::new(&entry.path);
        let parent = path.parent().context("锁定记录缺少父目录")?;
        let header = lock.original.split('(').next().unwrap_or_default();
        if header.contains('P') {
            continue;
        }
        if let Some(parent_record) = managed.get(&comparable_path(&path_key(parent))) {
            if lock
                .parent
                .as_ref()
                .is_some_and(|original| !acl::equivalent(original, &parent_record.original))
            {
                bail!(vault::ErrorCode::ProtectionConflict);
            }
            continue;
        }
        if let Some(original) = &lock.parent
            && !acl::equivalent(&acl::Object::read_parent(path)?, original)
        {
            bail!(vault::ErrorCode::ProtectionConflict);
        }
    }
    Ok(())
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
    let current = vault::reject_link(Path::new(&entry.path)).and_then(|()| {
        let value = attributes::read(Path::new(&entry.path))?;
        let acl_matches = if let Some(lock) = &entry.lock {
            let object = acl::Object::open(Path::new(&entry.path))?;
            let current = object.verify(lock)?;
            acl::equivalent(
                &current,
                if entry.mode.is_locked() {
                    &lock.locked
                } else {
                    &lock.original
                },
            )
        } else {
            true
        };
        Ok((value, acl_matches))
    });
    let (current_attributes, state, os_code) = match current {
        Ok((value, acl_matches)) => (
            Some(value),
            if value & entry.managed_attributes == expected && acl_matches {
                AttributeState::Consistent
            } else {
                AttributeState::Changed
            },
            None,
        ),
        Err(error) => (
            None,
            if vault::error_code(&error) == vault::ErrorCode::ProtectionConflict {
                AttributeState::Changed
            } else if is_missing(&error) {
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
    vault::reject_link(path).map_err(|error| {
        let code = if is_missing(&error) {
            vault::ErrorCode::ProtectionInput
        } else {
            vault::ErrorCode::ProtectionUnsupported
        };
        error.context(code)
    })?;
    if !path.exists() {
        bail!(vault::ErrorCode::ProtectionInput);
    }
    fs::canonicalize(path).with_context(|| format!("解析路径失败: {}", path.display()))
}

fn validate_lock_root(root: &Path) -> Result<()> {
    let key = path_key(root);
    if root.parent().is_none() {
        bail!(vault::ErrorCode::ProtectionLocation);
    }
    for name in [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ] {
        if let Some(value) = env::var_os(name) {
            let reserved = path_key(&PathBuf::from(value));
            if is_same_or_child(&key, &reserved) || is_same_or_child(&reserved, &key) {
                bail!(vault::ErrorCode::ProtectionLocation);
            }
        }
    }
    for name in ["USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        if let Some(value) = env::var_os(name)
            && is_same_or_child(&path_key(&PathBuf::from(value)), &key)
        {
            bail!(vault::ErrorCode::ProtectionLocation);
        }
    }
    if is_same_or_child(&path_key(&env::current_exe()?), &key) {
        bail!(vault::ErrorCode::ProtectionLocation);
    }
    Ok(())
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
                vault::reject_link(entry.path())
                    .context(vault::ErrorCode::ProtectionUnsupported)?;
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

    #[cfg(windows)]
    fn no_service_for_fixture(store: &mut StateStore) {
        store.local_check = Some(Box::new(|paths| {
            service::ensure_local_available_using(
                paths,
                || Ok(false),
                |_| panic!("未安装服务时不能请求管道"),
            )
        }));
    }

    #[test]
    #[cfg(windows)]
    fn overlap_guard_rejects_all_mutations_before_file_or_state_changes() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("local.txt");
        fs::write(&file, b"unchanged-content").unwrap();
        let state = fixture.path().join("state.json");
        let backup = fixture.path().join("recovery.fsb");
        let original = attributes::read(&file).unwrap();
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.local_check = Some(Box::new(|_| {
            bail!(vault::ErrorCode::ProtectionServiceOverlap)
        }));
        assert_eq!(
            vault::error_code(&store.protect(&file, ProtectionMode::Hidden).unwrap_err()),
            vault::ErrorCode::ProtectionServiceOverlap
        );
        assert!(!state.exists());
        assert_eq!(attributes::read(&file).unwrap(), original);
        assert_eq!(store.entries().count(), 0);

        no_service_for_fixture(&mut store);
        store
            .protect_with_note(&file, ProtectionMode::Hidden, Some("原备注"))
            .unwrap();
        store.export_backup(&backup).unwrap();
        let saved = fs::read(&state).unwrap();
        let protected = attributes::read(&file).unwrap();
        let root = store.roots()[0].path.clone();
        store.local_check = Some(Box::new(|_| {
            bail!(vault::ErrorCode::ProtectionServiceOverlap)
        }));
        for result in [
            store.protect(&file, ProtectionMode::Readonly),
            store.unprotect(&file),
            store.protect(&file, ProtectionMode::None),
        ] {
            assert_eq!(
                vault::error_code(&result.unwrap_err()),
                vault::ErrorCode::ProtectionServiceOverlap
            );
        }
        assert_eq!(
            vault::error_code(&store.set_root_note(&root, "不能保存").unwrap_err()),
            vault::ErrorCode::ProtectionServiceOverlap
        );
        assert_eq!(fs::read(&state).unwrap(), saved);
        assert_eq!(attributes::read(&file).unwrap(), protected);
        assert_eq!(store.root_note(&root), "原备注");
        drop(store);

        // 模拟只丢失本地状态文件；导入在登记前也必须通过服务范围门。
        fs::remove_file(&state).unwrap();
        let mut imported = StateStore::load_from(state.clone()).unwrap();
        imported.local_check = Some(Box::new(|_| {
            bail!(vault::ErrorCode::ProtectionServiceOverlap)
        }));
        assert_eq!(
            vault::error_code(&imported.import_backup(&backup).unwrap_err()),
            vault::ErrorCode::ProtectionServiceOverlap
        );
        assert!(!state.exists());
        assert_eq!(imported.entries().count(), 0);
        assert_eq!(attributes::read(&file).unwrap(), protected);
        no_service_for_fixture(&mut imported);
        imported.import_backup(&backup).unwrap();
        imported.unprotect(&file).unwrap();
        assert_eq!(attributes::read(&file).unwrap(), original);
        assert_eq!(fs::read(&file).unwrap(), b"unchanged-content");
    }

    #[test]
    #[cfg(windows)]
    fn overlap_guard_local_first_rejects_service_same_ancestor_and_child_and_holds_state_lock() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("parent").join("root");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("child.txt");
        fs::write(&file, b"keep").unwrap();
        let outside = fixture.path().join("outside.txt");
        fs::write(&outside, b"outside").unwrap();
        let state = fixture.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        let locked_state = state.clone();
        store.local_check = Some(Box::new(move |paths| {
            assert!(StateStore::load_from(locked_state.clone()).is_err());
            service::ensure_local_available_using(
                paths,
                || Ok(false),
                |_| panic!("未安装服务时不能请求管道"),
            )
        }));
        store.protect(&root, ProtectionMode::Hidden).unwrap();
        for input in [&root, root.parent().unwrap(), &file] {
            assert_eq!(
                vault::error_code(
                    &store
                        .ensure_service_registration_available(input)
                        .unwrap_err()
                ),
                vault::ErrorCode::ProtectionLocalOverlap
            );
        }
        store
            .ensure_service_registration_available(&outside)
            .unwrap();
        store.unprotect(&root).unwrap();
        drop(store);
        assert!(StateStore::load_from(state).is_ok());
    }

    #[test]
    #[cfg(windows)]
    fn overlap_guard_installed_unreachable_prevents_local_writes_and_restore() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("source.txt");
        fs::write(&file, b"keep").unwrap();
        let state = fixture.path().join("state.json");
        let original = attributes::read(&file).unwrap();
        let mut store = StateStore::load_from(state.clone()).unwrap();
        no_service_for_fixture(&mut store);
        store.protect(&file, ProtectionMode::Hidden).unwrap();
        let protected = attributes::read(&file).unwrap();
        let saved = fs::read(&state).unwrap();
        store.local_check = Some(Box::new(|paths| {
            service::ensure_local_available_using(
                paths,
                || Ok(true),
                |_| Err(std::io::Error::from_raw_os_error(2).into()),
            )
        }));
        for result in [
            store.protect(&file, ProtectionMode::Readonly),
            store.unprotect(&file),
        ] {
            assert_eq!(
                vault::error_code(&result.unwrap_err()),
                vault::ErrorCode::ServiceUnavailable
            );
        }
        assert_eq!(fs::read(&state).unwrap(), saved);
        assert_eq!(attributes::read(&file).unwrap(), protected);
        no_service_for_fixture(&mut store);
        store.unprotect(&file).unwrap();
        assert_eq!(attributes::read(&file).unwrap(), original);
    }

    #[test]
    #[cfg(windows)]
    fn initial_note_is_saved_with_plan_and_failed_save_changes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("资料.txt");
        fs::write(&file, b"keep").unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        let before = attributes::read(&file).unwrap();
        fs::create_dir(&state).unwrap();
        assert!(
            store
                .protect_with_note(&file, ProtectionMode::HiddenLocked, Some("工作资料"))
                .is_err()
        );
        assert!(store.entries().next().is_none());
        assert!(store.data.root_notes.is_empty());
        assert_eq!(attributes::read(&file).unwrap(), before);
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        fs::remove_dir(&state).unwrap();
        store
            .protect_with_note(&file, ProtectionMode::HiddenLocked, Some("  工作资料  "))
            .unwrap();
        let root = store.roots()[0].path.clone();
        assert_eq!(store.root_note(&root), "工作资料");
        assert!(fs::read(&file).is_err());
        drop(store);
        let mut store = StateStore::load_from(state).unwrap();
        assert_eq!(store.root_note(&root), "工作资料");
        store.unprotect(&file).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"keep");
    }

    #[test]
    #[cfg(windows)]
    fn child_initial_note_cannot_overwrite_registered_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("资料");
        fs::create_dir(&root).unwrap();
        let file = root.join("内部.txt");
        fs::write(&file, b"keep").unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store
            .protect_with_note(&root, ProtectionMode::Hidden, Some("原始备注"))
            .unwrap();
        let saved = fs::read(&state).unwrap();
        let before = attributes::read(&file).unwrap();
        let error = store
            .protect_with_note(&file, ProtectionMode::Readonly, Some("子项备注"))
            .unwrap_err();
        assert_eq!(
            vault::error_code(&error),
            vault::ErrorCode::ProtectionConflict
        );
        assert_eq!(fs::read(&state).unwrap(), saved);
        assert_eq!(attributes::read(&file).unwrap(), before);
        assert_eq!(store.root_note(&store.roots()[0].path), "原始备注");
        store.unprotect(&root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn encrypted_backup_import_restores_full_plan_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("private-child.txt");
        fs::write(&file, b"keep backup content").unwrap();
        let original_attributes = attributes::read(&file).unwrap();
        let mut store = StateStore::load_from(temp.path().join("state.json")).unwrap();
        store
            .protect_with_note(&file, ProtectionMode::HiddenLocked, Some("private-note"))
            .unwrap();
        let root = store.roots()[0].path.clone();
        let backup = temp.path().join("backup.fsbackup.json");
        let report = store.export_backup(&backup).unwrap();
        assert_eq!((report.roots, report.entries), (1, 1));
        let saved = fs::read(&backup).unwrap();
        let text = String::from_utf8(saved.clone()).unwrap();
        assert!(!text.contains("private-child"));
        assert!(!text.contains("private-note"));
        let error = store.export_backup(&backup).unwrap_err();
        assert_eq!(vault::error_code(&error), vault::ErrorCode::OutputExists);
        assert_eq!(fs::read(&backup).unwrap(), saved);
        let error = store.import_backup(&backup).unwrap_err();
        assert_eq!(
            vault::error_code(&error),
            vault::ErrorCode::ProtectionBackupConflict
        );
        drop(store);
        let restored_state = temp.path().join("restored-state.json");
        let mut restored = StateStore::load_from(restored_state.clone()).unwrap();
        let report = restored.import_backup(&backup).unwrap();
        assert_eq!((report.roots, report.entries), (1, 1));
        assert_eq!(restored.root_note(&root), "private-note");
        assert!(fs::read(&file).is_err(), "导入不能解除锁定");
        drop(restored);
        let mut restored = StateStore::load_from(restored_state).unwrap();
        restored.unprotect(&file).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"keep backup content");
        assert_eq!(attributes::read(&file).unwrap(), original_attributes);
    }

    #[test]
    #[cfg(windows)]
    fn backup_rejects_tampering_replacement_and_failed_commit_without_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("资料.txt");
        fs::write(&file, b"original").unwrap();
        let mut store = StateStore::load_from(temp.path().join("state.json")).unwrap();
        store.protect(&file, ProtectionMode::Hidden).unwrap();
        let backup_path = temp.path().join("backup.json");
        store.export_backup(&backup_path).unwrap();
        let saved = fs::read(&backup_path).unwrap();
        let mut corrupted: RecoveryBackup = serde_json::from_slice(&saved).unwrap();
        let mut encrypted = serde_json::to_value(&corrupted.protected_state).unwrap();
        let byte = encrypted["ciphertext"][0].as_u64().unwrap();
        encrypted["ciphertext"][0] = serde_json::json!(byte ^ 1);
        corrupted.protected_state = serde_json::from_value(encrypted).unwrap();
        fs::write(&backup_path, serde_json::to_vec(&corrupted).unwrap()).unwrap();
        let new_state = temp.path().join("new-state.json");
        let mut restored = StateStore::load_from(new_state.clone()).unwrap();
        assert_eq!(
            vault::error_code(&restored.import_backup(&backup_path).unwrap_err()),
            vault::ErrorCode::ProtectionBackupInvalid
        );
        assert!(restored.entries().next().is_none());
        assert!(!new_state.exists());
        fs::write(&backup_path, saved).unwrap();
        let current_attributes = attributes::read(&file).unwrap();
        fs::create_dir(&new_state).unwrap();
        assert!(restored.import_backup(&backup_path).is_err());
        assert!(restored.entries().next().is_none());
        assert_eq!(attributes::read(&file).unwrap(), current_attributes);
        fs::remove_dir(&new_state).unwrap();
        store.unprotect(&file).unwrap();
        fs::remove_file(&file).unwrap();
        fs::write(&file, b"replacement").unwrap();
        assert_eq!(
            vault::error_code(&restored.import_backup(&backup_path).unwrap_err()),
            vault::ErrorCode::ProtectionConflict
        );
        assert!(restored.entries().next().is_none());
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
    }

    #[test]
    #[cfg(windows)]
    fn backup_import_rejects_authenticated_invalid_state_and_external_acl() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("资料.txt");
        fs::write(&file, b"keep").unwrap();
        let mut store = StateStore::load_from(temp.path().join("state.json")).unwrap();
        store.protect(&file, ProtectionMode::HiddenLocked).unwrap();
        let backup_path = temp.path().join("backup.json");
        store.export_backup(&backup_path).unwrap();
        let mut backup: RecoveryBackup =
            serde_json::from_slice(&fs::read(&backup_path).unwrap()).unwrap();
        let mut payload: RecoveryBackupPayload =
            serde_json::from_slice(&backup::open(&backup.protected_state).unwrap()).unwrap();
        payload
            .state
            .entries
            .values_mut()
            .next()
            .unwrap()
            .managed_attributes = u32::MAX;
        backup.protected_state = backup::seal(&serde_json::to_vec(&payload).unwrap()).unwrap();
        let invalid_path = temp.path().join("invalid.json");
        fs::write(&invalid_path, serde_json::to_vec(&backup).unwrap()).unwrap();
        let mut restored = StateStore::load_from(temp.path().join("restored-state.json")).unwrap();
        assert_eq!(
            vault::error_code(&restored.import_backup(&invalid_path).unwrap_err()),
            vault::ErrorCode::ProtectionBackupInvalid
        );
        let lock = store.entries().next().unwrap().lock.clone().unwrap();
        let object = acl::Object::open(&file).unwrap();
        object
            .write(&lock.locked.replacen("(D;", "(A;", 1))
            .unwrap();
        assert!(restored.import_backup(&backup_path).is_err());
        assert!(restored.entries().next().is_none());
        object.write(&lock.original).unwrap();
        drop(object);
        store.unprotect(&file).unwrap();
    }

    #[test]
    fn mode_application_preserves_existing_attributes() {
        let original = 0x20;
        assert_eq!(ProtectionMode::Hidden.apply(original), 0x26);
        assert_eq!(ProtectionMode::Readonly.apply(original), 0x21);
        assert_eq!(ProtectionMode::HiddenReadonly.apply(original), 0x27);
        assert_eq!(ProtectionMode::Locked.apply(original), original);
        assert_eq!(ProtectionMode::HiddenLocked.apply(original), 0x26);
    }

    #[test]
    #[cfg(windows)]
    fn root_notes_survive_restart_and_do_not_change_protection() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state.json");
        let file = temp.path().join("资料.txt");
        fs::write(&file, b"keep").unwrap();
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&file, ProtectionMode::Hidden).unwrap();
        let root = store.roots()[0].path.clone();
        let before = attributes::read(&file).unwrap();
        store.set_root_note(&root, "  合同备份  ").unwrap();
        assert_eq!(store.root_note(&root), "合同备份");
        for invalid in ["换行\n备注".to_owned(), "长".repeat(81)] {
            let error = store.set_root_note(&root, &invalid).unwrap_err();
            assert_eq!(vault::error_code(&error), vault::ErrorCode::ProtectionInput);
        }
        assert_eq!(store.root_note(&root), "合同备份");
        assert_eq!(attributes::read(&file).unwrap(), before);
        drop(store);
        let mut store = StateStore::load_from(state).unwrap();
        assert_eq!(store.root_note(&root), "合同备份");
        store.unprotect(&file).unwrap();
        assert!(store.data.root_notes.is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"keep");
    }

    #[test]
    #[cfg(windows)]
    fn failed_note_save_preserves_previous_note_and_protection() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state.json");
        let file = temp.path().join("资料.txt");
        fs::write(&file, b"keep").unwrap();
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&file, ProtectionMode::Hidden).unwrap();
        let root = store.roots()[0].path.clone();
        store.set_root_note(&root, "原备注").unwrap();
        let saved = fs::read(&state).unwrap();
        let before = attributes::read(&state).unwrap();
        attributes::write(&state, before | attributes::READONLY).unwrap();
        let result = store.set_root_note(&root, "新备注");
        attributes::write(&state, before).unwrap();
        assert!(result.is_err());
        assert_eq!(store.root_note(&root), "原备注");
        assert_eq!(fs::read(&state).unwrap(), saved);
        assert_eq!(
            store.verify_root(&root).unwrap(),
            ProtectionCheck::Consistent
        );
        store.unprotect(&file).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn protection_rejections_have_specific_stable_categories() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state).unwrap();
        let missing = store
            .protect(&temp.path().join("missing.txt"), ProtectionMode::Hidden)
            .unwrap_err();
        assert_eq!(
            vault::error_code(&missing),
            vault::ErrorCode::ProtectionInput
        );
        let recovery_parent = store
            .protect(temp.path(), ProtectionMode::Hidden)
            .unwrap_err();
        assert_eq!(
            vault::error_code(&recovery_parent),
            vault::ErrorCode::ProtectionLocation
        );
        let file = temp.path().join("source.txt");
        fs::write(&file, b"keep").unwrap();
        fs::hard_link(&file, temp.path().join("alias.txt")).unwrap();
        let unsupported = store.protect(&file, ProtectionMode::Locked).unwrap_err();
        assert_eq!(
            vault::error_code(&unsupported),
            vault::ErrorCode::ProtectionUnsupported
        );
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        assert!(store.roots().is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn root_verification_checks_registered_entries_without_writes_or_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("资料");
        fs::create_dir(&root).unwrap();
        let file = root.join("private-child.txt");
        fs::write(&file, b"keep").unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&root, ProtectionMode::Hidden).unwrap();
        let key = store.roots()[0].path.clone();
        let saved = fs::read(&state).unwrap();
        fs::write(root.join("not-registered.txt"), b"new").unwrap();
        assert_eq!(
            store.verify_root(&key).unwrap(),
            ProtectionCheck::Consistent
        );
        assert_eq!(store.entries().count(), 2);
        let original = attributes::read(&file).unwrap();
        attributes::write(&file, original & !attributes::SYSTEM).unwrap();
        assert_eq!(store.verify_root(&key).unwrap(), ProtectionCheck::Changed);
        assert_eq!(
            attributes::read(&file).unwrap(),
            original & !attributes::SYSTEM
        );
        fs::remove_file(&file).unwrap();
        assert_eq!(
            store.verify_root(&key).unwrap(),
            ProtectionCheck::Unavailable
        );
        assert_eq!(fs::read(&state).unwrap(), saved);
        store.unprotect(&root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn root_verification_detects_acl_identity_and_parent_changes() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let file = parent.join("资料.txt");
        fs::write(&file, b"keep").unwrap();
        let state = temp.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&file, ProtectionMode::Locked).unwrap();
        let root = store.roots()[0].path.clone();
        let record = store.entries().next().unwrap().lock.clone().unwrap();
        let consistent = store.verify_root(&root).unwrap();
        let parent_object = acl::Object::open(&parent).unwrap();
        let parent_record = parent_object.prepare().unwrap();
        parent_object.write(&parent_record.locked).unwrap();
        let changed_parent = store.verify_root(&root).unwrap();
        parent_object.write(&parent_record.original).unwrap();
        let object = acl::Object::open(&file).unwrap();
        object.write(&record.original).unwrap();
        drop(object);
        let saved = fs::read(&state).unwrap();
        let changed_acl = store.verify_root(&root).unwrap();
        fs::remove_file(&file).unwrap();
        fs::write(&file, b"replacement").unwrap();
        let changed_identity = store.verify_root(&root).unwrap();
        assert_eq!(consistent, ProtectionCheck::Consistent);
        assert_eq!(changed_parent, ProtectionCheck::Changed);
        assert_eq!(changed_acl, ProtectionCheck::Changed);
        assert_eq!(changed_identity, ProtectionCheck::Changed);
        assert_eq!(fs::read(&state).unwrap(), saved);
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
    }

    #[test]
    #[cfg(windows)]
    fn lock_blocks_content_and_enumeration_and_restores_tree_after_restart() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("中文资料");
        fs::create_dir_all(root.join("子目录")).unwrap();
        let file = root.join("子目录").join("文档.txt");
        fs::write(&file, b"unchanged content").unwrap();
        let before: Vec<_> = targets(&root)
            .unwrap()
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    acl::Object::open(path).unwrap().read().unwrap(),
                )
            })
            .collect();
        let mut existing_handle = File::open(&file).unwrap();
        let state = dir.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&root, ProtectionMode::HiddenLocked).unwrap();
        let blocked = (
            fs::read(&file).is_err(),
            fs::write(&file, b"overwritten").is_err(),
            fs::read_dir(&root).is_err(),
            fs::write(root.join("new.txt"), b"new").is_err(),
        );
        let statuses = store.statuses(None, 0, 100).unwrap();
        let mut old_content = Vec::new();
        existing_handle.read_to_end(&mut old_content).unwrap();
        drop(existing_handle);
        drop(store);
        let mut store = StateStore::load_from(state).unwrap();
        store.unprotect(&root).unwrap();
        assert_eq!(blocked, (true, true, true, true));
        assert!(
            statuses
                .iter()
                .all(|status| status.state == AttributeState::Consistent)
        );
        assert_eq!(old_content, b"unchanged content");
        assert_eq!(fs::read(&file).unwrap(), b"unchanged content");
        for (path, original) in before {
            assert!(acl::equivalent(
                &acl::Object::open(&path).unwrap().read().unwrap(),
                &original
            ));
        }
        assert_eq!(store.entries().count(), 0);
    }

    #[test]
    #[cfg(windows)]
    fn lock_policy_switches_preserve_original_attributes_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("folder");
        fs::create_dir(&root).unwrap();
        let source = root.join("file.txt");
        fs::write(&source, b"keep").unwrap();
        let original = acl::Object::open(&source).unwrap().read().unwrap();
        let attrs = attributes::read(&source).unwrap();
        let mut store = StateStore::load_from(dir.path().join("state.json")).unwrap();
        for mode in [
            ProtectionMode::HiddenLocked,
            ProtectionMode::Locked,
            ProtectionMode::Hidden,
            ProtectionMode::Readonly,
            ProtectionMode::HiddenLocked,
            ProtectionMode::None,
        ] {
            store.protect(&root, mode).unwrap();
            assert_eq!(fs::read(&source).is_err(), mode.is_locked());
        }
        assert_eq!(attributes::read(&source).unwrap(), attrs);
        assert!(acl::equivalent(
            &acl::Object::open(&source).unwrap().read().unwrap(),
            &original
        ));
        assert!(store.roots().is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn failed_save_never_applies_lock() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.txt");
        fs::write(&source, b"keep").unwrap();
        let original = acl::Object::open(&source).unwrap().read().unwrap();
        let state = dir.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.save().unwrap();
        let attrs = attributes::read(&state).unwrap();
        attributes::write(&state, attrs | attributes::READONLY).unwrap();
        let result = store.protect(&source, ProtectionMode::HiddenLocked);
        attributes::write(&state, attrs).unwrap();
        assert!(result.is_err());
        assert!(store.entries().next().is_none());
        assert_eq!(fs::read(&source).unwrap(), b"keep");
        assert!(acl::equivalent(
            &acl::Object::open(&source).unwrap().read().unwrap(),
            &original
        ));
    }

    #[test]
    #[cfg(windows)]
    fn recovery_refuses_external_acl_changes_and_replaced_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.txt");
        fs::write(&source, b"keep").unwrap();
        let mut store = StateStore::load_from(dir.path().join("state.json")).unwrap();
        store.protect(&source, ProtectionMode::Locked).unwrap();
        let record = store.entries().next().unwrap().lock.clone().unwrap();
        {
            let object = acl::Object::open(&source).unwrap();
            let external = record.locked.replacen("(D;", "(A;", 1);
            object.write(&external).unwrap();
            assert!(store.unprotect(&source).is_err());
            assert!(acl::equivalent(&object.read().unwrap(), &external));
            object.write(&record.original).unwrap();
        }
        fs::remove_file(&source).unwrap();
        fs::write(&source, b"replacement").unwrap();
        assert!(store.unprotect(&source).is_err());
        assert_eq!(store.entries().count(), 1);
        assert_eq!(fs::read(&source).unwrap(), b"replacement");
    }

    #[test]
    #[cfg(windows)]
    fn interrupted_lock_plan_can_restore_without_completed_writes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.txt");
        fs::write(&source, b"keep").unwrap();
        let state = dir.path().join("state.json");
        let mut store = StateStore::load_from(state.clone()).unwrap();
        store.protect(&source, ProtectionMode::Locked).unwrap();
        let record = store.entries().next().unwrap().lock.clone().unwrap();
        acl::Object::open(&source)
            .unwrap()
            .write(&record.original)
            .unwrap();
        drop(store);
        let mut store = StateStore::load_from(state).unwrap();
        store.unprotect(&source).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"keep");
        assert!(store.roots().is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn lock_recovery_checks_parent_inheritance_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let source = parent.join("file.txt");
        fs::write(&source, b"keep").unwrap();
        let mut store = StateStore::load_from(dir.path().join("state.json")).unwrap();
        store.protect(&source, ProtectionMode::Locked).unwrap();
        let parent_object = acl::Object::open(&parent).unwrap();
        let parent_record = parent_object.prepare().unwrap();
        parent_object.write(&parent_record.locked).unwrap();
        assert!(store.unprotect(&source).is_err());
        parent_object.write(&parent_record.original).unwrap();
        store.unprotect(&source).unwrap();
        store.protect(&source, ProtectionMode::Locked).unwrap();
        let record = store.entries().next().unwrap().lock.clone().unwrap();
        acl::Object::open(&source)
            .unwrap()
            .write(&record.original)
            .unwrap();
        fs::remove_file(&source).unwrap();
        assert!(store.unprotect(&source).is_err());
        assert_eq!(store.entries().count(), 1);
    }

    #[test]
    #[cfg(windows)]
    fn lock_expanding_parent_covers_siblings_and_refuses_partial_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("a.txt"), b"a").unwrap();
        fs::write(second.join("b.txt"), b"b").unwrap();
        let mut store = StateStore::load_from(dir.path().join("state.json")).unwrap();
        store.protect(&first, ProtectionMode::Locked).unwrap();
        store.protect(&root, ProtectionMode::HiddenLocked).unwrap();
        assert!(store.unprotect(&first).is_err());
        let blocked = fs::read(second.join("b.txt")).is_err();
        store.unprotect(&root).unwrap();
        assert!(blocked);
        assert_eq!(fs::read(first.join("a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(second.join("b.txt")).unwrap(), b"b");
    }

    #[test]
    #[cfg(windows)]
    fn lock_rejects_hardlinks_before_any_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.txt");
        fs::write(&source, b"keep").unwrap();
        fs::hard_link(&source, dir.path().join("alias.txt")).unwrap();
        let mut store = StateStore::load_from(dir.path().join("state.json")).unwrap();
        assert!(
            store
                .protect(&source, ProtectionMode::HiddenLocked)
                .is_err()
        );
        assert_eq!(fs::read(&source).unwrap(), b"keep");
        assert!(store.roots().is_empty());
        assert_eq!(attributes::read(&source).unwrap() & MANAGED_ATTRIBUTES, 0);
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
                lock: None,
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
