//! 加密资产登记独立于可清空的界面历史；未知位置不能被当成已解除风险。
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const INDEX_FILE: &str = "vault-inventory.json";
const INDEX_VERSION: u32 = 1;
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ASSETS: usize = 10_000;
const MAX_LOCATIONS: usize = 64;
const MAX_SCAN_ENTRIES: usize = 100_000;
const MAX_SCAN_ROOTS: usize = 100;
const MAX_SCAN_ISSUES: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetState {
    Pending,
    NeedsRestore,
    Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationState {
    Present,
    Missing,
    Unreadable,
    Changed,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocationSnapshot {
    pub path: PathBuf,
    pub state: LocationState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetSnapshot {
    pub fingerprint: String,
    pub state: AssetState,
    pub paths: Vec<LocationSnapshot>,
    pub restored_outputs: Vec<LocationSnapshot>,
}

impl AssetSnapshot {
    pub fn requires_restore(&self) -> bool {
        self.state != AssetState::Restored
            || !self
                .restored_outputs
                .iter()
                .any(|location| location.state == LocationState::Present)
    }

    pub fn has_unknown_locations(&self) -> bool {
        if self.state == AssetState::Restored
            && self
                .restored_outputs
                .iter()
                .any(|location| location.state == LocationState::Present)
        {
            return false;
        }
        self.state == AssetState::Pending
            || self
                .paths
                .iter()
                .any(|location| location.state != LocationState::Present)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryIssue {
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryReport {
    pub inspected_files: usize,
    pub discovered_packages: usize,
    pub scanned_paths: Vec<PathBuf>,
    pub issues: Vec<DiscoveryIssue>,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    state: AssetState,
    paths: BTreeSet<PathBuf>,
    restored_outputs: BTreeSet<PathBuf>,
    // 旧记录没有身份时保留路径，但不能据此证明当前仍是恢复输出。
    #[serde(default)]
    restored_identities: BTreeMap<PathBuf, RestoredIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RestoredIdentity {
    directory: bool,
    identity: [u64; 3],
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    assets: BTreeMap<String, Asset>,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            assets: BTreeMap::new(),
        }
    }
}

struct Store {
    path: PathBuf,
    index: Index,
    _lock: File,
}

impl Store {
    fn load(data_dir: &Path) -> Result<Self> {
        ensure!(data_dir.is_absolute(), "加密资产目录必须为绝对路径");
        fs::create_dir_all(data_dir).context("创建加密资产目录失败")?;
        super::reject_link(data_dir)?;
        let path = data_dir.join(INDEX_FILE);
        let lock_path = data_dir.join("vault-inventory.lock");
        match fs::symlink_metadata(&lock_path) {
            Ok(_) => super::reject_link(&lock_path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("检查加密资产锁失败"),
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .context("打开加密资产锁失败")?;
        lock.try_lock()
            .map_err(|error| anyhow::anyhow!("加密资产记录正在使用或无法锁定: {error}"))?;
        // 先检查目录项，避免断开的链接被 File::open 的 NotFound 当成“索引不存在”。
        let had_index = match fs::symlink_metadata(&path) {
            Ok(_) => {
                super::reject_link(&path)?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error).context("检查加密资产记录失败"),
        };
        let index = match File::open(&path) {
            Ok(file) => {
                // 再核对一次目录项，尽量发现打开前后被替换为重解析点的记录。
                super::reject_link(&path)?;
                ensure!(file.metadata()?.is_file(), "加密资产记录不是普通文件");
                let mut data = Vec::new();
                file.take(MAX_INDEX_BYTES + 1).read_to_end(&mut data)?;
                ensure!(
                    data.len() as u64 <= MAX_INDEX_BYTES,
                    "加密资产记录超过大小上限"
                );
                let index: Index =
                    serde_json::from_slice(&data).context("加密资产记录损坏，不能确认卸载安全")?;
                validate(&index)?;
                index
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !had_index => {
                Index::default()
            }
            Err(error) => return Err(error).context("读取加密资产记录失败"),
        };
        Ok(Self {
            path,
            index,
            _lock: lock,
        })
    }

    fn save(&self) -> Result<()> {
        validate(&self.index)?;
        let bytes = serde_json::to_vec(&self.index)?;
        ensure!(
            bytes.len() as u64 <= MAX_INDEX_BYTES,
            "加密资产记录超过大小上限"
        );
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.path.parent().context("加密资产目录缺失")?)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| anyhow::anyhow!("保存加密资产记录失败: {}", error.error))?;
        Ok(())
    }

    fn record(
        &mut self,
        fingerprint: String,
        path: PathBuf,
        state: AssetState,
        restored: Option<PathBuf>,
    ) -> Result<()> {
        let asset = self
            .index
            .assets
            .entry(fingerprint)
            .or_insert_with(|| Asset {
                state,
                paths: BTreeSet::new(),
                restored_outputs: BTreeSet::new(),
                restored_identities: BTreeMap::new(),
            });
        // 找到同一内容的新位置后，只去掉已证实不存在且父目录仍可读的旧位置。
        // 离线卷、拒绝访问和其他错误仍保留，留给卸载检查明确呈现。
        let mut relocated = Vec::new();
        for old in &asset.paths {
            if old == &path {
                continue;
            }
            if fs::symlink_metadata(old)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                && old
                    .parent()
                    .is_some_and(|parent| fs::read_dir(parent).is_ok())
            {
                relocated.push(old.clone());
            }
        }
        for old in relocated {
            asset.paths.remove(&old);
        }
        asset.paths.insert(path);
        if let Some(output) = restored {
            let identity = restored_identity(&output)?;
            asset.restored_outputs.insert(output.clone());
            asset.restored_identities.insert(output, identity);
            asset.state = AssetState::Restored;
        } else if asset.state != AssetState::Restored {
            asset.state = state;
        }
        validate(&self.index)
    }
}

fn validate(index: &Index) -> Result<()> {
    ensure!(
        index.version == INDEX_VERSION && index.assets.len() <= MAX_ASSETS,
        "加密资产记录版本或数量无效"
    );
    for (fingerprint, asset) in &index.assets {
        ensure!(
            fingerprint.len() == 64 && fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
            "加密资产指纹无效"
        );
        ensure!(
            !asset.paths.is_empty()
                && asset.paths.len() <= MAX_LOCATIONS
                && asset.restored_outputs.len() <= MAX_LOCATIONS
                && asset.restored_identities.len() <= MAX_LOCATIONS,
            "加密资产位置数量无效"
        );
        ensure!(
            asset
                .paths
                .iter()
                .chain(&asset.restored_outputs)
                .all(|path| path.is_absolute()),
            "加密资产位置不是绝对路径"
        );
        ensure!(
            asset.state != AssetState::Restored || !asset.restored_outputs.is_empty(),
            "已恢复资产缺少恢复位置"
        );
        ensure!(
            asset
                .restored_identities
                .keys()
                .all(|path| asset.restored_outputs.contains(path)),
            "恢复身份包含未登记的位置"
        );
    }
    Ok(())
}

fn read_fingerprint(path: &Path) -> Result<Option<String>> {
    super::reject_link(path)?;
    let mut file = super::open_stable(path)?;
    ensure!(file.metadata()?.is_file(), "加密包不是普通文件");
    let mut magic = [0; 8];
    if let Err(error) = file.read_exact(&mut magic) {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(error).context("读取加密包标识失败");
    }
    let length = match &magic {
        b"FSEAL001" => 38,
        b"FSEAL002" => 46,
        _ => return Ok(None),
    };
    let mut header = vec![0; length];
    header[..8].copy_from_slice(&magic);
    file.read_exact(&mut header[8..])
        .context("加密包头部不完整")?;
    ensure!(
        header[8] == if length == 38 { 1 } else { 2 } && header[9] <= 1,
        "加密包头部无效"
    );
    if length == 46 {
        ensure!(header[33] == 20, "加密包分块格式无效");
    }
    Ok(Some(format!("{:x}", Sha256::digest(header))))
}

fn required_fingerprint(path: &Path) -> Result<String> {
    read_fingerprint(path)?.context("不是可识别的 FoldSentinel 加密包")
}

pub(crate) fn record_pending(data_dir: &Path, payload: &Path, output: &Path) -> Result<String> {
    let fingerprint = required_fingerprint(payload)?;
    let mut store = Store::load(data_dir)?;
    ensure!(
        !store.index.assets.contains_key(&fingerprint),
        "本次加密包随机标识已存在，拒绝提交以避免资产状态冲突"
    );
    store.record(
        fingerprint.clone(),
        output.to_path_buf(),
        AssetState::Pending,
        None,
    )?;
    store.save()?;
    Ok(fingerprint)
}

pub(crate) fn mark_encrypted(data_dir: &Path, fingerprint: &str, output: &Path) -> Result<()> {
    let mut store = Store::load(data_dir)?;
    let asset = store
        .index
        .assets
        .get(fingerprint)
        .context("加密提交缺少待提交资产登记")?;
    ensure!(
        asset.state == AssetState::Pending
            && asset.paths.len() == 1
            && asset.paths.contains(output)
            && asset.restored_outputs.is_empty(),
        "加密提交资产登记状态不一致，保留待检查记录"
    );
    store.record(
        fingerprint.to_string(),
        output.to_path_buf(),
        AssetState::NeedsRestore,
        None,
    )?;
    store.save()
}

/// 仅撤销本次新加密、目标明确不存在的 pending 登记。
/// 输出存在、访问失败或记录不一致时保留，不能据此放行卸载。
pub(crate) fn rollback_pending(data_dir: &Path, fingerprint: &str, output: &Path) -> Result<()> {
    let mut store = Store::load(data_dir)?;
    let asset = store
        .index
        .assets
        .get(fingerprint)
        .context("缺少待提交资产登记")?;
    ensure!(
        asset.state == AssetState::Pending
            && asset.paths.len() == 1
            && asset.paths.contains(output)
            && asset.restored_outputs.is_empty(),
        "待提交资产登记状态不一致，保留记录"
    );
    match fs::symlink_metadata(output) {
        Ok(_) => bail!("待提交输出已存在，保留资产登记"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = output.parent().context("待提交输出缺少父目录")?;
            fs::read_dir(parent).context("无法确认待提交输出所在位置，保留资产登记")?;
        }
        Err(error) => return Err(error).context("无法确认待提交输出状态，保留资产登记"),
    }
    store.index.assets.remove(fingerprint);
    store.save()
}

pub(crate) fn observe(data_dir: &Path, path: &Path) -> Result<String> {
    let path = fs::canonicalize(path).context("解析加密包位置失败")?;
    let fingerprint = required_fingerprint(&path)?;
    let mut store = Store::load(data_dir)?;
    store.record(fingerprint.clone(), path, AssetState::NeedsRestore, None)?;
    store.save()?;
    Ok(fingerprint)
}

pub(crate) fn mark_restored(
    data_dir: &Path,
    fingerprint: &str,
    input: &Path,
    output: &Path,
) -> Result<()> {
    let input = fs::canonicalize(input).context("解析已恢复加密包位置失败")?;
    let mut store = Store::load(data_dir)?;
    let asset = store
        .index
        .assets
        .get(fingerprint)
        .context("恢复提交缺少加密资产登记")?;
    ensure!(
        asset.paths.contains(&input),
        "恢复提交的加密包位置未登记，保留待检查记录"
    );
    store.record(
        fingerprint.to_string(),
        input,
        AssetState::Restored,
        Some(output.to_path_buf()),
    )?;
    store.save()
}

fn restored_identity(path: &Path) -> Result<RestoredIdentity> {
    super::reject_link(path)?;
    let mut options = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        // 仅取元数据；不要求 WRITE_DAC、写权限或 NTFS，且持有期间拒绝替换。
        options
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(windows))]
    options.read(true);
    let file = options.open(path).context("打开恢复输出身份失败")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() || metadata.is_dir(),
        "恢复输出不是普通文件或目录"
    );
    #[cfg(windows)]
    let identity = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, GetFileInformationByHandle,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0,
            std::io::Error::last_os_error()
        );
        ensure!(
            info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "恢复输出是重解析点"
        );
        [
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
                | u64::from(info.ftCreationTime.dwLowDateTime),
        ]
    };
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        [metadata.dev(), metadata.ino(), 0]
    };
    #[cfg(not(any(windows, unix)))]
    let identity = bail!("当前平台不能核验恢复输出身份");
    Ok(RestoredIdentity {
        directory: metadata.is_dir(),
        identity,
    })
}

