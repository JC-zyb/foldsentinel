//! 新版本读取真实旧版进程中断记录；仅操作自有临时目录和子进程。
use anyhow::{Context, Result, ensure};
use clap::Parser;
use foldsentinel::vault::{self, Control, recovery};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Args {
    /// 从旧版本源码构建的 validate_v03 worker，不接受命令字符串。
    #[arg(long)]
    baseline_worker: PathBuf,
    #[arg(long)]
    report: PathBuf,
}

// 旧版验证程序的公开测试口令，不是用户凭据。
const PASSWORD: &str = "v03-synthetic-process-validation";

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(!args.report.exists(), "报告已存在，拒绝覆盖");
    let worker = fs::canonicalize(&args.baseline_worker)?;
    let worker_hash = format!("{:x}", Sha256::digest(fs::read(&worker)?));
    let mut checks = Vec::new();
    for decrypt in [false, true] {
        let temporary = tempfile::Builder::new()
            .prefix("foldsentinel-upgrade-")
            .tempdir()?;
        let root = temporary.path();
        let content = vec![37; 64 * 1024 + 13];
        fs::write(root.join("source"), &content)?;
        let control = || Control::with_recovery_dir(root.join("jobs"), |_| {});
        if decrypt {
            vault::encrypt_with_control(
                &root.join("source"),
                &root.join("sealed"),
                PASSWORD,
                &control(),
            )?;
        }
        let mut command = Command::new(&worker);
        command
            .args(["--worker", "--root"])
            .arg(root)
            .args(["--phase", "committing"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if decrypt {
            command.arg("--decrypt");
        }
        let mut child = command.spawn()?;
        let ready = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !root.join("ready").exists() {
                ensure!(child.try_wait()?.is_none(), "旧版 worker 提前退出");
                ensure!(Instant::now() < deadline, "旧版 worker 等待超时");
                std::thread::sleep(Duration::from_millis(20));
            }
            let active = recovery::inspect_jobs(&root.join("jobs"))?;
            ensure!(
                active.len() == 1 && active[0].status == recovery::RecoveryStatus::Active,
                "新版本没有识别旧版活动锁"
            );
            Ok(())
        })();
        // 无论观察成功与否都结束本次自有 worker，避免遗留挂起进程。
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
        ready?;
        ensure!(!root.join("output").exists(), "中断任务已经提交输出");
        let entries = recovery::inspect_jobs(&root.join("jobs"))?;
        ensure!(
            entries.len() == 1 && entries[0].status == recovery::RecoveryStatus::Cleanable,
            "旧版中断记录不可识别"
        );
        let records = recovery::interrupted_jobs(&root.join("jobs"))?;
        ensure!(
            records.len() == 1 && records[0].decrypt == decrypt,
            "旧版记录方向错误"
        );
        recovery::cleanup_interrupted(&root.join("jobs"), &records[0].id)?;
        ensure!(!records[0].staging.exists(), "自有旧版 staging 未清理");
        ensure!(
            recovery::inspect_jobs(&root.join("jobs"))?.is_empty(),
            "旧版登记未清理"
        );
        ensure!(fs::read(root.join("source"))? == content, "源内容改变");
        let sealed = if decrypt {
            root.join("sealed")
        } else {
            vault::encrypt_with_control(
                &root.join("source"),
                &root.join("output"),
                PASSWORD,
                &control(),
            )?;
            root.join("output")
        };
        vault::decrypt_with_control(&sealed, &root.join("restored"), PASSWORD, &control())?;
        ensure!(
            fs::read(root.join("restored"))? == content,
            "清理后重试内容不一致"
        );
        checks.push(json!({"operation":if decrypt {"decrypt"} else {"encrypt"},
            "active_lock_recognized":true,"real_old_worker_killed":true,
            "old_journal_readable":true,"no_committed_output":true,
            "owned_cleanup":true,"source_unchanged":true,"retry_matches":true}));
    }
    let report = json!({"schema_version":1,"version":env!("CARGO_PKG_VERSION"),
        "baseline_worker_sha256":worker_hash,"passed":true,"checks":checks,
        "limitations":["Caller must record the old worker source revision separately.",
            "Isolated core journal compatibility; not an NSIS install or clean Windows VM test."]});
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(args.report)?;
    serde_json::to_writer_pretty(&mut file, &report).context("写入报告失败")?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    println!("旧版加密和恢复中断记录兼容验证通过");
    Ok(())
}
