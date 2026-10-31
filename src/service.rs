//! 本机服务通信与宿主。客户端身份由 Windows 管道令牌确定，不能由请求指定。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use zeroize::{Zeroize, Zeroizing};

use crate::ProtectionMode;
use crate::vault::{ErrorCode, error_code};

pub const SERVICE_NAME: &str = "FoldSentinelProtection";
pub const PIPE_NAME: &str = r"\\.\pipe\FoldSentinel.Protection.v1";
pub const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub command: Command,
}

impl Request {
    pub fn new(command: Command) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            command,
        }
    }
}

// 故意不派生 Debug：密码不应通过日志或断言意外展开。
#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    List,
    UninstallAudit,
    CheckLocal {
        path: PathBuf,
    },
    Enroll {
        path: PathBuf,
        mode: ProtectionMode,
        #[serde(default)]
        note: String,
        password: Option<String>,
    },
    Unlock {
        id: String,
        password: String,
        #[serde(default = "default_duration")]
        duration_seconds: u64,
    },
    Relock {
        id: String,
    },
    Remove {
        id: String,
        password: Option<String>,
    },
    SetPassword {
        id: String,
        password: String,
        new_password: Option<String>,
    },
    SetContinuous {
        id: String,
        password: Option<String>,
        enabled: bool,
    },
}

fn default_duration() -> u64 {
    300
}

impl Drop for Command {
    fn drop(&mut self) {
        match self {
            Self::Enroll { password, .. }
            | Self::Remove { password, .. }
            | Self::SetContinuous { password, .. } => {
                if let Some(value) = password {
                    value.zeroize();
                }
            }
            Self::Unlock { password, .. } => password.zeroize(),
            Self::SetPassword {
                password,
                new_password,
                ..
            } => {
                password.zeroize();
                if let Some(value) = new_password {
                    value.zeroize();
                }
            }
            _ => {}
        }
    }
}