enum ExpectedLocation<'a> {
    Package(&'a str),
    Restored(Option<&'a RestoredIdentity>),
}

fn inspect_location(path: &Path, expected: ExpectedLocation<'_>) -> LocationSnapshot {
    let result = (|| -> Result<LocationState> {
        fs::symlink_metadata(path)?;
        super::reject_link(path)?;
        match expected {
            ExpectedLocation::Package(fingerprint) => {
                Ok(if read_fingerprint(path)?.as_deref() == Some(fingerprint) {
                    LocationState::Present
                } else {
                    LocationState::Changed
                })
            }
            ExpectedLocation::Restored(identity) => {
                let Some(expected) = identity else {
                    return Ok(LocationState::Changed);
                };
                let actual = restored_identity(path)?;
                Ok(if &actual == expected {
                    LocationState::Present
                } else {
                    LocationState::Changed
                })
            }
        }
    })();
    match result {
        Ok(state) => LocationSnapshot {
            path: path.to_path_buf(),
            state,
            detail: None,
        },
        Err(error) => {
            let missing = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            LocationSnapshot {
                path: path.to_path_buf(),
                state: if missing {
                    LocationState::Missing
                } else {
                    LocationState::Unreadable
                },
                detail: Some(format!("{error:#}")),
            }
        }
    }
}

/// 只读检查每条登记的当前位置；没有访问权或位置丢失不会被过滤掉。
pub fn snapshot(data_dir: &Path) -> Result<Vec<AssetSnapshot>> {
    let index_path = data_dir.join(INDEX_FILE);
    match fs::symlink_metadata(&index_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("加密资产记录不存在，无法确认卸载安全")
        }
        Err(error) => return Err(error).context("读取加密资产记录位置失败，无法确认卸载安全"),
    }
    let store = Store::load(data_dir)?;
    Ok(store
        .index
        .assets
        .into_iter()
        .map(|(fingerprint, asset)| AssetSnapshot {
            paths: asset
                .paths
                .iter()
                .map(|path| inspect_location(path, ExpectedLocation::Package(&fingerprint)))
                .collect(),
            restored_outputs: asset
                .restored_outputs
                .iter()
                .map(|path| {
                    inspect_location(
                        path,
                        ExpectedLocation::Restored(asset.restored_identities.get(path)),
                    )
                })
                .collect(),
            fingerprint,
            state: asset.state,
        })
        .collect())
}

