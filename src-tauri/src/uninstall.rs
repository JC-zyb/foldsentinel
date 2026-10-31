//! NSIS 卸载器使用的安全预检与收尾。
//!
//! 这个入口只处理卸载协议，不启动 Tauri 窗口。NSIS 在删除程序目录前
//! 调用 `--uninstall-prepare`，删除完成后调用 `--uninstall-finish`。返回码
//! 是卸载钩子之间的稳定契约：0 表示可以继续，10 表示已完成交互式强制
//! 确认，其他值都表示拒绝或无法确认。

use anyhow::{Context, Result, bail, ensure};
use foldsentinel::{StateStore, lifecycle, service, vault};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const EXIT_OK: i32 = 0;
pub const EXIT_FORCED: i32 = 10;
pub const EXIT_BLOCKED: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Prepare,
    Finish,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    parent_pid: u32,
    silent: bool,
}

pub fn maybe_run() -> Option<i32> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--uninstall-service-elevated") {
        return Some(run_service_remove(false));
    }
    if args
        .iter()
        .any(|arg| arg == "--uninstall-service-force-elevated")
    {
        return Some(run_service_remove(true));
    }
    let mode = if args.iter().any(|arg| arg == "--uninstall-prepare") {
        Some(Mode::Prepare)
    } else if args.iter().any(|arg| arg == "--uninstall-finish") {
        Some(Mode::Finish)
    } else {
        None
    }?;
    Some(run(Args::parse(&args, mode)))
}

fn run_service_remove(forced: bool) -> i32 {
    let result = if forced {
        service::uninstall_forced()
    } else {
        service::uninstall_empty()
    };
    match result {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("持续防护服务未能安全卸载：{error:#}");
            EXIT_BLOCKED
        }
    }
}

#[cfg(windows)]
fn elevate_service_remove(forced: bool, silent: bool) -> i32 {
    use std::mem::size_of;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    let elevated = unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            false
        } else {
            let mut value: TOKEN_ELEVATION = std::mem::zeroed();
            let mut size = 0;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&mut value as *mut TOKEN_ELEVATION).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut size,
            ) != 0;
            CloseHandle(token);
            ok && value.TokenIsElevated != 0
        }
    };
    if elevated {
        return run_service_remove(forced);
    }
    if silent {
        return EXIT_BLOCKED;
    }
    let file = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("无法定位卸载程序：{error}");
            return EXIT_BLOCKED;
        }
    };
    let file = wide_os(&file);
    let verb = wide("runas");
    let parameters = wide(if forced {
        "--uninstall-service-force-elevated --silent"
    } else {
        "--uninstall-service-elevated --silent"
    });
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 0;
    let launched = unsafe { ShellExecuteExW(&mut info) != 0 };
    if !launched || info.hProcess.is_null() {
        return EXIT_BLOCKED;
    }
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
    }
    let mut code = 1;
    unsafe {
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    code as i32
}

#[cfg(not(windows))]
fn elevate_service_remove(forced: bool, _silent: bool) -> i32 {
    run_service_remove(forced)
}

#[cfg(windows)]
fn wide_os(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Args {
    fn parse(args: &[String], mode: Mode) -> Args {
        let parent_pid = args
            .windows(2)
            .find(|pair| pair[0] == "--parent-pid")
            .and_then(|pair| pair[1].parse().ok())
            .unwrap_or(0);
        Args {
            mode,
            parent_pid,
            silent: args.iter().any(|arg| arg == "--silent"),
        }
    }
}

fn run(args: Args) -> i32 {
    let result = match args.mode {
        Mode::Prepare => prepare(&args),
        Mode::Finish => finish(&args),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("卸载安全检查失败：{error:#}");
            if !args.silent {
                show_message(
                    &format!(
                        "FoldSentinel 卸载已停止 / Uninstall stopped\n\n{error:#}\n\n请关闭软件中的文件任务、完成解密或解除保护后重试。\nClose running tasks, restore encrypted projects and remove protection before retrying."
                    ),
                    "FoldSentinel 卸载安全检查",
                    MessageKind::Error,
                );
            }
            EXIT_BLOCKED
        }
    }
}

