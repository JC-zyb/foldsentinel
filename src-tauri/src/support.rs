use anyhow::{Context, Result, ensure};
use foldsentinel::vault::{ErrorCode, workflow::Operation};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

// 诊断采用封闭字段，不接收密码、任意错误文本或本机路径。
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRequest {
    pub code: ErrorCode,
    pub operation: Option<Operation>,
    pub os_codes: Vec<i32>,
}

#[derive(Serialize)]
struct DiagnosticReport<'a> {
    schema: u8,
    app_version: &'static str,
    os: &'static str,
    arch: &'static str,
    timestamp_ms: u128,
    paths_redacted: bool,
    failure: &'a DiagnosticRequest,
}

pub fn diagnostic_report(request: &DiagnosticRequest) -> Result<String> {
    ensure!(
        request.os_codes.len() <= 8
            && request
                .os_codes
                .iter()
                .all(|code| (0..=65535).contains(code)),
        ErrorCode::InvalidInput
    );
    Ok(serde_json::to_string_pretty(&DiagnosticReport {
        schema: 1,
        app_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        paths_redacted: true,
        failure: request,
    })?)
}

pub fn export_diagnostic(path: &Path, request: &DiagnosticRequest) -> Result<()> {
    let report = diagnostic_report(request)?;
    ensure!(
        path.is_absolute()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json")),
        ErrorCode::InvalidInput
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context(ErrorCode::WriteFailed)?;
    file.write_all(report.as_bytes())
        .context(ErrorCode::WriteFailed)?;
    file.sync_all().context(ErrorCode::WriteFailed)
}

pub fn output_folder(path: &Path) -> Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&path)?;
    if metadata.is_dir() {
        Ok(path)
    } else {
        ensure!(metadata.is_file(), ErrorCode::InvalidInput);
        Ok(path
            .parent()
            .context(ErrorCode::InvalidInput)?
            .to_path_buf())
    }
}

pub fn open_output_folder(path: &Path) -> Result<()> {
    let folder = output_folder(path)?;
    #[cfg(windows)]
    {
        let explorer = PathBuf::from(std::env::var_os("WINDIR").context(ErrorCode::InvalidInput)?)
            .join("explorer.exe");
        // 只启动系统资源管理器并传入目录，路径不参与 Shell 或命令文本拼接。
        std::process::Command::new(explorer)
            .arg(folder)
            .spawn()
            .context(ErrorCode::Internal)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = folder;
        anyhow::bail!(ErrorCode::UnsupportedFormat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_schema_rejects_secrets_and_contains_no_paths() {
        let request: DiagnosticRequest = serde_json::from_str(
            r#"{"code":"permission_denied","operation":"decrypt","os_codes":[5]}"#,
        )
        .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&diagnostic_report(&request).unwrap()).unwrap();
        assert_eq!(report["paths_redacted"], true);
        assert_eq!(report["failure"]["os_codes"][0], 5);
        assert!(
            serde_json::from_str::<DiagnosticRequest>(
                r#"{"code":"read_failed","operation":null,"os_codes":[],"password":"secret"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<DiagnosticRequest>(
                r#"{"code":"read_failed","operation":null,"os_codes":[],"path":"C:\\private"}"#
            )
            .is_err()
        );
        assert!(
            diagnostic_report(&DiagnosticRequest {
                code: ErrorCode::Internal,
                operation: None,
                os_codes: vec![0; 9]
            })
            .is_err()
        );
    }

    #[test]
    fn diagnostic_export_is_explicit_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let request = DiagnosticRequest {
            code: ErrorCode::WriteFailed,
            operation: Some(Operation::Encrypt),
            os_codes: vec![112],
        };
        let path = dir.path().join("report.json");
        export_diagnostic(&path, &request).unwrap();
        let original = std::fs::read(&path).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(json["failure"]["os_codes"][0], 112);
        assert_eq!(json["app_version"], env!("CARGO_PKG_VERSION"));
        assert!(
            !String::from_utf8_lossy(&original).contains(&dir.path().to_string_lossy().to_string())
        );
        assert!(export_diagnostic(&path, &request).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(export_diagnostic(Path::new("relative.json"), &request).is_err());
        assert!(export_diagnostic(&dir.path().join("not-json.txt"), &request).is_err());
        assert_eq!(
            output_folder(dir.path()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert_eq!(
            output_folder(&dir.path().join("report.json")).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert!(output_folder(&dir.path().join("missing")).is_err());
    }
}
