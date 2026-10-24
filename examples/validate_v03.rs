//! 多阶段真实进程中断验证；只操作自己的合成夹具与子进程。
use anyhow::{Context, Result, ensure};
use clap::Parser;
use foldsentinel::vault::{self, Control};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    report: Option<PathBuf>,
    #[arg(long, hide = true)]
    worker: bool,
    #[arg(long, hide = true)]
    root: Option<PathBuf>,
    #[arg(long, hide = true)]
    phase: Option<String>,
    #[arg(long, hide = true)]
    decrypt: bool,
}
// 公开合成测试密码，不是用户凭据。
const PASSWORD: &str = "v03-synthetic-process-validation";

fn main() -> Result<()> {
    let args = Args::parse();
    if args.worker {
        let root = args.root.context("worker root")?;
        let phase = args.phase.context("worker phase")?;
        let ready = root.join("ready");
        let c = Control::with_recovery_dir(root.join("jobs"), move |p| {
            if p.phase == phase {
                fs::write(&ready, b"worker reached phase").expect("write ready marker");
                loop {
                    std::thread::park();
                }
            }
        });
        if args.decrypt {
            vault::decrypt_with_control(&root.join("sealed"), &root.join("output"), PASSWORD, &c)?;
        } else {
            vault::encrypt_with_control(&root.join("source"), &root.join("output"), PASSWORD, &c)?;
        }
        anyhow::bail!("worker unexpectedly completed");
    }
    let report = args.report.context("provide --report")?;
    ensure!(!report.exists(), "report already exists");
    let mut results = Vec::new();
    for decrypt in [false, true] {
        for phase in if decrypt {
            ["deriving", "decrypting", "syncing", "committing"]
        } else {
            ["deriving", "encrypting", "syncing", "committing"]
        } {
            let dir = tempfile::Builder::new()
                .prefix("foldsentinel-v03-crash-")
                .tempdir()?;
            let root = dir.path();
            let bytes = vec![37u8; 3 * 1024 * 1024 + 13];
            fs::write(root.join("source"), &bytes)?;
            let control = || Control::with_recovery_dir(root.join("jobs"), |_| {});
            if decrypt {
                vault::encrypt_with_control(
                    &root.join("source"),
                    &root.join("sealed"),
                    PASSWORD,
                    &control(),
                )?;
            }
            let mut command = Command::new(std::env::current_exe()?);
            command
                .args(["--worker", "--root"])
                .arg(root)
                .args(["--phase", phase])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if decrypt {
                command.arg("--decrypt");
            }
            let mut child = command.spawn()?;
            let deadline = Instant::now() + Duration::from_secs(30);
            let ready = loop {
                if root.join("ready").exists() {
                    break true;
                }
                if let Some(status) = child.try_wait()? {
                    anyhow::bail!("worker exited early: {status}");
                }
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            child.kill()?;
            child.wait()?;
            ensure!(ready, "worker phase timed out");
            ensure!(!root.join("output").exists(), "interrupted output exists");
            let jobs = vault::recovery::interrupted_jobs(&root.join("jobs"))?;
            ensure!(jobs.len() == 1, "expected one interrupted record");
            vault::recovery::cleanup_interrupted(&root.join("jobs"), &jobs[0].id)?;
            ensure!(!jobs[0].staging.exists(), "staging not cleaned");
            ensure!(fs::read(root.join("source"))? == bytes, "source changed");
            if decrypt {
                vault::decrypt_with_control(
                    &root.join("sealed"),
                    &root.join("output"),
                    PASSWORD,
                    &control(),
                )?;
                ensure!(
                    fs::read(root.join("output"))? == bytes,
                    "restore retry differs"
                );
            } else {
                vault::encrypt_with_control(
                    &root.join("source"),
                    &root.join("output"),
                    PASSWORD,
                    &control(),
                )?;
                vault::verify_with_control(&root.join("output"), PASSWORD, &control())?;
                vault::decrypt_with_control(
                    &root.join("output"),
                    &root.join("restored"),
                    PASSWORD,
                    &control(),
                )?;
                ensure!(
                    fs::read(root.join("restored"))? == bytes,
                    "encrypt retry differs"
                );
            }
            ensure!(
                vault::recovery::interrupted_jobs(&root.join("jobs"))?.is_empty(),
                "leftover record"
            );
            results.push(json!({"operation":if decrypt {"decrypt"} else {"encrypt"},"phase":phase,"real_child_killed":true,"source_unchanged":true,"no_committed_output":true,"owned_cleanup":true,"retry_matches":true}));
            println!(
                "{} {phase}: passed",
                if decrypt { "decrypt" } else { "encrypt" }
            );
            dir.close()?;
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report)?;
    serde_json::to_writer_pretty(
        &mut file,
        &json!({"version":"0.3.0","cases":results,"temporary_fixtures_removed":true}),
    )?;
    file.sync_all()?;
    Ok(())
}