fn prepare(args: &Args) -> Result<i32> {
    ensure!(args.parent_pid != 0, "缺少卸载器进程标识");
    let data_dir = lifecycle::default_data_dir()?;
    let guard = lifecycle::UninstallGuard::acquire(&data_dir)?;

    let audit = audit_normal(&data_dir, args.silent).err();
    if let Some(error) = audit {
        if args.silent {
            bail!("正常卸载条件未满足（静默模式不会强制卸载）：{error:#}");
        }
        if !confirm_force(error) {
            bail!("用户取消强制卸载");
        }
        if service::is_installed()? {
            ensure!(
                elevate_service_remove(true, false) == EXIT_OK,
                "独立服务未能卸载，已保留程序与恢复资料"
            );
        }
        guard.arm(args.parent_pid, true)?;
        return Ok(EXIT_FORCED);
    }

    guard.arm(args.parent_pid, false)?;
    Ok(EXIT_OK)
}

fn audit_normal(data_dir: &Path, silent: bool) -> Result<()> {
    let cache = cache_dir()?;
    cache_entries(&cache)?;
    audit_local(data_dir, &scan_roots()?)?;
    if service::is_installed()? {
        ensure!(
            elevate_service_remove(false, silent) == EXIT_OK,
            "保护服务仍有登记、无法完成全账户审计，或管理员卸载未完成"
        );
    }
    Ok(())
}

fn audit_local(data_dir: &Path, roots: &[PathBuf]) -> Result<()> {
    metadata_entries(data_dir)?;
    let store = StateStore::load_for_uninstall(data_dir.join("state.json"))?;
    ensure!(
        store.entries().next().is_none(),
        "本机仍登记有文件保护项目；请先逐项关闭保护并保存结果"
    );

    let jobs = vault::recovery::inspect_jobs(&data_dir.join("jobs"))?;
    ensure!(
        jobs.is_empty(),
        "存在未完成或无法确认的恢复任务（{} 条）",
        jobs.len()
    );

    let discovery = vault::inventory::discover(roots, data_dir)?;
    ensure!(
        discovery.complete && discovery.issues.is_empty(),
        "旧加密包扫描未完整完成（{} 个问题）：{}",
        discovery.issues.len(),
        discovery
            .issues
            .iter()
            .take(3)
            .map(|issue| format!("{}: {}", issue.path.display(), issue.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let assets = vault::inventory::snapshot(data_dir)?;
    ensure!(
        assets.iter().all(|asset| !asset.requires_restore()),
        "仍有加密项目未解密，或解密后的文件无法确认存在"
    );

    Ok(())
}

#[cfg(windows)]
fn known_folder(id: &windows_sys::core::GUID) -> Result<PathBuf> {
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath},
    };
    let mut value = std::ptr::null_mut();
    let result = unsafe {
        SHGetKnownFolderPath(
            id,
            KF_FLAG_DONT_VERIFY as u32,
            std::ptr::null_mut(),
            &mut value,
        )
    };
    ensure!(
        result >= 0 && !value.is_null(),
        "无法读取 Windows 用户目录：{result}"
    );
    let mut length = 0;
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
    }
    let path = unsafe { String::from_utf16(std::slice::from_raw_parts(value, length)) };
    unsafe {
        CoTaskMemFree(value.cast());
    }
    Ok(PathBuf::from(path?))
}

#[cfg(windows)]
fn cache_dir() -> Result<PathBuf> {
    use windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData;
    Ok(known_folder(&FOLDERID_LocalAppData)?.join("io.github.jc-zyb.foldsentinel"))
}

