//! 服务独占的受管访问状态；调用方负责通过 Windows 令牌验证目标 SID。
use crate::{ProtectionMode, acl, attributes, comparable_path, is_same_or_child, path_key, vault};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;
use zeroize::Zeroizing;

pub const DEFAULT_UNLOCK_SECONDS: u64 = 300;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const CREDENTIAL_CONTEXT: &[u8] = b"FoldSentinel managed access credential v1";

/// 只能由服务模拟客户端时实际打开的对象生成，不能直接接收客户端 JSON 清单。
#[derive(Debug, Clone)]
pub struct AuthorizedPath {
    pub path: PathBuf,
    pub identity: [u64; 3],
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    salt: [u8; 16],
    nonce: [u8; 12],
    verifier: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    original_attributes: u32,
    plan: acl::LockRecord,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Root {
    id: String,
    path: String,
    mode: ProtectionMode,
    note: String,
    credential: Option<Credential>,
    monitoring: bool,
    unlocked_until: Option<u64>,
    #[serde(default)]
    unlocked_at: Option<u64>,
    #[serde(default)]
    opening: bool,
    restoring: bool,
    last_error: Option<vault::ErrorCode>,
    entries: BTreeMap<String, Entry>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    user_sid: String,
    roots: BTreeMap<String, Root>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagedRootView {
    pub id: String,
    pub name: String,
    pub note: String,
    pub mode: ProtectionMode,
    pub password_required: bool,
    pub monitoring: bool,
    pub unlocked_until: Option<u64>,
    pub restoring: bool,
    pub last_error: Option<vault::ErrorCode>,
    pub entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootOutcome {
    pub id: String,
    pub error: Option<vault::ErrorCode>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SweepReport {
    pub succeeded: usize,
    pub failed: usize,
    pub outcomes: Vec<RootOutcome>,
}

pub struct ManagedStore {
    path: PathBuf,
    data: State,
    _lock: File,
}

pub fn current_user_sid() -> Result<String> {
    acl::current_sid()
}

impl ManagedStore {
    pub fn load(path: PathBuf, user_sid: &str) -> Result<Self> {
        validate_sid(user_sid)?;
        let parent = path.parent().context("受管状态缺少父目录")?;
        fs::create_dir_all(parent)?;
        let path = fs::canonicalize(parent)?.join(path.file_name().context("受管状态缺少文件名")?);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock().context(vault::ErrorCode::Busy)?;
        let data = match File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_STATE_BYTES {
                    bail!(vault::ErrorCode::ProtectionInput);
                }
                let state: State =
                    serde_json::from_slice(&bytes).context("受管状态无效，原文件已保留")?;
                validate_state(&state, user_sid)?;
                state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State {
                version: 1,
                user_sid: user_sid.to_owned(),
                roots: BTreeMap::new(),
            },
            Err(error) => return Err(error).context("读取受管状态失败"),
        };
        Ok(Self {
            path,
            data,
            _lock: lock,
        })
    }

    pub fn list(&self) -> Vec<ManagedRootView> {
        self.data.roots.values().map(view).collect()
    }
    pub fn registered_paths(&self) -> Vec<PathBuf> {
        self.data
            .roots
            .values()
            .map(|root| PathBuf::from(&root.path))
            .collect()
    }

    pub fn enroll_authorized(
        &mut self,
        root: &Path,
        mode: ProtectionMode,
        note: &str,
        password: Option<&str>,
        authorized: &[AuthorizedPath],
    ) -> Result<ManagedRootView> {
        validate_mode(mode)?;
        let note = crate::validated_note(note)?.to_owned();
        let credential = password.map(make_credential).transpose()?;
        let root = crate::canonical_root(root)?;
        crate::validate_lock_root(&root)?;
        let root_path = path_key(&root);
        if is_same_or_child(&path_key(&self.path), &root_path)
            || self.data.roots.values().any(|other| {
                is_same_or_child(&root_path, &other.path)
                    || is_same_or_child(&other.path, &root_path)
            })
        {
            bail!(vault::ErrorCode::ProtectionConflict);
        }
        if authorized.is_empty() || authorized.len() > MAX_ENTRIES {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        let mut entries = BTreeMap::new();
        let mut held = Vec::new();
        let mut paths = authorized.to_vec();
        paths.sort_by_key(|entry| entry.path.components().count());
        for authorized in paths {
            let path = path_key(&authorized.path);
            if !is_same_or_child(&path, &root_path) {
                bail!(vault::ErrorCode::ProtectionConflict);
            }
            let object = checked_object(&path, &root_path, Some(authorized.identity))?;
            let original_attributes = object.attributes()?;
            let plan = object
                .prepare_for_user(mode == ProtectionMode::ReadonlyAcl, &self.data.user_sid)?;
            if entries
                .insert(
                    path.clone(),
                    Entry {
                        path,
                        original_attributes,
                        plan,
                    },
                )
                .is_some()
            {
                bail!(vault::ErrorCode::ProtectionInput);
            }
            held.push(object);
        }
        if !entries.contains_key(&root_path) {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        let id: String = random_bytes::<16>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let managed = Root {
            id: id.clone(),
            path: root_path,
            mode,
            note,
            credential,
            monitoring: true,
            unlocked_until: None,
            unlocked_at: None,
            opening: false,
            restoring: false,
            last_error: None,
            entries,
        };
        let mut next = self.data.clone();
        next.roots.insert(id.clone(), managed);
        self.commit(next)?;
        let result = self.apply(&id, false, false);
        self.finish(&id, result)?;
        Ok(view(&self.data.roots[&id]))
    }

    pub fn unlock(&mut self, id: &str, password: &str, duration_secs: u64, now: u64) -> Result<()> {
        if !(30..=3600).contains(&duration_secs) {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        let root = self.root(id)?;
        if root.restoring {
            bail!(vault::ErrorCode::ProtectionConflict);
        }
        let credential = root
            .credential
            .as_ref()
            .context(vault::ErrorCode::Authentication)?;
        verify_credential(credential, password)?;
        let deadline = now
            .checked_add(duration_secs)
            .context(vault::ErrorCode::ProtectionInput)?;
        let mut next = self.data.clone();
        next.roots.get_mut(id).expect("known root").unlocked_until = Some(deadline);
        next.roots.get_mut(id).expect("known root").unlocked_at = Some(now);
        next.roots.get_mut(id).expect("known root").opening = true;
        next.roots.get_mut(id).expect("known root").last_error = None;
        self.commit(next)?;
        let result = self.apply(id, true, false);
        if result.is_err() {
            self.relock(id)
                .context("临时访问失败，重新锁定未完整完成；恢复计划已保留")?;
        } else {
            let mut next = self.data.clone();
            next.roots.get_mut(id).expect("known root").opening = false;
            self.commit(next)?;
        }
        self.finish(id, result)
    }

    pub fn relock(&mut self, id: &str) -> Result<()> {
        let _ = self.root(id)?;
        let mut next = self.data.clone();
        next.roots.get_mut(id).expect("known root").unlocked_until = Some(0);
        if let Err(error) = self.commit(next) {
            // 即使日志此刻不可写，既有完整重锁计划也足以先关闭已登记内容的临时访问。
            self.apply(id, false, true)
                .context("状态保存失败且重新锁定未完成，原计划已保留")?;
            return Err(error);
        }
        let discovery = self.discover(id);
        let result = self.apply(id, false, true);
        self.finish(id, result)?;
        self.finish(id, discovery)?;
        let mut next = self.data.clone();
        next.roots.get_mut(id).expect("known root").unlocked_until = None;
        next.roots.get_mut(id).expect("known root").unlocked_at = None;
        next.roots.get_mut(id).expect("known root").opening = false;
        self.commit(next)
    }

    pub fn change_password(
        &mut self,
        id: &str,
        current: &str,
        next_password: Option<&str>,
    ) -> Result<()> {
        authenticate(self.root(id)?, Some(current))?;
        let credential = next_password.map(make_credential).transpose()?;
        if credential.is_none() {
            self.relock(id)?;
        }
        let mut next = self.data.clone();
        next.roots.get_mut(id).expect("known root").credential = credential;
        self.commit(next)
    }

    pub fn set_monitoring(
        &mut self,
        id: &str,
        enabled: bool,
        password: Option<&str>,
    ) -> Result<()> {
        authenticate(self.root(id)?, password)?;
        let mut next = self.data.clone();
        next.roots.get_mut(id).expect("known root").monitoring = enabled;
        self.commit(next)
    }

    pub fn restore(&mut self, id: &str, password: Option<&str>) -> Result<()> {
        authenticate(self.root(id)?, password)?;
        let mut next = self.data.clone();
        let root = next.roots.get_mut(id).expect("known root");
        root.restoring = true;
        root.unlocked_until = None;
        root.unlocked_at = None;
        root.opening = false;
        self.commit(next)?;
        let result = self.apply(id, true, false);
        self.finish(id, result)?;
        let mut next = self.data.clone();
        next.roots.remove(id);
        self.commit(next)
    }

    pub fn tick(&mut self, now: u64) -> SweepReport {
        self.sweep(|store, id| {
            let root = store.root(id)?;
            if root.restoring {
                return store.continue_restore(id);
            }
            if root.opening
                || root.unlocked_until.is_some_and(|deadline| deadline <= now)
                || root.unlocked_at.is_some_and(|issued| now < issued)
            {
                return store.relock(id);
            }
            if root.unlocked_until.is_some() || !root.monitoring {
                return Ok(());
            }
            store.discover(id)?;
            let result = store.apply(id, false, true);
            store.finish(id, result)
        })
    }

    pub fn recover_startup(&mut self) -> SweepReport {
        self.sweep(|store, id| {
            if store.root(id)?.restoring {
                return store.continue_restore(id);
            }
            if store.root(id)?.unlocked_until.is_some() || store.root(id)?.monitoring {
                store.relock(id)
            } else {
                Ok(())
            }
        })
    }

    pub fn prepare_stop(&mut self) -> SweepReport {
        self.sweep(|store, id| {
            if store.root(id)?.restoring {
                return store.continue_restore(id);
            }
            if store.root(id)?.unlocked_until.is_some() {
                store.relock(id)
            } else {
                Ok(())
            }
        })
    }

    fn continue_restore(&mut self, id: &str) -> Result<()> {
        let result = self.apply(id, true, false);
        self.finish(id, result)?;
        let mut next = self.data.clone();
        next.roots.remove(id);
        self.commit(next)
    }

    fn root(&self, id: &str) -> Result<&Root> {
        self.data
            .roots
            .get(id)
            .context(vault::ErrorCode::ProtectionRecordChanged)
    }

    fn commit(&mut self, next: State) -> Result<()> {
        let bytes = Zeroizing::new(serde_json::to_vec(&next)?);
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.path.parent().context("受管状态缺少父目录")?)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        self.data = next;
        Ok(())
    }

    fn finish(&mut self, id: &str, result: Result<()>) -> Result<()> {
        let code = result.as_ref().err().map(vault::error_code);
        if self.root(id)?.last_error != code {
            let mut next = self.data.clone();
            next.roots.get_mut(id).expect("known root").last_error = code;
            self.commit(next)?;
        }
        result
    }

    fn sweep(&mut self, mut action: impl FnMut(&mut Self, &str) -> Result<()>) -> SweepReport {
        let ids: Vec<_> = self.data.roots.keys().cloned().collect();
        let mut report = SweepReport::default();
        for id in ids {
            let result = action(self, &id);
            let result = if self.data.roots.contains_key(&id) {
                self.finish(&id, result)
            } else {
                result
            };
            let error = result.as_ref().err().map(vault::error_code);
            if error.is_some() {
                report.failed += 1;
            } else {
                report.succeeded += 1;
            }
            report.outcomes.push(RootOutcome { id, error });
        }
        report
    }

    fn discover(&mut self, id: &str) -> Result<()> {
        let root = self.root(id)?.clone();
        let root_entry = root
            .entries
            .get(&root.path)
            .context(vault::ErrorCode::ProtectionConflict)?;
        let held_root = checked_object(&root.path, &root.path, Some(root_entry.plan.identity))?;
        verify_parent(&root)?;
        let mut next = self.data.clone();
        let mut held = vec![held_root];
        let mut changed = false;
        for entry in WalkDir::new(&root.path).follow_links(false) {
            let entry = entry.context(vault::ErrorCode::ReadFailed)?;
            let path = path_key(entry.path());
            let object = checked_object(
                &path,
                &root.path,
                root.entries.get(&path).map(|entry| entry.plan.identity),
            )?;
            if !root.entries.contains_key(&path) {
                let plan = object.prepare_for_user(
                    root.mode == ProtectionMode::ReadonlyAcl,
                    &self.data.user_sid,
                )?;
                let original_attributes = object.attributes()?;
                let target = next.roots.get_mut(id).expect("known root");
                if target.entries.len() >= MAX_ENTRIES {
                    bail!(vault::ErrorCode::ProtectionInput);
                }
                target.entries.insert(
                    path.clone(),
                    Entry {
                        path,
                        plan,
                        original_attributes,
                    },
                );
                changed = true;
            }
            held.push(object);
        }
        if changed {
            self.commit(next)?;
        }
        Ok(())
    }

    fn apply(&mut self, id: &str, open: bool, repair_drift: bool) -> Result<()> {
        let root = self.root(id)?;
        verify_parent(root)?;
        let mut entries: Vec<_> = root.entries.values().collect();
        entries.sort_by_key(|entry| Path::new(&entry.path).components().count());
        let mut held = Vec::new();
        // 全部对象先验证，避免发现后面的身份冲突前已改变前面的对象。
        for entry in &entries {
            let object = checked_object(&entry.path, &root.path, Some(entry.plan.identity))?;
            if !repair_drift {
                object.verify(&entry.plan)?;
            }
            held.push(object);
        }
        let mut targets: Vec<_> = entries.into_iter().zip(held).collect();
        if !open {
            targets.reverse();
        }
        for (entry, object) in targets {
            let desired = if open {
                &entry.plan.original
            } else {
                &entry.plan.locked
            };
            if !acl::equivalent(&object.read()?, desired) {
                object.write(desired)?;
            }
            let current = object.attributes()?;
            let mask = attributes::HIDDEN | attributes::SYSTEM;
            let expected = if open {
                entry.original_attributes
            } else {
                root.mode.apply(entry.original_attributes)
            };
            let desired_attributes = (current & !mask) | (expected & mask);
            if current != desired_attributes {
                object.write_attributes(desired_attributes)?;
            }
        }
        Ok(())
    }
}

fn view(root: &Root) -> ManagedRootView {
    ManagedRootView {
        id: root.id.clone(),
        name: Path::new(&root.path)
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        note: root.note.clone(),
        mode: root.mode,
        password_required: root.credential.is_some(),
        monitoring: root.monitoring,
        unlocked_until: root.unlocked_until,
        restoring: root.restoring,
        last_error: root.last_error,
        entries: root.entries.len(),
    }
}

fn validate_mode(mode: ProtectionMode) -> Result<()> {
    if !matches!(
        mode,
        ProtectionMode::Locked | ProtectionMode::HiddenLocked | ProtectionMode::ReadonlyAcl
    ) {
        bail!(vault::ErrorCode::ProtectionInput);
    }
    Ok(())
}

fn validate_sid(sid: &str) -> Result<()> {
    if sid.len() > 184
        || !sid.starts_with("S-1-")
        || sid[4..]
            .chars()
            .any(|character| !character.is_ascii_digit() && character != '-')
        || matches!(sid, "S-1-5-18" | "S-1-5-19" | "S-1-5-20")
    {
        bail!(vault::ErrorCode::ProtectionInput);
    }
    Ok(())
}

fn checked_object(path: &str, root: &str, identity: Option<[u64; 3]>) -> Result<acl::Object> {
    let object = acl::Object::open(Path::new(path))?;
    let resolved = path_key(&object.resolved_path()?);
    if !is_same_or_child(&resolved, root)
        || comparable_path(&resolved) != comparable_path(path)
        || identity.is_some_and(|identity| object.identity().ok() != Some(identity))
    {
        bail!(vault::ErrorCode::ProtectionConflict);
    }
    Ok(object)
}

fn verify_parent(root: &Root) -> Result<()> {
    let entry = root
        .entries
        .get(&root.path)
        .context(vault::ErrorCode::ProtectionConflict)?;
    if let Some(parent) = &entry.plan.parent
        && !acl::equivalent(&acl::Object::read_parent(Path::new(&root.path))?, parent)
    {
        bail!(vault::ErrorCode::ProtectionConflict);
    }
    Ok(())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn password_key(password: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    let params =
        Params::new(64 * 1024, 3, 1, Some(32)).map_err(|_| anyhow::anyhow!("密码派生参数无效"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| anyhow::anyhow!("密码验证资料生成失败"))?;
    Ok(key)
}

fn make_credential(password: &str) -> Result<Credential> {
    if password.chars().count() < 8 || password.len() > 1024 {
        bail!(vault::ErrorCode::ProtectionInput);
    }
    let salt = random_bytes();
    let nonce = random_bytes();
    let key = password_key(password, &salt)?;
    let verifier = Aes256Gcm::new_from_slice(key.as_ref())
        .expect("fixed key length")
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: CREDENTIAL_CONTEXT,
                aad: CREDENTIAL_CONTEXT,
            },
        )
        .map_err(|_| anyhow::anyhow!("密码验证资料加密失败"))?;
    Ok(Credential {
        salt,
        nonce,
        verifier,
    })
}

fn verify_credential(credential: &Credential, password: &str) -> Result<()> {
    if password.chars().count() < 8 || password.len() > 1024 {
        bail!(vault::ErrorCode::Authentication);
    }
    let key = password_key(password, &credential.salt)?;
    let result = Aes256Gcm::new_from_slice(key.as_ref())
        .expect("fixed key length")
        .decrypt(
            Nonce::from_slice(&credential.nonce),
            Payload {
                msg: &credential.verifier,
                aad: CREDENTIAL_CONTEXT,
            },
        );
    if !result.is_ok_and(|plaintext| plaintext == CREDENTIAL_CONTEXT) {
        bail!(vault::ErrorCode::Authentication);
    }
    Ok(())
}

fn authenticate(root: &Root, password: Option<&str>) -> Result<()> {
    if let Some(credential) = &root.credential {
        verify_credential(
            credential,
            password.context(vault::ErrorCode::Authentication)?,
        )?;
    }
    Ok(())
}

fn validate_state(state: &State, sid: &str) -> Result<()> {
    if state.version != 1 || state.user_sid != sid {
        bail!(vault::ErrorCode::ProtectionConflict);
    }
    let mut seen = BTreeSet::new();
    for (id, root) in &state.roots {
        validate_mode(root.mode)?;
        crate::validated_note(&root.note)?;
        if id != &root.id
            || id.len() != 32
            || !id.chars().all(|character| character.is_ascii_hexdigit())
            || !Path::new(&root.path).is_absolute()
            || !root.entries.contains_key(&root.path)
            || root.entries.len() > MAX_ENTRIES
        {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        if root
            .credential
            .as_ref()
            .is_some_and(|credential| credential.verifier.len() != CREDENTIAL_CONTEXT.len() + 16)
        {
            bail!(vault::ErrorCode::ProtectionInput);
        }
        for (path, entry) in &root.entries {
            if path != &entry.path
                || !is_same_or_child(path, &root.path)
                || entry.plan.version != 1
                || !seen.insert(comparable_path(path))
            {
                bail!(vault::ErrorCode::ProtectionConflict);
            }
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const PASSWORD: &str = "test-password-only";

    struct Fixture {
        // 先释放禁止共享删除的授权句柄，再由TempDir清理测试目录。
        _held: Vec<acl::Object>,
        _temp: tempfile::TempDir,
        state: PathBuf,
        root: PathBuf,
        file: PathBuf,
        authorized: Vec<AuthorizedPath>,
        original_file_acl: String,
        original_root_acl: String,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path().join("service-state").join("managed.json");
            let root = temp.path().join("files").join("资料");
            fs::create_dir_all(&root).unwrap();
            let file = root.join("内部原文件.txt");
            fs::write(&file, b"original").unwrap();
            let original_file_acl = acl::Object::open(&file).unwrap().read().unwrap();
            let original_root_acl = acl::Object::open(&root).unwrap().read().unwrap();
            let mut held = Vec::new();
            let mut authorized = Vec::new();
            for entry in WalkDir::new(&root) {
                let entry = entry.unwrap();
                let object = acl::Object::open(entry.path()).unwrap();
                authorized.push(AuthorizedPath {
                    path: object.resolved_path().unwrap(),
                    identity: object.identity().unwrap(),
                });
                held.push(object);
            }
            Self {
                _temp: temp,
                state,
                root,
                file,
                authorized,
                _held: held,
                original_file_acl,
                original_root_acl,
            }
        }
        fn store(&self) -> ManagedStore {
            ManagedStore::load(self.state.clone(), &current_user_sid().unwrap()).unwrap()
        }
        fn enroll(&self, store: &mut ManagedStore, password: Option<&str>) -> String {
            store
                .enroll_authorized(
                    &self.root,
                    ProtectionMode::ReadonlyAcl,
                    "测试备注",
                    password,
                    &self.authorized,
                )
                .unwrap()
                .id
        }
    }

    #[test]
    fn password_access_expires_and_registers_new_children_before_relocking() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let id = fixture.enroll(&mut store, Some(PASSWORD));
        assert_eq!(fs::read(&fixture.file).unwrap(), b"original");
        assert!(fs::write(&fixture.file, b"denied").is_err());
        assert!(fs::write(fixture.root.join("not-created.txt"), b"denied").is_err());
        let saved = fs::read(&fixture.state).unwrap();
        assert_eq!(
            vault::error_code(&store.unlock(&id, "wrong-password", 30, 1000).unwrap_err()),
            vault::ErrorCode::Authentication
        );
        assert_eq!(fs::read(&fixture.state).unwrap(), saved);
        assert!(fs::write(&fixture.file, b"still denied").is_err());
        store.unlock(&id, PASSWORD, 30, 1000).unwrap();
        fs::write(&fixture.file, b"edited while open").unwrap();
        let added = fixture.root.join("新文件.txt");
        fs::write(&added, b"new content").unwrap();
        assert_eq!(store.tick(1029).failed, 0);
        assert!(fs::write(&added, b"still open").is_ok());
        assert_eq!(store.list()[0].unlocked_until, Some(1030));
        let report = store.tick(1030);
        assert_eq!(report.failed, 0, "{report:?}");
        assert_eq!(store.list()[0].unlocked_until, None);
        assert_eq!(store.list()[0].entries, 3);
        assert!(fs::write(&fixture.file, b"denied").is_err());
        assert!(fs::write(&added, b"denied").is_err());
        let public = serde_json::to_string(&store.list()).unwrap();
        assert!(!public.contains("内部原文件"));
        assert!(!public.contains("service-state"));
        assert!(!public.contains("D:"));
        assert!(!String::from_utf8(saved).unwrap().contains(PASSWORD));
        store.restore(&id, Some(PASSWORD)).unwrap();
        assert!(store.list().is_empty());
        assert_eq!(fs::read(&fixture.file).unwrap(), b"edited while open");
        assert_eq!(fs::read(&added).unwrap(), b"still open");
        assert!(acl::equivalent(
            &acl::Object::open(&fixture.file).unwrap().read().unwrap(),
            &fixture.original_file_acl
        ));
        assert!(acl::equivalent(
            &acl::Object::open(&fixture.root).unwrap().read().unwrap(),
            &fixture.original_root_acl
        ));
    }

    #[test]
    fn hidden_locked_password_access_restores_content_and_original_attributes() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let original_attributes = attributes::read(&fixture.file).unwrap();
        let id = store
            .enroll_authorized(
                &fixture.root,
                ProtectionMode::HiddenLocked,
                "",
                Some(PASSWORD),
                &fixture.authorized,
            )
            .unwrap()
            .id;
        assert!(fs::read(&fixture.file).is_err());
        assert_ne!(
            attributes::read(&fixture.file).unwrap() & attributes::HIDDEN,
            0
        );
        store
            .unlock(&id, PASSWORD, DEFAULT_UNLOCK_SECONDS, 1000)
            .unwrap();
        assert_eq!(fs::read(&fixture.file).unwrap(), b"original");
        assert_eq!(
            attributes::read(&fixture.file).unwrap(),
            original_attributes
        );
        fs::write(&fixture.file, b"edited").unwrap();
        store.relock(&id).unwrap();
        assert!(fs::read(&fixture.file).is_err());
        assert_ne!(
            attributes::read(&fixture.file).unwrap() & attributes::HIDDEN,
            0
        );
        store.restore(&id, Some(PASSWORD)).unwrap();
        assert_eq!(fs::read(&fixture.file).unwrap(), b"edited");
        assert_eq!(
            attributes::read(&fixture.file).unwrap(),
            original_attributes
        );
    }

    #[test]
    fn restart_stop_and_clock_rollback_close_temporary_access() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let id = fixture.enroll(&mut store, Some(PASSWORD));
        store.unlock(&id, PASSWORD, 300, 1000).unwrap();
        assert!(fs::write(&fixture.file, b"open").is_ok());
        drop(store);
        let mut store = fixture.store();
        assert!(fs::write(&fixture.file, b"before startup recovery").is_ok());
        assert_eq!(store.recover_startup().failed, 0);
        assert!(fs::write(&fixture.file, b"closed after startup").is_err());
        store.unlock(&id, PASSWORD, 300, 2000).unwrap();
        assert_eq!(store.tick(1999).failed, 0);
        assert!(fs::write(&fixture.file, b"closed after rollback").is_err());
        store.unlock(&id, PASSWORD, 300, 3000).unwrap();
        assert_eq!(store.prepare_stop().failed, 0);
        assert!(fs::write(&fixture.file, b"closed before stop").is_err());
        store.restore(&id, Some(PASSWORD)).unwrap();
    }

    #[test]
    fn password_changes_and_monitoring_cannot_bypass_authentication() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let id = fixture.enroll(&mut store, Some(PASSWORD));
        for result in [
            store.restore(&id, None),
            store.set_monitoring(&id, false, Some("wrong-password")),
            store.change_password(&id, "wrong-password", None),
        ] {
            assert_eq!(
                vault::error_code(&result.unwrap_err()),
                vault::ErrorCode::Authentication
            );
        }
        assert!(store.list()[0].password_required);
        assert!(store.list()[0].monitoring);
        store.set_monitoring(&id, false, Some(PASSWORD)).unwrap();
        let drift = store.data.roots[&id].entries
            [&path_key(&fs::canonicalize(&fixture.file).unwrap())]
            .plan
            .locked
            .replacen("(D;", "(A;", 1);
        acl::Object::open(&fixture.file)
            .unwrap()
            .write(&drift)
            .unwrap();
        assert_eq!(store.tick(1000).failed, 0);
        assert!(fs::write(&fixture.file, b"monitor paused").is_ok());
        store.set_monitoring(&id, true, Some(PASSWORD)).unwrap();
        assert_eq!(store.tick(1001).failed, 0);
        assert!(fs::write(&fixture.file, b"drift repaired").is_err());
        store
            .change_password(&id, PASSWORD, Some("replacement-test-password"))
            .unwrap();
        assert!(store.unlock(&id, PASSWORD, 300, 2000).is_err());
        store
            .unlock(&id, "replacement-test-password", 300, 2000)
            .unwrap();
        store
            .change_password(&id, "replacement-test-password", None)
            .unwrap();
        assert!(!store.list()[0].password_required);
        assert!(fs::write(&fixture.file, b"password removed after relock").is_err());
        store.restore(&id, None).unwrap();
    }

    #[test]
    fn registration_and_unlock_commit_failures_never_open_files() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fs::create_dir(&fixture.state).unwrap();
        assert!(
            store
                .enroll_authorized(
                    &fixture.root,
                    ProtectionMode::ReadonlyAcl,
                    "备注",
                    None,
                    &fixture.authorized
                )
                .is_err()
        );
        assert!(store.list().is_empty());
        assert!(fs::write(&fixture.file, b"not protected").is_ok());
        fs::remove_dir(&fixture.state).unwrap();
        let id = fixture.enroll(&mut store, Some(PASSWORD));
        let attrs = attributes::read(&fixture.state).unwrap();
        attributes::write(&fixture.state, attrs | attributes::READONLY).unwrap();
        let result = store.unlock(&id, PASSWORD, 300, 1000);
        attributes::write(&fixture.state, attrs).unwrap();
        assert!(result.is_err());
        assert_eq!(store.list()[0].unlocked_until, None);
        assert!(fs::write(&fixture.file, b"not opened").is_err());
        store.restore(&id, Some(PASSWORD)).unwrap();
    }

    #[test]
    fn failed_relock_commit_closes_known_content_and_retries_discovery() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let id = fixture.enroll(&mut store, Some(PASSWORD));
        store.unlock(&id, PASSWORD, 300, 1000).unwrap();
        let added = fixture.root.join("临时访问时新增.txt");
        fs::write(&added, b"new").unwrap();
        let attrs = attributes::read(&fixture.state).unwrap();
        attributes::write(&fixture.state, attrs | attributes::READONLY).unwrap();
        let report = store.tick(1300);
        attributes::write(&fixture.state, attrs).unwrap();
        assert_eq!(report.failed, 1);
        assert!(fs::write(&fixture.file, b"existing entry must close").is_err());
        assert_eq!(store.list()[0].unlocked_until, Some(1300));
        let retry = store.relock(&id);
        assert!(retry.is_ok(), "{retry:?}");
        assert_eq!(store.list()[0].entries, 3);
        assert_eq!(store.list()[0].unlocked_until, None);
        assert!(fs::write(&added, b"new entry closed after retry").is_err());
        store.restore(&id, Some(PASSWORD)).unwrap();
        assert_eq!(fs::read(&added).unwrap(), b"new");
    }

    #[test]
    fn authorized_manifest_and_target_sid_are_enforced_before_mutation() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let mut wrong = fixture.authorized.clone();
        wrong[0].identity[1] ^= 1;
        assert!(
            store
                .enroll_authorized(&fixture.root, ProtectionMode::Locked, "", None, &wrong)
                .is_err()
        );
        let mut outside = fixture.authorized.clone();
        outside.push(AuthorizedPath {
            path: fixture.state.clone(),
            identity: [0; 3],
        });
        assert!(
            store
                .enroll_authorized(&fixture.root, ProtectionMode::Locked, "", None, &outside)
                .is_err()
        );
        assert!(store.list().is_empty());
        assert!(fs::write(&fixture.file, b"unmodified").is_ok());
        drop(store);
        let fake_sid = "S-1-5-21-111111111-222222222-333333333-1001";
        let mut store = ManagedStore::load(fixture.state.clone(), fake_sid).unwrap();
        let id = store
            .enroll_authorized(
                &fixture.root,
                ProtectionMode::Locked,
                "",
                None,
                &fixture.authorized,
            )
            .unwrap()
            .id;
        assert!(
            store.data.roots[&id]
                .entries
                .values()
                .all(|entry| entry.plan.locked.contains(fake_sid))
        );
        assert!(fs::write(&fixture.file, b"not the target user").is_ok());
        store.restore(&id, None).unwrap();
        drop(store);
        assert!(ManagedStore::load(fixture.state.clone(), &current_user_sid().unwrap()).is_err());
    }