impl Command {
    fn validate(&self) -> Result<()> {
        let valid_password = |password: Option<&str>| password.is_none_or(|p| p.len() <= 1024);
        let valid_id =
            |id: &str| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control);
        let valid = match self {
            Self::Status | Self::List | Self::UninstallAudit => true,
            Self::CheckLocal { path } => valid_local_check_path(path),
            Self::Enroll {
                path,
                mode,
                note,
                password,
            } => {
                path.is_absolute()
                    && path.as_os_str().len() <= 32760
                    && matches!(
                        mode,
                        ProtectionMode::Locked
                            | ProtectionMode::HiddenLocked
                            | ProtectionMode::ReadonlyAcl
                    )
                    && note.chars().count() <= 80
                    && !note.chars().any(char::is_control)
                    && valid_password(password.as_deref())
            }
            Self::Unlock {
                id,
                password,
                duration_seconds,
            } => {
                valid_id(id)
                    && valid_password(Some(password))
                    && (30..=3600).contains(duration_seconds)
            }
            Self::Relock { id } => valid_id(id),
            Self::Remove { id, password } | Self::SetContinuous { id, password, .. } => {
                valid_id(id) && valid_password(password.as_deref())
            }
            Self::SetPassword {
                id,
                password,
                new_password,
            } => {
                valid_id(id)
                    && valid_password(Some(password))
                    && valid_password(new_password.as_deref())
            }
        };
        if !valid {
            bail!(ErrorCode::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportStatus {
    pub host: String,
    pub healthy: bool,
    pub last_tick_error: Option<ErrorCode>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub transport: TransportStatus,
    pub data: Option<Value>,
    pub error: Option<ErrorCode>,
}

/// 卸载前的全账户汇总；不包含SID、路径、密码或单项恢复资料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UninstallAudit {
    pub registered: usize,
    pub has_errors: bool,
}

impl Response {
    pub fn into_result(self) -> Result<Value> {
        if self.version != PROTOCOL_VERSION {
            bail!(ErrorCode::UnsupportedFormat);
        }
        match (self.data, self.error) {
            (_, Some(code)) => Err(code.into()),
            (Some(data), None) => Ok(data),
            _ => Err(ErrorCode::Internal.into()),
        }
    }
}

pub struct ClientContext {
    sid: String,
    state_dir: PathBuf,
}

impl ClientContext {
    pub fn sid(&self) -> &str {
        &self.sid
    }
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

/// 只能由宿主在客户端模拟身份下创建；持有句柄至登记完成。
pub struct AuthorizedTree {
    pub(crate) paths: Vec<crate::managed::AuthorizedPath>,
    #[cfg(windows)]
    _held: Vec<std::fs::File>,
}

impl AuthorizedTree {
    pub fn paths(&self) -> &[crate::managed::AuthorizedPath] {
        &self.paths
    }
}

pub trait Backend: Send + 'static {
    fn handle(
        &mut self,
        client: &ClientContext,
        command: &Command,
        authorization: Option<&AuthorizedTree>,
    ) -> Result<Value>;
    fn tick(&mut self) -> Result<()>;
    fn shutdown(&mut self) -> Result<()>;
}

/// 生产入口只信任SYSTEM服务；无法连接时返回真实错误，不启动替代后台进程。
pub fn request(request: &Request) -> Result<Response> {
    #[cfg(windows)]
    {
        windows::request_to(request, PIPE_NAME, None)
    }
    #[cfg(not(windows))]
    {
        let _ = request;
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

/// 只有SCM明确报告服务不存在才返回false；查询权限或状态错误必须向上传递。
pub fn is_installed() -> Result<bool> {
    #[cfg(windows)]
    {
        windows::service_is_installed()
    }
    #[cfg(not(windows))]
    {
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

/// 通过已鉴权的运行中服务读取全部账户登记总数。失联和旧协议不能当作无保护。
pub fn uninstall_audit() -> Result<Option<UninstallAudit>> {
    uninstall_audit_using(is_installed, || {
        request(&Request::new(Command::UninstallAudit))
    })
}

/// 管理员执行一次正常服务卸载。运行中的服务先审计为空才停止，停止后再次审计。
/// 只有全账户恢复审计为空且固定程序目录安全时才会删除。
pub fn uninstall_empty() -> Result<()> {
    #[cfg(windows)]
    {
        windows::uninstall_empty()
    }
    #[cfg(not(windows))]
    {
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

/// 管理员执行已经由调用方双重确认的强制卸载；保留全部 ProgramData 恢复资料。
/// 此 API 只跳过受管项目空审计，不跳过身份、固定程序路径和服务停止核验。
pub fn uninstall_forced() -> Result<()> {
    #[cfg(windows)]
    {
        windows::uninstall_forced()
    }
    #[cfg(not(windows))]
    {
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

fn uninstall_audit_using(
    installed: impl FnOnce() -> Result<bool>,
    read: impl FnOnce() -> Result<Response>,
) -> Result<Option<UninstallAudit>> {
    if !installed().context(ErrorCode::ServiceUnavailable)? {
        return Ok(None);
    }
    let response = read().context(ErrorCode::ServiceUnavailable)?;
    if response.version != PROTOCOL_VERSION
        || response.transport.host != "service"
        || !response.transport.healthy
        || response.transport.last_tick_error.is_some()
        || response.error.is_some()
    {
        bail!(ErrorCode::ServiceUnavailable);
    }
    let audit = serde_json::from_value(response.data.context(ErrorCode::ServiceUnavailable)?)
        .context(ErrorCode::ServiceUnavailable)?;
    Ok(Some(audit))
}

/// 本地保护在修改前排除所有账户的服务受管范围；服务失联不能当作空范围。
pub fn ensure_local_available(paths: &[PathBuf]) -> Result<()> {
    #[cfg(windows)]
    {
        ensure_local_available_using(paths, windows::service_is_installed, |path| {
            request(&Request::new(Command::CheckLocal {
                path: path.to_owned(),
            }))
        })
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

pub(crate) fn ensure_local_available_using(
    paths: &[PathBuf],
    installed: impl FnOnce() -> Result<bool>,
    mut check: impl FnMut(&Path) -> Result<Response>,
) -> Result<()> {
    if paths.iter().any(|path| !valid_local_check_path(path)) {
        bail!(ErrorCode::InvalidInput);
    }
    if paths.is_empty() {
        return Ok(());
    }
    if !installed().context(ErrorCode::ServiceUnavailable)? {
        return Ok(());
    }
    for path in paths {
        let response = check(path).context(ErrorCode::ServiceUnavailable)?;
        if response.version != PROTOCOL_VERSION
            || response.transport.host != "service"
            || !response.transport.healthy
            || response.transport.last_tick_error.is_some()
        {
            bail!(ErrorCode::ServiceUnavailable);
        }
        match (response.data, response.error) {
            (None, Some(ErrorCode::ProtectionServiceOverlap)) => {
                bail!(ErrorCode::ProtectionServiceOverlap)
            }
            (Some(Value::Object(fields)), None)
                if fields.len() == 1 && fields.get("available") == Some(&Value::Bool(true)) => {}
            _ => bail!(ErrorCode::ServiceUnavailable),
        }
    }
    Ok(())
}

// 纯词法检查。服务绝不为CheckLocal以SYSTEM打开、canonicalize或探测输入路径。
fn valid_local_check_path(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    if text.is_empty() || text.len() > 32760 || text.chars().any(char::is_control) {
        return false;
    }
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let bytes = text.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return false;
    }
    let tail = &text[3..];
    tail.is_empty()
        || tail.split('\\').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && !segment.ends_with(['.', ' '])
                && !segment
                    .chars()
                    .any(|c| matches!(c, '/' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        })
}

pub fn run_service() -> Result<()> {
    #[cfg(windows)]
    {
        windows::run_service()
    }
    #[cfg(not(windows))]
    {
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

/// 显式非SYSTEM调试入口。状态与管道必须由调用者选择隔离位置。
pub fn run_console(state_dir: PathBuf, pipe_name: &str, stop: Arc<AtomicBool>) -> Result<()> {
    #[cfg(windows)]
    {
        windows::run_console(state_dir, pipe_name, stop)
    }
    #[cfg(not(windows))]
    {
        let _ = (state_dir, pipe_name, stop);
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

/// 调试客户端仍核对管道宿主进程SID，仅允许本账户；不可用来信任任意系统服务名。
pub fn request_console(request: &Request, pipe_name: &str) -> Result<Response> {
    #[cfg(windows)]
    {
        let sid = windows::process_sid()?;
        windows::request_to(request, pipe_name, Some(&sid))
    }
    #[cfg(not(windows))]
    {
        let _ = (request, pipe_name);
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

pub fn audit_empty() -> Result<()> {
    #[cfg(windows)]
    {
        windows::audit_empty()
    }
    #[cfg(not(windows))]
    {
        bail!(ErrorCode::ProtectionUnsupported)
    }
}

fn decode_request(bytes: &[u8]) -> Result<Request> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        bail!(ErrorCode::InvalidInput);
    }
    // serde内部tag的unit variant可能忽略多余字段；先收紧每种命令的键集合。
    let mut value: Value = serde_json::from_slice(bytes).context(ErrorCode::InvalidInput)?;
    let command = value
        .get("command")
        .and_then(Value::as_object)
        .context(ErrorCode::InvalidInput)?;
    let allowed: &[&str] = match command.get("action").and_then(Value::as_str) {
        Some("status" | "list" | "uninstall_audit") => &["action"],
        Some("check_local") => &["action", "path"],
        Some("enroll") => &["action", "path", "mode", "note", "password"],
        Some("unlock") => &["action", "id", "password", "duration_seconds"],
        Some("relock") => &["action", "id"],
        Some("remove") => &["action", "id", "password"],
        Some("set_password") => &["action", "id", "password", "new_password"],
        Some("set_continuous") => &["action", "id", "password", "enabled"],
        _ => bail!(ErrorCode::InvalidInput),
    };
    let unknown_key = command.keys().any(|key| !allowed.contains(&key.as_str()));
    fn clear_strings(value: &mut Value) {
        match value {
            Value::String(text) => text.zeroize(),
            Value::Array(items) => items.iter_mut().for_each(clear_strings),
            Value::Object(fields) => fields.values_mut().for_each(clear_strings),
            _ => {}
        }
    }
    clear_strings(&mut value);
    if unknown_key {
        bail!(ErrorCode::InvalidInput);
    }
    let request: Request = serde_json::from_slice(bytes).context(ErrorCode::InvalidInput)?;
    if request.version != PROTOCOL_VERSION {
        bail!(ErrorCode::UnsupportedFormat);
    }
    request.command.validate()?;
    Ok(request)
}

#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    fn available_response() -> Response {
        Response {
            version: PROTOCOL_VERSION,
            transport: TransportStatus {
                host: "service".to_owned(),
                healthy: true,
                last_tick_error: None,
            },
            data: Some(serde_json::json!({"available": true})),
            error: None,
        }
    }

    #[test]
    fn uninstall_audit_requires_strict_all_account_response() {
        assert_eq!(
            uninstall_audit_using(|| Ok(false), || panic!("absent service must not connect"))
                .unwrap(),
            None
        );
        let response = || {
            let mut response = available_response();
            response.data = Some(serde_json::json!({"registered": 3, "has_errors": true}));
            response
        };
        assert_eq!(
            uninstall_audit_using(|| Ok(true), || Ok(response())).unwrap(),
            Some(UninstallAudit {
                registered: 3,
                has_errors: true
            })
        );
        for data in [
            serde_json::json!({"registered": 0}),
            serde_json::json!({"registered": -1, "has_errors": false}),
            serde_json::json!({"registered": 0, "has_errors": false, "sid": "secret"}),
            serde_json::Value::Null,
        ] {
            let mut invalid = response();
            invalid.data = Some(data);
            assert!(uninstall_audit_using(|| Ok(true), || Ok(invalid)).is_err());
        }
        for invalid in 0..5 {
            let mut value = response();
            match invalid {
                0 => value.version += 1,
                1 => value.transport.host = "console".to_owned(),
                2 => value.transport.healthy = false,
                3 => value.transport.last_tick_error = Some(ErrorCode::Recovery),
                _ => value.error = Some(ErrorCode::InvalidInput),
            }
            assert_eq!(
                error_code(&uninstall_audit_using(|| Ok(true), || Ok(value)).unwrap_err()),
                ErrorCode::ServiceUnavailable
            );
        }
        assert!(uninstall_audit_using(|| bail!("SCM denied"), || Ok(response())).is_err());
        assert!(uninstall_audit_using(|| Ok(true), || bail!("pipe disconnected")).is_err());
        assert!(decode_request(br#"{"version":1,"command":{"action":"uninstall_audit"}}"#).is_ok());
        assert!(
            decode_request(
                br#"{"version":1,"command":{"action":"uninstall_audit","sid":"other"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn local_overlap_check_only_skips_confirmed_absent_service() {
        let paths = [
            PathBuf::from(r"C:\missing\ordinary"),
            PathBuf::from(r"\\?\C:\missing\another"),
        ];
        let mut queries = 0;
        assert!(
            ensure_local_available_using(
                &paths,
                || Ok(false),
                |_| {
                    queries += 1;
                    bail!("should not query");
                }
            )
            .is_ok()
        );
        assert_eq!(queries, 0);
        let error = ensure_local_available_using(
            &paths,
            || bail!("SCM query denied"),
            |_| Ok(available_response()),
        )
        .unwrap_err();
        assert_eq!(error_code(&error), ErrorCode::ServiceUnavailable);
        let error =
            ensure_local_available_using(&paths, || Ok(true), |_| bail!("pipe disconnected"))
                .unwrap_err();
        assert_eq!(error_code(&error), ErrorCode::ServiceUnavailable);
        let error = ensure_local_available_using(
            &paths,
            || Err(std::io::Error::from_raw_os_error(5).into()),
            |_| Ok(available_response()),
        )
        .unwrap_err();
        assert_eq!(
            error_code(&error),
            ErrorCode::ServiceUnavailable,
            "SCM访问拒绝必须保留服务不可用类别"
        );
        let error = ensure_local_available_using(
            &paths,
            || Ok(true),
            |_| Err(std::io::Error::from_raw_os_error(5).into()),
        )
        .unwrap_err();
        assert_eq!(
            error_code(&error),
            ErrorCode::ServiceUnavailable,
            "管道身份或访问拒绝不能当作范围可用"
        );
        assert!(
            ensure_local_available_using(
                &paths,
                || Ok(true),
                |_| {
                    queries += 1;
                    Ok(available_response())
                }
            )
            .is_ok()
        );
        assert_eq!(queries, paths.len());
    }

    #[test]
    fn local_overlap_check_requires_strict_trusted_healthy_response() {
        let paths = [PathBuf::from(r"C:\ordinary")];
        let mut invalid = Vec::new();
        let mut response = available_response();
        response.version += 1;
        invalid.push(response);
        let mut response = available_response();
        response.transport.host = "console".to_owned();
        invalid.push(response);
        let mut response = available_response();
        response.transport.healthy = false;
        invalid.push(response);
        let mut response = available_response();
        response.transport.last_tick_error = Some(ErrorCode::Recovery);
        invalid.push(response);
        let mut response = available_response();
        response.data = Some(serde_json::json!({"available": true, "path":"secret"}));
        invalid.push(response);
        let mut response = available_response();
        response.data = Some(serde_json::json!({"available": false}));
        invalid.push(response);
        let mut response = available_response();
        response.data = None;
        invalid.push(response);
        let mut response = available_response();
        response.error = Some(ErrorCode::ProtectionServiceOverlap);
        invalid.push(response);
        let mut response = available_response();
        response.data = None;
        response.error = Some(ErrorCode::InvalidInput);
        invalid.push(response);
        for response in invalid {
            let mut response = Some(response);
            let result =
                ensure_local_available_using(&paths, || Ok(true), |_| Ok(response.take().unwrap()));
            assert_eq!(
                error_code(&result.unwrap_err()),
                ErrorCode::ServiceUnavailable
            );
        }
        let mut response = available_response();
        response.data = None;
        response.error = Some(ErrorCode::ProtectionServiceOverlap);
        let mut response = Some(response);
        let result =
            ensure_local_available_using(&paths, || Ok(true), |_| Ok(response.take().unwrap()));
        assert_eq!(
            error_code(&result.unwrap_err()),
            ErrorCode::ProtectionServiceOverlap
        );
    }

    #[test]
    fn local_check_path_validation_is_lexical_and_rejects_ambiguous_forms() {
        for path in [
            r"C:\",
            r"C:\does-not-exist\child",
            r"\\?\c:\does-not-exist\child",
        ] {
            assert!(valid_local_check_path(Path::new(path)), "{path}");
        }
        for path in [
            r"C:relative",
            r"relative",
            r"\rooted",
            r"\\server\share",
            r"\\.\C:\root",
            r"C:\a\..\b",
            r"C:\a\.\b",
            r"C:\a\\b",
            r"C:\a\",
            r"C:\a.\b",
            r"C:\a \b",
            r"C:\a:stream",
            r"C:/a/b",
            r"\\?\UNC\server\share",
            r"C:\a\*",
        ] {
            assert!(!valid_local_check_path(Path::new(path)), "{path}");
        }
        let invalid = [PathBuf::from(r"C:\a\..\b")];
        assert!(
            ensure_local_available_using(
                &invalid,
                || panic!("invalid path must fail before SCM"),
                |_| panic!("invalid path must not query")
            )
            .is_err()
        );
        assert!(
            decode_request(
                br#"{"version":1,"command":{"action":"check_local","path":"C:\\missing\\child"}}"#
            )
            .is_ok()
        );
        for request in [
            br#"{"version":1,"command":{"action":"check_local","path":"C:\\a\\..\\b"}}"#.as_slice(),
            br#"{"version":1,"command":{"action":"check_local","path":"C:\\a","sid":"S-1-5-18"}}"#
                .as_slice(),
            br#"{"version":1,"command":{"action":"check_local","path":"C:\\a","original":"D:P"}}"#
                .as_slice(),
        ] {
            assert!(decode_request(request).is_err());
        }
    }

    #[test]
    fn wire_rejects_sid_state_acl_unknown_fields_and_invalid_duration() {
        for value in [
            r#"{"version":1,"sid":"S-1-5-18","command":{"action":"list"}}"#,
            r#"{"version":1,"command":{"action":"list","state_path":"elsewhere"}}"#,
            r#"{"version":1,"command":{"action":"enroll","path":"C:\\a","mode":"locked","password":null,"original":"D:P"}}"#,
            r#"{"version":1,"command":{"action":"unlock","id":"a","password":"password","duration_seconds":1}}"#,
        ] {
            assert!(decode_request(value.as_bytes()).is_err());
        }
        assert!(decode_request(br#"{"version":1,"command":{"action":"list"}}"#).is_ok());
        assert!(decode_request(&vec![0; MAX_FRAME_BYTES + 1]).is_err());
    }
}
