use anyhow::{Context, Result, ensure};
use foldsentinel::{AttributeState, ProtectionMode, StateStore};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let mut args = std::env::args_os().skip(1);
        ensure!(
            args.next().as_deref() == Some(std::ffi::OsStr::new("--report")),
            "需要 --report <新报告.json>"
        );
        let report = PathBuf::from(args.next().context("缺少报告路径")?);
        ensure!(args.next().is_none(), "不支持的参数");
        let mut report_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(report)?;
        let root = tempfile::tempdir()?;
        let source = root.path().join("资料");
        fs::create_dir_all(source.join("空目录"))?;
        let file = source.join("内容.txt");
        fs::write(&file, b"public attribute validation")?;
        let original = fs::metadata(&file)?.file_attributes();
        let state = root.path().join("state.json");
        let mut store = StateStore::load_from(state.clone())?;
        store.protect(&source, ProtectionMode::Hidden)?;
        let saved = fs::read(&state)?;
        let statuses = store.statuses(None, 0, 50)?;
        ensure!(
            statuses.len() == 3
                && statuses
                    .iter()
                    .all(|entry| entry.state == AttributeState::Consistent),
            "递归隐藏或状态检查失败"
        );
        let hidden = fs::metadata(&file)?.file_attributes();
        ensure!(hidden & 0x6 == 0x6, "实际 Hidden/System 位不完整");
        ensure!(fs::read(&state)? == saved, "刷新修改了恢复记录");
        drop(store);
        let mut store = StateStore::load_from(state.clone())?;
        let roots = store.roots();
        ensure!(
            roots.len() == 1 && roots[0].entry_count == 3,
            "重启后最外层路径或数量丢失"
        );
        ensure!(
            Path::new(&roots[0].path) == fs::canonicalize(&source)?,
            "保存的最外层路径错误"
        );
        ensure!(fs::read(&state)? == saved, "重启加载改写了恢复记录");

        // 只改自有夹具，模拟其他程序移除 System 属性。
        {
            use std::iter::once;
            use std::os::windows::ffi::OsStrExt;
            let wide: Vec<_> = file.as_os_str().encode_wide().chain(once(0)).collect();
            ensure!(
                unsafe {
                    windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
                        wide.as_ptr(),
                        hidden & !0x4,
                    )
                } != 0,
                "修改自有夹具属性失败: {}",
                std::io::Error::last_os_error()
            );
        }
        ensure!(
            store.statuses(Some(&file), 0, 1)?[0].state == AttributeState::Changed,
            "未识别外部属性差异"
        );
        store.unprotect(&PathBuf::from(&roots[0].path))?;
        ensure!(
            fs::metadata(&file)?.file_attributes() == original,
            "原始属性未恢复"
        );
        store.protect(&file, ProtectionMode::Readonly)?;
        let write_error = OpenOptions::new()
            .append(true)
            .open(&file)
            .err()
            .context("只读文件不应允许写入")?;
        ensure!(
            write_error.kind() == std::io::ErrorKind::PermissionDenied,
            "只读写入返回意外错误: {write_error}"
        );
        store.unprotect(&file)?;
        ensure!(
            fs::read(&file)? == b"public attribute validation",
            "源内容改变"
        );
        ensure!(store.entries().count() == 0, "恢复后仍有记录");

        report_file.write_all(serde_json::to_string_pretty(&json!({
            "version": env!("CARGO_PKG_VERSION"), "platform": "windows", "passed": true,
            "checks": ["recursive_hidden_system_flags", "readonly_status_refresh", "saved_root_reload_and_restore", "external_change_detection", "original_attribute_restore", "readonly_write_denied", "source_content_kept"],
            "observed_file_attributes": {"original": original, "hidden": hidden, "restored": fs::metadata(&file)?.file_attributes()},
            "limitations": ["Explorer visibility is validated separately in the real UI.", "No ACL, administrator, clean-VM or missing-WebView2 test is performed."]
        }))?.as_bytes())?;
        writeln!(report_file)?;
        println!("Windows 属性检查通过：7 项");
        Ok(())
    }
    #[cfg(not(windows))]
    anyhow::bail!("此验证仅支持 Windows")
}