#[cfg(windows)]
fn scan_roots() -> Result<Vec<PathBuf>> {
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads};
    // KnownFolder 返回重定向后的实际目录，包括用户配置的 OneDrive 位置。
    let candidates = vec![
        known_folder(&FOLDERID_Desktop)?,
        known_folder(&FOLDERID_Documents)?,
        known_folder(&FOLDERID_Downloads)?,
        lifecycle::default_data_dir()?,
        cache_dir()?,
    ];
    let mut roots = BTreeSet::new();
    for candidate in candidates {
        ensure!(candidate.is_absolute(), "旧包扫描目录不是绝对路径");
        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 只有父目录明确可读时，才把尚未创建的标准目录视为空。
                // 离线卷或不可访问的重定向父路径不能冒充“没有旧包”。
                fs::read_dir(candidate.parent().context("扫描目录缺少父路径")?)
                    .context("旧包扫描目录的父路径不可访问")?;
                continue;
            }
            Err(error) => return Err(error).context("无法读取旧包扫描目录"),
        };
        ensure!(metadata.is_dir(), "旧包扫描路径不是目录");
        ensure_plain(&candidate)?;
        roots.insert(fs::canonicalize(candidate)?);
    }
    ensure!(!roots.is_empty(), "没有可检查的旧包扫描目录");
    Ok(roots.into_iter().collect())
}

#[cfg(not(windows))]
fn scan_roots() -> Result<Vec<PathBuf>> {
    bail!("卸载仅支持 Windows")
}
#[cfg(not(windows))]
fn cache_dir() -> Result<PathBuf> {
    bail!("卸载仅支持 Windows")
}

fn confirm_force(error: anyhow::Error) -> bool {
    confirm_force_using(error, show_message)
}

fn confirm_force_using(
    error: anyhow::Error,
    mut ask: impl FnMut(&str, &str, MessageKind) -> Option<MessageResult>,
) -> bool {
    let summary = format!(
        "正常卸载被阻止 / Normal uninstall is blocked:\n\n{error:#}\n\n强制卸载会移除软件及独立服务，但不会解密或解除文件保护。加密包、源文件、恢复记录和应用数据均保留。文件可能继续不可访问；以后需要重新安装软件、原密码和恢复资料才能处理，资料丢失可能导致无法恢复。\n\nForced uninstall removes the app and its service without decrypting packages or removing file protection. Source files, packages, recovery records and app data are retained. Files may remain inaccessible. Reinstall the app with the original password and recovery records to recover them; lost records may prevent recovery.\n\n是否理解风险并继续？ / Accept these risks and continue?"
    );
    if ask(
        &summary,
        "FoldSentinel 强制卸载免责声明",
        MessageKind::Warning,
    ) != Some(MessageResult::Yes)
    {
        return false;
    }
    ask(
        "第二次确认：我已理解并接受上述风险，仍要强制卸载。\n\nSecond confirmation: I understand and accept the risks and want to force uninstall.\n\n是否继续？ / Continue?",
        "FoldSentinel 二次确认",
        MessageKind::Confirm,
    ) == Some(MessageResult::Yes)
}

fn finish(args: &Args) -> Result<i32> {
    finish_at(
        args,
        &lifecycle::default_data_dir()?,
        &cache_dir()?,
        service::is_installed,
    )
}

fn finish_at(
    args: &Args,
    data_dir: &Path,
    cache: &Path,
    installed: impl FnOnce() -> Result<bool>,
) -> Result<i32> {
    ensure!(args.parent_pid != 0, "缺少卸载器进程标识");
    let (guard, session) = lifecycle::UninstallGuard::resume(data_dir, args.parent_pid)?;
    if !session.forced {
        ensure!(!installed()?, "独立服务仍存在，拒绝清理应用状态");
        validate_local_state(data_dir)?;
        let cached = cache_entries(cache)?;
        remove_cache_entries(cache, cached)?;
        clean_verified_metadata(data_dir)?;
    }
    guard.finish()?;
    if !session.forced {
        // Windows 不能在持有生命周期锁时删除锁文件；会话清除后再移除
        // 固定锁和空目录，避免“程序文件删了但状态目录还残留”的假完成。
        drop(guard);
        match fs::remove_file(data_dir.join("lifecycle.lock")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("清理应用生命周期锁失败"),
        }
        ensure!(
            fs::read_dir(data_dir)?.next().transpose()?.is_none(),
            "应用数据目录仍有内容，未完成完整卸载"
        );
        fs::remove_dir(data_dir).context("清理空应用数据目录失败")?;
    }
    Ok(EXIT_OK)
}