    #[test]
    fn identity_and_parent_conflicts_preserve_records_and_do_not_touch_replacement() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let id = fixture.enroll(&mut store, None);
        let parent = acl::Object::open(fixture.root.parent().unwrap()).unwrap();
        let parent_plan = parent.prepare_readonly(true).unwrap();
        parent.write(&parent_plan.locked).unwrap();
        let report = store.tick(1000);
        parent.write(&parent_plan.original).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(
            store.list()[0].last_error,
            Some(vault::ErrorCode::ProtectionConflict)
        );
        assert_eq!(store.tick(1001).failed, 0);
        assert_eq!(store.list()[0].last_error, None);
        // 夹具登记时持有的授权句柄在真实服务请求返回后应释放；本测试显式释放后替换。
        drop(fixture._held);
        acl::Object::open(&fixture.root)
            .unwrap()
            .write(&fixture.original_root_acl)
            .unwrap();
        acl::Object::open(&fixture.file)
            .unwrap()
            .write(&fixture.original_file_acl)
            .unwrap();
        fs::remove_file(&fixture.file).unwrap();
        fs::write(&fixture.file, b"replacement").unwrap();
        let report = store.tick(1002);
        assert_eq!(report.failed, 1);
        assert_eq!(store.list()[0].id, id);
        assert_eq!(
            store.list()[0].last_error,
            Some(vault::ErrorCode::ProtectionConflict)
        );
        assert_eq!(fs::read(&fixture.file).unwrap(), b"replacement");
        assert!(fs::write(&fixture.file, b"replacement untouched").is_ok());
        assert!(store.restore(&id, None).is_err());
        assert_eq!(store.list()[0].id, id);
    }
}
