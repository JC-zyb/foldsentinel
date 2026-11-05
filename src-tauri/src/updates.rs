use semver::Version;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

const RELEASE_PAGE_PREFIX: &str = "https://github.com/JC-zyb/foldsentinel/releases/tag/";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_NOTES_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateError {
    Network,
    Timeout,
    RateLimited,
    InvalidResponse,
    Unavailable,
    Busy,
    OpenFailed,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Available,
    UpToDate,
    Ahead,
    NoRelease,
}

#[derive(Debug, Serialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub status: UpdateStatus,
    pub release_url: Option<String>,
    pub published_at: Option<String>,
    pub notes: Option<String>,
    pub prerelease: bool,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    published_at: Option<String>,
    body: Option<String>,
}

fn tag_version(tag: &str) -> Option<Version> {
    if tag.len() > 128 || !tag.is_ascii() {
        return None;
    }
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

fn checked_release_url(url: &str) -> bool {
    url.strip_prefix(RELEASE_PAGE_PREFIX)
        .and_then(tag_version)
        .is_some()
}

fn parse_releases(body: &[u8], current: &str) -> Result<UpdateCheck, UpdateError> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(UpdateError::InvalidResponse);
    }
    let current_version = Version::parse(current).map_err(|_| UpdateError::InvalidResponse)?;
    let releases: Vec<Release> =
        serde_json::from_slice(body).map_err(|_| UpdateError::InvalidResponse)?;
    if releases.len() > 100 {
        return Err(UpdateError::InvalidResponse);
    }
    // 项目以预览版发布；不能使用排除 prerelease 的 /releases/latest。
    let has_published = releases.iter().any(|release| !release.draft);
    let selected = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| tag_version(&release.tag_name).map(|version| (release, version)))
        .max_by(|(_, a), (_, b)| a.cmp_precedence(b));
    let Some((release, version)) = selected else {
        if has_published {
            return Err(UpdateError::InvalidResponse);
        }
        return Ok(UpdateCheck {
            current_version: current.to_owned(),
            latest_version: None,
            status: UpdateStatus::NoRelease,
            release_url: None,
            published_at: None,
            notes: None,
            prerelease: false,
        });
    };
    let status = match version.cmp_precedence(&current_version) {
        Ordering::Greater => UpdateStatus::Available,
        Ordering::Equal => UpdateStatus::UpToDate,
        Ordering::Less => UpdateStatus::Ahead,
    };
    let notes = release.body.map(|body| {
        let mut end = body.len().min(MAX_NOTES_BYTES);
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        let mut notes = body[..end]
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect::<String>();
        if end < body.len() {
            notes.push_str("\n…");
        }
        notes
    });
    Ok(UpdateCheck {
        current_version: current.to_owned(),
        latest_version: Some(version.to_string()),
        status,
        // 不信任响应里的 html_url；固定仓库域名和路径，仅拼接合法 SemVer 标签。
        release_url: Some(format!("{RELEASE_PAGE_PREFIX}{}", release.tag_name)),
        published_at: release.published_at.filter(|date| {
            date.len() <= 64 && date.is_ascii() && !date.chars().any(char::is_control)
        }),
        notes,
        prerelease: release.prerelease,
    })
}

fn check_status(status: u32) -> Result<(), UpdateError> {
    match status {
        200 => Ok(()),
        403 | 429 => Err(UpdateError::RateLimited),
        404 => Err(UpdateError::Unavailable),
        500..=599 => Err(UpdateError::Network),
        _ => Err(UpdateError::InvalidResponse),
    }
}

#[cfg(windows)]
pub fn check_for_updates() -> Result<UpdateCheck, UpdateError> {
    use std::{sync::atomic::AtomicBool, time::Duration};
    static CHECKING: AtomicBool = AtomicBool::new(false);
    run_check(&CHECKING, Duration::from_secs(20), || {
        windows::request_releases()
            .and_then(|body| parse_releases(&body, env!("CARGO_PKG_VERSION")))
    })
}