fn ensure_plain(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "路径包含重解析点：{}",
                ancestor.display()
            );
        }
        #[cfg(not(windows))]
        ensure!(!metadata.file_type().is_symlink(), "路径包含链接");
    }
    Ok(())
}

fn validate_local_state(data_dir: &Path) -> Result<()> {
    metadata_entries(data_dir)?;
    let store = StateStore::load_for_uninstall(data_dir.join("state.json"))?;
    ensure!(
        store.entries().next().is_none(),
        "仍有文件保护登记，拒绝清理恢复资料"
    );
    ensure!(
        vault::inventory::snapshot(data_dir)?
            .iter()
            .all(|asset| !asset.requires_restore()),
        "加密资产仍需恢复，拒绝清理资产资料"
    );
    Ok(())
}

fn metadata_entries(data_dir: &Path) -> Result<Vec<PathBuf>> {
    // 预检已确认这些文件只包含本软件状态；任何未知条目都留下并报错，
    // 防止卸载器把用户放进应用目录的文件递归删除。
    let allowed = BTreeSet::from([
        "state.json",
        "state.lock",
        "vault-inventory.json",
        "vault-inventory.lock",
        "lifecycle.lock",
        "uninstall-session.json",
        "jobs",
    ]);
    ensure_plain(data_dir)?;
    let entries: Vec<_> = fs::read_dir(data_dir)
        .context("读取应用数据目录失败")?
        .collect::<std::io::Result<Vec<_>>>()?;
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        ensure!(
            allowed.contains(name.as_str()),
            "应用数据目录存在未知文件：{name}"
        );
        let path = entry.path();
        ensure_plain(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        if name == "jobs" {
            ensure!(metadata.is_dir(), "恢复目录不是普通目录");
            ensure!(
                fs::read_dir(&path)?.next().transpose()?.is_none(),
                "恢复目录仍有任务或未知数据"
            );
        } else {
            ensure!(metadata.is_file(), "应用状态不是普通文件：{name}");
        }
    }
    Ok(entries.into_iter().map(|entry| entry.path()).collect())
}

fn clean_verified_metadata(data_dir: &Path) -> Result<()> {
    for path in metadata_entries(data_dir)? {
        let name = path.file_name().context("应用数据条目缺少名称")?;
        if name == "lifecycle.lock" || name == "uninstall-session.json" {
            continue;
        }
        if name == "jobs" {
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(metadata.is_dir(), "恢复目录不是普通目录");
            ensure!(fs::read_dir(&path)?.next().is_none(), "恢复目录仍有记录");
            fs::remove_dir(&path)?;
        } else {
            fs::remove_file(&path)
                .with_context(|| format!("清理应用状态失败：{}", path.display()))?;
        }
    }
    Ok(())
}

fn cache_entries(cache: &Path) -> Result<Vec<PathBuf>> {
    match fs::symlink_metadata(cache) {
        Ok(_) => ensure_plain(cache)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("无法读取应用浏览器数据"),
    }
    for entry in fs::read_dir(cache)? {
        let entry = entry?;
        ensure!(
            entry.file_name() == "EBWebView",
            "应用缓存目录存在未知条目，已保留：{}",
            entry.path().display()
        );
        ensure!(entry.file_type()?.is_dir(), "应用浏览器数据不是目录");
    }
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(cache).follow_links(false) {
        let entry = entry?;
        ensure!(
            entries.len() < 100_000,
            "应用缓存条目过多，无法确认完整清理"
        );
        ensure_plain(entry.path())?;
        ensure!(
            entry.file_type().is_dir() || entry.file_type().is_file(),
            "应用缓存包含未知文件类型"
        );
        entries.push(entry.into_path());
    }
    Ok(entries)
}

