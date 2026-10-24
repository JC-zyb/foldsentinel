//! Windows 真实 IO 验证。卷写满模式只允许带一次性标记、容量 <= 128 MiB 的隔离卷。
#[cfg(not(windows))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("仅支持 Windows 验证");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    validation::run()
}

#[cfg(windows)]
mod validation {
    use anyhow::{Context, Result, ensure};
    use clap::Parser;
    use foldsentinel::vault::{self, Control, ErrorCode};
    use serde_json::json;
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
        path::{Path, PathBuf},
        process::Command,
        sync::{Arc, Mutex},
    };

    const PASSWORD: &str = "v04-public-synthetic-fixture";
    const MIB: u64 = 1024 * 1024;
    #[derive(Parser)]
    struct Args {
        #[arg(long)]
        report: PathBuf,
        #[arg(long, requires = "volume_token")]
        volume_root: Option<PathBuf>,
        #[arg(long, requires = "volume_root")]
        volume_token: Option<String>,
    }

    fn control(root: &Path) -> Control {
        Control::with_recovery_dir(root.join("jobs"), |_| {})
    }
    fn failed(result: Result<vault::VaultReport>, code: ErrorCode, output: &Path) -> Result<()> {
        let error = result.expect_err("测试操作意外成功");
        ensure!(vault::error_code(&error) == code, "错误类别不符: {error:#}");
        ensure!(!output.exists(), "失败后生成了最终输出");
        Ok(())
    }
    fn icacls(path: &Path, operation: &str, sid: &str) -> Result<()> {
        let status = Command::new("icacls.exe")
            .arg(path)
            .args([operation, sid, "/q"])
            .output()?;
        ensure!(
            status.status.success(),
            "私有测试目录权限操作失败: {}",
            status.status
        );
        Ok(())
    }

    pub fn run() -> Result<()> {
        let args = Args::parse();
        ensure!(!args.report.exists(), "报告已存在，拒绝覆盖");
        let temp = tempfile::Builder::new()
            .prefix("foldsentinel-v04-io-")
            .tempdir()?;
        let root = temp.path();
        let source = root.join("source");
        let bytes = vec![0x37; 3 * MIB as usize + 13];
        fs::write(&source, &bytes)?;
        let sealed = root.join("sealed.fse");
        vault::encrypt_with_control(&source, &sealed, PASSWORD, &control(root))?;
        let mut cases = Vec::new();

        // 只在新建私有目录添加当前用户写入拒绝项，测试后显式移除。
        let who = Command::new("whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()?;
        ensure!(who.status.success(), "无法确定测试用户 SID");
        let who = String::from_utf8(who.stdout)?;
        let sid = who
            .trim()
            .rsplit(',')
            .next()
            .context("SID 缺失")?
            .trim_matches('"');
        ensure!(
            sid.starts_with("S-1-")
                && sid
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == 'S' || c == '-'),
            "SID 无效"
        );
        let denied = root.join("denied");
        fs::create_dir(&denied)?;
        icacls(&denied, "/deny", &format!("*{sid}:(OI)(CI)(W)"))?;
        let denied_result = (|| -> Result<()> {
            for decrypt in [false, true] {
                let output = denied.join(if decrypt { "restored" } else { "sealed" });
                let result = if decrypt {
                    vault::decrypt_with_control(&sealed, &output, PASSWORD, &control(root))
                } else {
                    vault::encrypt_with_control(&source, &output, PASSWORD, &control(root))
                };
                failed(result, ErrorCode::PermissionDenied, &output)?;
                cases.push(
                    json!({"case": "target_write_denied", "decrypt": decrypt, "passed": true}),
                );
            }
            Ok(())
        })();
        icacls(&denied, "/remove:d", &format!("*{sid}"))?;
        denied_result?;

        for decrypt in [false, true] {
            let input = if decrypt { &sealed } else { &source };
            let lock = OpenOptions::new().read(true).share_mode(0).open(input)?;
            let output = root.join("locked-output");
            let result = if decrypt {
                vault::decrypt_with_control(input, &output, PASSWORD, &control(root))
            } else {
                vault::encrypt_with_control(input, &output, PASSWORD, &control(root))
            };
            ensure!(result.is_err() && !output.exists(), "独占占用未阻止操作");
            drop(lock);
            cases.push(
                json!({"case": "source_exclusive_handle", "decrypt": decrypt, "passed": true}),
            );
        }

        // 预处理与执行之间目标目录失效；不能生成最终输出。
        for decrypt in [false, true] {
            let parent = root.join("moving");
            fs::create_dir(&parent)?;
            let moved = root.join("moved");
            let input = if decrypt { &sealed } else { &source };
            let operation = if decrypt {
                vault::workflow::Operation::Decrypt
            } else {
                vault::workflow::Operation::Encrypt
            };
            let plan = vault::workflow::prepare(
                std::slice::from_ref(input),
                operation,
                Some(&parent),
                &[],
            )?;
            let output = plan[0].output.as_ref().context("预处理未提供输出")?;
            fs::rename(&parent, &moved)?;
            let result = if decrypt {
                vault::decrypt_with_control(input, output, PASSWORD, &control(root))
            } else {
                vault::encrypt_with_control(input, output, PASSWORD, &control(root))
            };
            ensure!(result.is_err() && !output.exists(), "失效目标未阻止执行");
            fs::rename(&moved, &parent)?;
            vault::decrypt_with_control(&sealed, output, PASSWORD, &control(root))?;
            ensure!(fs::read(output)? == bytes, "路径恢复后重试不一致");
            fs::remove_file(output)?;
            fs::remove_dir(parent)?;
            cases.push(json!({"case": "target_parent_missing_after_prepare", "decrypt": decrypt, "passed": true}));
        }

        // 在提交门持有真实共享句柄，阻止改名和清理；失败必须保留登记供解除占用后处理。
        for decrypt in [false, true] {
            let held = Arc::new(Mutex::new(None));
            let capture = held.clone();
            let registry = root.join("jobs");
            let c = Control::with_recovery_dir(registry.clone(), move |progress| {
                if progress.phase == "committing" {
                    let entries =
                        vault::recovery::inspect_jobs(&registry).expect("读取私有任务登记");
                    let record = entries[0].record.as_ref().expect("有效任务记录");
                    *capture.lock().unwrap() = Some(
                        OpenOptions::new()
                            .read(true)
                            .share_mode(3)
                            .open(record.staging.join("payload"))
                            .expect("占用待提交文件"),
                    );
                }
            });
            let output = root.join("commit-locked-output");
            let result = if decrypt {
                vault::decrypt_with_control(&sealed, &output, PASSWORD, &c)
            } else {
                vault::encrypt_with_control(&source, &output, PASSWORD, &c)
            };
            failed(result, ErrorCode::Recovery, &output)?;
            let entries = vault::recovery::inspect_jobs(&root.join("jobs"))?;
            ensure!(
                entries.len() == 1
                    && entries[0].status == vault::recovery::RecoveryStatus::Cleanable,
                "清理失败未保留登记"
            );
            ensure!(
                vault::recovery::cleanup_interrupted(&root.join("jobs"), &entries[0].id).is_err(),
                "占用期间不应清理成功"
            );
            ensure!(
                root.join("jobs")
                    .join(format!("{}.json", entries[0].id))
                    .exists(),
                "失败清理丢失记录"
            );
            drop(held.lock().unwrap().take());
            vault::recovery::cleanup_interrupted(&root.join("jobs"), &entries[0].id)?;
            vault::decrypt_with_control(&sealed, &output, PASSWORD, &control(root))?;
            ensure!(fs::read(&output)? == bytes, "清理后重试不一致");
            fs::remove_file(output)?;
            cases.push(json!({"case": "commit_and_cleanup_locked_then_retry", "decrypt": decrypt, "passed": true}));
        }

        if let Some(volume) = args.volume_root {
            let token = args.volume_token.context("缺少隔离卷标记")?;
            ensure!(
                fs::read_to_string(volume.join("foldsentinel-test-volume"))? == token,
                "隔离卷标记不匹配"
            );
            let (_, total) = space(&volume)?;
            ensure!(
                (32 * MIB..=128 * MIB).contains(&total),
                "仅允许 <=128 MiB 的小容量隔离卷"
            );
            for decrypt in [false, true] {
                let target = tempfile::Builder::new()
                    .prefix("v04-full-")
                    .tempdir_in(&volume)?;
                let filler = target.path().join("filler");
                let copied = filler.clone();
                let fill_result = Arc::new(Mutex::new(None));
                let capture = fill_result.clone();
                let c = Control::with_recovery_dir(root.join("jobs"), move |progress| {
                    if progress.phase == if decrypt { "decrypting" } else { "deriving" } {
                        let mut result = capture.lock().unwrap();
                        if result.is_none() {
                            *result = Some(fill_to_capacity(&copied));
                        }
                    }
                });
                let output = target.path().join("output");
                let result = if decrypt {
                    vault::decrypt_with_control(&sealed, &output, PASSWORD, &c)
                } else {
                    vault::encrypt_with_control(&source, &output, PASSWORD, &c)
                };
                fill_result
                    .lock()
                    .unwrap()
                    .take()
                    .context("未触发运行中卷填满")??;
                let error = result.expect_err("写满卷后操作意外成功");
                ensure!(
                    vault::error_code(&error) == ErrorCode::InsufficientSpace,
                    "磁盘满类别不符: {error:#}"
                );
                ensure!(
                    error.chain().any(|cause| cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| matches!(io.raw_os_error(), Some(39 | 112)))),
                    "未观察到实际 OS 磁盘满错误"
                );
                ensure!(!output.exists(), "磁盘满时提交了输出");
                fs::remove_file(filler)?;
                ensure!(
                    vault::recovery::inspect_jobs(&root.join("jobs"))?.is_empty(),
                    "磁盘满清理留下记录"
                );
                vault::decrypt_with_control(&sealed, &output, PASSWORD, &control(root))?;
                ensure!(fs::read(output)? == bytes, "释放空间后重试内容不同");
                cases.push(json!({"case": "real_volume_full_after_preflight", "decrypt": decrypt, "passed": true}));
            }
        }
        ensure!(fs::read(&source)? == bytes, "源数据变化");
        let report = json!({"version": env!("CARGO_PKG_VERSION"), "platform": "windows-x64", "cases": cases, "source_preserved": true});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args.report)?;
        file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
        file.sync_all()?;
        println!("Windows IO verification passed: {} cases", cases.len());
        Ok(())
    }

    fn space(path: &Path) -> Result<(u64, u64)> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut free, mut total) = (0, 0);
        // NUL 结尾路径与两个输出缓冲在调用期间有效。
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                &mut total,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error()).context("读取隔离卷容量");
        }
        Ok((free, total))
    }

    fn fill_to_capacity(path: &Path) -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let buffer = vec![0x75; MIB as usize];
        for _ in 0..=128 {
            match file.write_all(&buffer) {
                Ok(()) => {}
                Err(error) if matches!(error.raw_os_error(), Some(39 | 112)) => return Ok(()),
                Err(error) => return Err(error).context("填充隔离卷失败"),
            }
        }
        anyhow::bail!("超过隔离卷容量上限，停止写入");
    }
}