#[cfg(windows)]
fn run_check(
    checking: &'static std::sync::atomic::AtomicBool,
    timeout: std::time::Duration,
    operation: impl FnOnce() -> Result<UpdateCheck, UpdateError> + Send + 'static,
) -> Result<UpdateCheck, UpdateError> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    if checking
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(UpdateError::Busy);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("update-check".to_owned())
        .spawn(move || {
            struct Reset<'a>(&'a AtomicBool);
            impl Drop for Reset<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let reset = Reset(checking);
            let result = operation();
            drop(reset);
            // UI 超时后接收端已关闭，结果不再交付；工作线程仍负责关闭原生句柄。
            let _ = sender.send(result);
        });
    if spawned.is_err() {
        checking.store(false, Ordering::Release);
        return Err(UpdateError::Unavailable);
    }
    receiver
        .recv_timeout(timeout)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => UpdateError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => UpdateError::Unavailable,
        })?
}

#[cfg(not(windows))]
pub fn check_for_updates() -> Result<UpdateCheck, UpdateError> {
    Err(UpdateError::Unavailable)
}

#[cfg(windows)]
pub fn open_release_page(url: &str) -> Result<(), UpdateError> {
    use std::ptr;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    if !checked_release_url(url) {
        return Err(UpdateError::InvalidResponse);
    }
    let url = windows::wide(url);
    let operation = windows::wide("open");
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        Err(UpdateError::OpenFailed)
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn open_release_page(_url: &str) -> Result<(), UpdateError> {
    Err(UpdateError::Unavailable)
}

#[cfg(windows)]
mod windows {
    use super::{MAX_RESPONSE_BYTES, UpdateError, check_status};
    use std::{
        ffi::c_void,
        mem, ptr,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{Foundation::GetLastError, Networking::WinHttp::*};

    struct Handle(*mut c_void);
    impl Handle {
        fn new(value: *mut c_void) -> Result<Self, UpdateError> {
            if value.is_null() {
                Err(last_error())
            } else {
                Ok(Self(value))
            }
        }
        fn option(&self, name: u32, value: u32) -> Result<(), UpdateError> {
            ensure_call(unsafe {
                WinHttpSetOption(
                    self.0,
                    name,
                    (&value as *const u32).cast(),
                    mem::size_of::<u32>() as u32,
                )
            })
        }
        fn timeouts(&self, deadline: Instant) -> Result<(), UpdateError> {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(UpdateError::Timeout)?;
            let timeout = remaining.as_millis().clamp(1, 5000) as i32;
            ensure_call(unsafe { WinHttpSetTimeouts(self.0, timeout, timeout, timeout, timeout) })?;
            self.option(WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, timeout as u32)
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
    fn last_error() -> UpdateError {
        if unsafe { GetLastError() } == ERROR_WINHTTP_TIMEOUT {
            UpdateError::Timeout
        } else {
            UpdateError::Network
        }
    }
    fn ensure_call(result: i32) -> Result<(), UpdateError> {
        if result == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    pub(super) fn request_releases() -> Result<Vec<u8>, UpdateError> {
        let deadline = Instant::now() + Duration::from_secs(18);
        let agent = wide(concat!("FoldSentinel/", env!("CARGO_PKG_VERSION")));
        let session = Handle::new(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                ptr::null(),
                ptr::null(),
                0,
            )
        })?;
        session.timeouts(deadline)?;
        session.option(WINHTTP_OPTION_CONNECT_RETRIES, 1)?;
        session.option(
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
        )?;
        let host = wide("api.github.com");
        let connection = Handle::new(unsafe { WinHttpConnect(session.0, host.as_ptr(), 443, 0) })?;
        let method = wide("GET");
        let path = wide("/repos/JC-zyb/foldsentinel/releases?per_page=100&page=1");
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;
        request.option(
            WINHTTP_OPTION_DISABLE_FEATURE,
            WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
        )?;
        request.option(
            WINHTTP_OPTION_AUTOLOGON_POLICY,
            WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
        )?;
        request.option(WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE, 32 * 1024)?;
        let headers =
            wide("Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n");
        request.timeouts(deadline)?;
        ensure_call(unsafe {
            WinHttpSendRequest(
                request.0,
                headers.as_ptr(),
                (headers.len() - 1) as u32,
                ptr::null(),
                0,
                0,
                0,
            )
        })?;
        request.timeouts(deadline)?;
        ensure_call(unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) })?;
        let mut status = 0u32;
        let mut length = mem::size_of::<u32>() as u32;
        ensure_call(unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut length,
                ptr::null_mut(),
            )
        })?;
        check_status(status)?;
        let mut body = Vec::new();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            request.timeouts(deadline)?;
            let mut count = 0;
            ensure_call(unsafe {
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut count,
                )
            })?;
            if count == 0 {
                break;
            }
            if body.len() + count as usize > MAX_RESPONSE_BYTES {
                return Err(UpdateError::InvalidResponse);
            }
            body.extend_from_slice(&buffer[..count as usize]);
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn release(tag: &str) -> Value {
        json!({"tag_name":tag,"draft":false,"prerelease":true,"published_at":"2026-11-05T10:00:00Z","body":"发布说明"})
    }
    fn parse(values: Vec<Value>, current: &str) -> Result<UpdateCheck, UpdateError> {
        parse_releases(&serde_json::to_vec(&values).unwrap(), current)
    }
    #[test]
    fn highest_semver_includes_previews_and_ignores_unpublished_drafts() {
        let mut draft = release("v99.0.0");
        draft["draft"] = json!(true);
        let result = parse(
            vec![
                release("v0.9.9"),
                release("v0.10.0-beta.2"),
                draft,
                release("nightly"),
            ],
            "0.9.0",
        )
        .unwrap();
        assert_eq!(result.status, UpdateStatus::Available);
        assert_eq!(result.latest_version.as_deref(), Some("0.10.0-beta.2"));
        assert!(result.prerelease);
        assert_eq!(
            result.release_url.as_deref(),
            Some("https://github.com/JC-zyb/foldsentinel/releases/tag/v0.10.0-beta.2")
        );
    }
    #[test]
    fn version_precedence_handles_numeric_components_prereleases_and_build_metadata() {
        for (tag, current, expected) in [
            ("v1.10.0", "1.9.0", UpdateStatus::Available),
            ("1.0.0-beta.11", "1.0.0-beta.2", UpdateStatus::Available),
            ("v1.0.0", "1.0.0-rc.1", UpdateStatus::Available),
            ("v1.0.0-beta.2", "1.0.0", UpdateStatus::Ahead),
            ("v1.0.0+build.1", "1.0.0+build.2", UpdateStatus::UpToDate),
            ("v0.8.0", "0.9.0", UpdateStatus::Ahead),
        ] {
            assert_eq!(parse(vec![release(tag)], current).unwrap().status, expected);
        }
    }
    #[test]
    fn empty_and_draft_only_are_distinct_from_invalid_published_versions() {
        assert_eq!(
            parse(vec![], "0.9.0").unwrap().status,
            UpdateStatus::NoRelease
        );
        let mut draft = release("v1.0.0");
        draft["draft"] = json!(true);
        assert_eq!(
            parse(vec![draft], "0.9.0").unwrap().status,
            UpdateStatus::NoRelease
        );
        assert_eq!(
            parse(vec![release("nightly")], "0.9.0").unwrap_err(),
            UpdateError::InvalidResponse
        );
        assert_eq!(
            parse_releases(b"{}", "0.9.0").unwrap_err(),
            UpdateError::InvalidResponse
        );
        assert_eq!(
            parse(vec![json!({"tag_name":"v1.0.0"})], "0.9.0").unwrap_err(),
            UpdateError::InvalidResponse
        );
    }
    #[test]
    fn release_link_is_constructed_from_validated_tag_and_open_target_is_restricted() {
        let mut entry = release("v1.0.0");
        entry["html_url"] = json!("https://evil.example/");
        let result = parse(vec![entry], "0.9.0").unwrap();
        assert!(checked_release_url(result.release_url.as_ref().unwrap()));
        for suffix in [
            "",
            "latest",
            "v1.0.0/extra",
            "v1.0.0?x=1",
            "v1.0.0#x",
            "v1.0.0\0",
            "v1.0.0\\x",
        ] {
            assert!(!checked_release_url(&format!(
                "{RELEASE_PAGE_PREFIX}{suffix}"
            )));
        }
        for target in [
            "https://evil.example/releases/tag/v1.0.0",
            "https://github.com.evil.example/JC-zyb/foldsentinel/releases/tag/v1.0.0",
            "http://github.com/JC-zyb/foldsentinel/releases/tag/v1.0.0",
            "file:///C:/Windows/notepad.exe",
        ] {
            assert!(!checked_release_url(target));
        }
    }
    #[test]
    fn response_and_notes_remain_bounded_and_unicode_safe() {
        assert_eq!(
            parse_releases(&vec![b' '; MAX_RESPONSE_BYTES + 1], "0.9.0").unwrap_err(),
            UpdateError::InvalidResponse
        );
        assert_eq!(
            parse(vec![release("v1.0.0"); 101], "0.9.0").unwrap_err(),
            UpdateError::InvalidResponse
        );
        let mut entry = release("v1.0.0");
        entry["body"] = json!("中".repeat(3000));
        let notes = parse(vec![entry], "0.9.0").unwrap().notes.unwrap();
        assert!(notes.len() <= MAX_NOTES_BYTES + "\n…".len());
        assert!(notes.ends_with("\n…"));
    }
    #[test]
    fn http_errors_do_not_report_no_updates() {
        for (status, expected) in [
            (403, UpdateError::RateLimited),
            (429, UpdateError::RateLimited),
            (404, UpdateError::Unavailable),
            (502, UpdateError::Network),
            (301, UpdateError::InvalidResponse),
            (401, UpdateError::InvalidResponse),
        ] {
            assert_eq!(check_status(status).unwrap_err(), expected);
        }
        assert!(check_status(200).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn timeout_returns_without_waiting_for_io_and_rejects_overlapping_checks() {
        use std::{
            sync::{
                atomic::{AtomicBool, Ordering},
                mpsc,
            },
            time::{Duration, Instant},
        };
        static CHECKING: AtomicBool = AtomicBool::new(false);
        let (release, waiting) = mpsc::sync_channel(1);
        let start = Instant::now();
        let result = run_check(&CHECKING, Duration::from_millis(20), move || {
            waiting.recv_timeout(Duration::from_secs(5)).unwrap();
            parse_releases(b"[]", "0.9.0")
        });
        assert_eq!(result.unwrap_err(), UpdateError::Timeout);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            run_check(&CHECKING, Duration::from_secs(1), || panic!(
                "并发检查不应启动"
            ))
            .unwrap_err(),
            UpdateError::Busy
        );
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while CHECKING.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(!CHECKING.load(Ordering::Acquire));
        assert_eq!(
            run_check(&CHECKING, Duration::from_secs(1), || parse_releases(
                b"[]", "0.9.0"
            ))
            .unwrap()
            .status,
            UpdateStatus::NoRelease
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "需要主动访问固定 GitHub Releases API"]
    fn live_github_check_returns_a_real_result() {
        let result = check_for_updates().expect("真实更新检查必须成功，不能把网络错误当作已是最新");
        assert_eq!(result.current_version, env!("CARGO_PKG_VERSION"));
        if let Some(url) = &result.release_url {
            assert!(checked_release_url(url));
        }
        println!(
            "真实 GitHub 更新检查结果: {:?}, latest={:?}",
            result.status, result.latest_version
        );
    }
}