fn remove_cache_entries(cache: &Path, mut entries: Vec<PathBuf>) -> Result<()> {
    entries.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in entries {
        ensure!(path.starts_with(cache), "应用缓存清理路径越界");
        ensure_plain(&path)?;
        if fs::symlink_metadata(&path)?.is_dir() {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    Error,
    Warning,
    Confirm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageResult {
    Yes,
}

#[cfg(windows)]
fn show_message(text: &str, title: &str, kind: MessageKind) -> Option<MessageResult> {
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    const MB_OK: u32 = 0x0000;
    const MB_YESNO: u32 = 0x0004;
    const MB_ICONERROR: u32 = 0x0010;
    const MB_ICONWARNING: u32 = 0x0030;
    const IDYES: i32 = 6;
    let text = wide(text);
    let title = wide(title);
    let typ = match kind {
        MessageKind::Error => MB_OK | MB_ICONERROR,
        MessageKind::Warning => MB_YESNO | MB_ICONWARNING | 0x100,
        MessageKind::Confirm => MB_YESNO | MB_ICONWARNING | 0x100,
    };
    let result = unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), typ) };
    (result == IDYES).then_some(MessageResult::Yes)
}

#[cfg(not(windows))]
fn show_message(_text: &str, _title: &str, _kind: MessageKind) -> Option<MessageResult> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let inputs = root.path().join("inputs");
        let cache = root.path().join("browser");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&inputs).unwrap();
        (root, data, inputs, cache)
    }

    fn arm(data: &Path, forced: bool) -> Args {
        let guard = lifecycle::UninstallGuard::acquire(data).unwrap();
        guard.arm(std::process::id(), forced).unwrap();
        Args {
            mode: Mode::Finish,
            parent_pid: std::process::id(),
            silent: true,
        }
    }

    #[test]
    fn old_packages_require_real_restore_and_normal_finish_removes_only_app_data() {
        let (_root, data, inputs, cache) = fixture();
        let source = inputs.join("source");
        let package = inputs.join("old-renamed.bin");
        let restored = inputs.join("restored");
        fs::write(&source, b"original content").unwrap();
        let control = || vault::Control::with_recovery_dir(data.join("jobs"), |_| {});
        vault::encrypt_with_control(&source, &package, "test", &control()).unwrap();
        assert!(audit_local(&data, std::slice::from_ref(&inputs)).is_err());
        vault::decrypt_with_control(&package, &restored, "test", &control()).unwrap();
        audit_local(&data, std::slice::from_ref(&inputs)).unwrap();
        fs::create_dir_all(cache.join("EBWebView/Default")).unwrap();
        fs::write(cache.join("EBWebView/Default/Preferences"), b"app settings").unwrap();
        let args = arm(&data, false);
        assert_eq!(
            finish_at(&args, &data, &cache, || Ok(false)).unwrap(),
            EXIT_OK
        );
        assert!(!data.exists());
        assert!(!cache.exists());
        assert_eq!(fs::read(source).unwrap(), b"original content");
        assert_eq!(fs::read(restored).unwrap(), b"original content");
        assert!(package.exists());
    }

    #[test]
    fn unknown_metadata_jobs_and_invalid_registry_block_before_removal() {
        let (_root, data, inputs, _cache) = fixture();
        for path in [data.join("unknown.txt"), data.join("jobs/partial.bin")] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"preserve").unwrap();
            assert!(audit_local(&data, std::slice::from_ref(&inputs)).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"preserve");
            fs::remove_file(path).unwrap();
        }
        fs::write(data.join("state.json"), b"broken").unwrap();
        assert!(audit_local(&data, std::slice::from_ref(&inputs)).is_err());
    }

    #[test]
    fn registered_local_protection_blocks_even_when_target_is_missing() {
        let (_root, data, inputs, _cache) = fixture();
        let path = inputs.join("missing").to_string_lossy().into_owned();
        let json = serde_json::json!({"version":1,"entries":{&path:{"path":&path,"mode":"hidden","original_attributes":32}}});
        fs::write(data.join("state.json"), serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(
            audit_local(&data, &[inputs])
                .unwrap_err()
                .to_string()
                .contains("文件保护")
        );
    }

    #[test]
    fn forced_finish_retains_all_recovery_and_browser_data() {
        let (_root, data, _inputs, cache) = fixture();
        fs::write(data.join("state.json"), b"unreadable registry").unwrap();
        fs::write(data.join("unknown-recovery"), b"keep").unwrap();
        fs::create_dir_all(cache.join("EBWebView")).unwrap();
        fs::write(cache.join("EBWebView/settings"), b"keep settings").unwrap();
        let args = arm(&data, true);
        finish_at(&args, &data, &cache, || {
            panic!("forced finish must not silently reinterpret a service state")
        })
        .unwrap();
        assert_eq!(
            fs::read(data.join("state.json")).unwrap(),
            b"unreadable registry"
        );
        assert_eq!(fs::read(data.join("unknown-recovery")).unwrap(), b"keep");
        assert_eq!(
            fs::read(cache.join("EBWebView/settings")).unwrap(),
            b"keep settings"
        );
        assert!(!data.join("uninstall-session.json").exists());
    }

    #[test]
    fn changed_state_or_service_at_finish_preserves_recovery_records() {
        let (_root, data, inputs, cache) = fixture();
        audit_local(&data, &[inputs]).unwrap();
        let args = arm(&data, false);
        assert!(finish_at(&args, &data, &cache, || Ok(true)).is_err());
        assert!(data.join("vault-inventory.json").exists());
        fs::write(data.join("unexpected"), b"keep").unwrap();
        assert!(finish_at(&args, &data, &cache, || Ok(false)).is_err());
        assert!(data.join("vault-inventory.json").exists());
    }

    #[test]
    fn cache_unknown_files_or_reparse_points_are_preserved() {
        let (_root, _data, _inputs, cache) = fixture();
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("user-file"), b"keep").unwrap();
        assert!(cache_entries(&cache).is_err());
        assert_eq!(fs::read(cache.join("user-file")).unwrap(), b"keep");
    }

    #[test]
    fn force_requires_two_affirmative_answers_and_contains_disclaimer() {
        for (answers, expected_calls, expected) in [
            (vec![None], 1, false),
            (vec![Some(MessageResult::Yes), None], 2, false),
            (
                vec![Some(MessageResult::Yes), Some(MessageResult::Yes)],
                2,
                true,
            ),
        ] {
            let mut calls = 0;
            let result = confirm_force_using(anyhow::anyhow!("blocked"), |text, _, _| {
                assert!(text.contains("风险") || text.contains("risks"));
                let answer = answers[calls];
                calls += 1;
                answer
            });
            assert_eq!(result, expected);
            assert_eq!(calls, expected_calls);
        }
    }

    #[test]
    fn parser_requires_explicit_parent_pid_for_a_valid_run() {
        let args = vec!["app".to_owned(), "--uninstall-prepare".to_owned()];
        assert_eq!(Args::parse(&args, Mode::Prepare).parent_pid, 0);
        let args = vec![
            "app".to_owned(),
            "--uninstall-finish".to_owned(),
            "--parent-pid".to_owned(),
            "42".to_owned(),
            "--silent".to_owned(),
        ];
        let parsed = Args::parse(&args, Mode::Finish);
        assert_eq!(parsed.parent_pid, 42);
        assert!(parsed.silent);
    }
}