/// 扫描调用者明确提供的范围；内容头识别不依赖扩展名，任何跳过或错误均保留。
pub fn discover(paths: &[PathBuf], data_dir: &Path) -> Result<DiscoveryReport> {
    ensure!(
        !paths.is_empty() && paths.len() <= MAX_SCAN_ROOTS,
        "旧包扫描范围数量无效"
    );
    ensure!(
        paths.iter().all(|path| path.is_absolute()),
        "旧包扫描范围必须为绝对路径"
    );
    let mut store = Store::load(data_dir)?;
    let mut report = DiscoveryReport {
        inspected_files: 0,
        discovered_packages: 0,
        scanned_paths: paths.to_vec(),
        issues: Vec::new(),
        complete: true,
    };
    let mut seen = BTreeSet::new();
    let mut seen_packages = BTreeSet::new();
    for root in paths {
        for entry in WalkDir::new(root).follow_links(false).max_depth(64) {
            ensure!(
                seen.len() < MAX_SCAN_ENTRIES,
                "旧包扫描项目超过上限，扫描未完成"
            );
            let mut issue_path = root.clone();
            let result = (|| -> Result<()> {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        if let Some(path) = error.path() {
                            issue_path = path.to_path_buf();
                        }
                        return Err(error.into());
                    }
                };
                let path = entry.path();
                issue_path = path.to_path_buf();
                if !seen.insert(path.to_path_buf()) {
                    return Ok(());
                }
                super::reject_link(path)?;
                if entry.file_type().is_dir() {
                    ensure!(entry.depth() < 64, "目录达到扫描深度上限，不能确认其下内容");
                    return Ok(());
                }
                if !entry.file_type().is_file() {
                    bail!("不是普通文件，未检查内容");
                }
                report.inspected_files += 1;
                let canonical = fs::canonicalize(path)?;
                if let Some(fingerprint) = read_fingerprint(&canonical)? {
                    store.record(
                        fingerprint.clone(),
                        canonical,
                        AssetState::NeedsRestore,
                        None,
                    )?;
                    seen_packages.insert(fingerprint);
                }
                Ok(())
            })();
            if let Err(error) = result {
                ensure!(
                    report.issues.len() < MAX_SCAN_ISSUES,
                    "旧包扫描错误过多，扫描未完成"
                );
                report.complete = false;
                report.issues.push(DiscoveryIssue {
                    path: issue_path,
                    detail: format!("{error:#}"),
                });
            }
        }
    }
    report.discovered_packages = seen_packages.len();
    store.save()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(path: &Path, magic: &[u8; 8]) {
        let mut header = vec![0u8; if magic == b"FSEAL001" { 38 } else { 46 }];
        header[..8].copy_from_slice(magic);
        header[8] = if magic == b"FSEAL001" { 1 } else { 2 };
        header[9] = 0;
        if header.len() == 46 {
            header[33] = 20;
        }
        let mut file = File::create(path).unwrap();
        file.write_all(&header).unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn discovery_is_extension_independent_and_missing_is_visible() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let package_path = root.path().join("renamed.bin");
        package(&package_path, b"FSEAL002");
        let report = discover(&[root.path().to_path_buf()], &data).unwrap();
        assert_eq!(report.discovered_packages, 1);
        let assets = snapshot(&data).unwrap();
        assert_eq!(assets.len(), 1);
        assert!(assets[0].requires_restore());
        fs::remove_file(&package_path).unwrap();
        let assets = snapshot(&data).unwrap();
        assert_eq!(assets[0].paths[0].state, LocationState::Missing);
        assert!(assets[0].has_unknown_locations());
    }

    #[test]
    fn restored_output_removes_restore_requirement_but_keeps_original_location() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let package_path = root.path().join("package.fse");
        let output = root.path().join("restored");
        package(&package_path, b"FSEAL001");
        fs::write(&output, b"restored").unwrap();
        let fingerprint = observe(&data, &package_path).unwrap();
        mark_restored(&data, &fingerprint, &package_path, &output).unwrap();
        let assets = snapshot(&data).unwrap();
        assert!(!assets[0].requires_restore());
        assert!(
            assets[0]
                .restored_outputs
                .iter()
                .any(|item| item.state == LocationState::Present)
        );
    }

    #[test]
    fn restored_identity_allows_edits_but_rejects_replaced_file_or_directory() {
        for directory in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("data");
            let package_path = root.path().join("package.fse");
            let output = root.path().join("restored");
            package(&package_path, b"FSEAL002");
            if directory {
                fs::create_dir(&output).unwrap();
                fs::write(output.join("content"), b"restored").unwrap();
            } else {
                fs::write(&output, b"restored").unwrap();
            }
            let fingerprint = observe(&data, &package_path).unwrap();
            mark_restored(&data, &fingerprint, &package_path, &output).unwrap();
            let edited = if directory {
                output.join("content")
            } else {
                output.clone()
            };
            fs::write(edited, b"user edited decrypted content").unwrap();
            assert!(!snapshot(&data).unwrap()[0].requires_restore());

            // 保留原对象，保证文件ID不能立即被文件系统重用。
            fs::rename(&output, root.path().join("previous-output")).unwrap();
            if directory {
                fs::create_dir(&output).unwrap();
            } else {
                fs::write(&output, b"replacement").unwrap();
            }
            let snapshot = snapshot(&data).unwrap();
            assert_eq!(
                snapshot[0].restored_outputs[0].state,
                LocationState::Changed
            );
            assert!(snapshot[0].requires_restore());
        }
    }

    #[test]
    fn legacy_restored_path_without_identity_requires_fresh_decryption() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let package_path = root.path().join("package.fse");
        let output = root.path().join("restored");
        package(&package_path, b"FSEAL002");
        fs::write(&output, b"restored").unwrap();
        let fingerprint = observe(&data, &package_path).unwrap();
        mark_restored(&data, &fingerprint, &package_path, &output).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(data.join(INDEX_FILE)).unwrap()).unwrap();
        value["assets"][&fingerprint]
            .as_object_mut()
            .unwrap()
            .remove("restored_identities");
        fs::write(data.join(INDEX_FILE), serde_json::to_vec(&value).unwrap()).unwrap();
        let snapshot = snapshot(&data).unwrap();
        assert_eq!(
            snapshot[0].restored_outputs[0].state,
            LocationState::Changed
        );
        assert!(snapshot[0].requires_restore());
        mark_restored(&data, &fingerprint, &package_path, &output).unwrap();
        assert!(!super::snapshot(&data).unwrap()[0].requires_restore());
    }

    #[test]
    fn corrupt_index_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join(INDEX_FILE), b"{broken").unwrap();
        assert!(snapshot(&data).is_err());
    }

    #[test]
    fn pending_commit_survives_failure_and_only_rolls_back_when_output_is_absent() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let payload = root.path().join("payload");
        let output = root.path().join("output.fse");
        package(&payload, b"FSEAL002");
        let fingerprint = record_pending(&data, &payload, &output).unwrap();
        let pending = snapshot(&data).unwrap();
        assert_eq!(pending[0].state, AssetState::Pending);
        assert!(pending[0].requires_restore());

        fs::write(&output, b"committed output").unwrap();
        assert!(rollback_pending(&data, &fingerprint, &output).is_err());
        assert_eq!(snapshot(&data).unwrap()[0].state, AssetState::Pending);

        fs::remove_file(&output).unwrap();
        rollback_pending(&data, &fingerprint, &output).unwrap();
        assert!(snapshot(&data).unwrap().is_empty());
    }

    #[test]
    fn relocation_removes_only_confirmed_missing_old_path() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let old = root.path().join("old.fse");
        let new = root.path().join("new.fse");
        package(&old, b"FSEAL002");
        let fingerprint = observe(&data, &old).unwrap();
        fs::rename(&old, &new).unwrap();
        assert_eq!(observe(&data, &new).unwrap(), fingerprint);
        let assets = snapshot(&data).unwrap();
        assert_eq!(assets[0].paths.len(), 1);
        assert_eq!(assets[0].paths[0].path, fs::canonicalize(new).unwrap());
    }

    #[test]
    fn changed_package_is_reported_without_being_reclassified_as_missing() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let package_path = root.path().join("package.fse");
        package(&package_path, b"FSEAL002");
        observe(&data, &package_path).unwrap();
        package(&package_path, b"FSEAL001");
        let assets = snapshot(&data).unwrap();
        assert_eq!(assets[0].paths[0].state, LocationState::Changed);
        assert!(assets[0].has_unknown_locations());
    }
}
