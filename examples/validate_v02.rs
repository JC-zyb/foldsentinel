//! 显式运行的大数据/真实进程中断验证；仅清理本程序创建的临时目录。
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use foldsentinel::vault::{self, Control};
use rand::RngCore;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// 仅用于生成的公开测试数据，不是用户密码。
const PASSWORD: &str = "synthetic-validation-only";
const MIB: usize = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// 生成并往返验证 4 GiB 文件与 10 GiB 文件夹，临时空间峰值约 30 GiB。
    Large { scratch: PathBuf, report: PathBuf },
    /// 真实杀死写入后的子进程，核对恢复记录、清理与重试。
    Crash { scratch: PathBuf, report: PathBuf },
    /// 留下一个中断恢复任务，用于桌面清理流程验证；只处理自动生成的夹具。
    PrepareUi { scratch: PathBuf },
    #[command(hide = true)]
    CrashChild {
        input: PathBuf,
        output: PathBuf,
        registry: PathBuf,
        ready: PathBuf,
    },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Action::Large { scratch, report } => large(&scratch, &report),
        Action::Crash { scratch, report } => crash(&scratch, &report),
        Action::PrepareUi { scratch } => {
            let temporary = tempfile::Builder::new()
                .prefix("foldsentinel-ui-validation-")
                .tempdir_in(scratch)?;
            let input = temporary.path().join("synthetic.bin");
            generate(&input, 32 * MIB as u64)?;
            let output = temporary.path().join("synthetic.fse");
            let registry = vault::recovery::default_recovery_dir()?;
            kill_at_sync(&input, &output, &registry, &temporary.path().join("ready"))?;
            println!("UI fixture retained: {}", temporary.keep().display());
            Ok(())
        }
        Action::CrashChild {
            input,
            output,
            registry,
            ready,
        } => {
            let c = Control::with_recovery_dir(registry, move |p| {
                if p.phase == "syncing" {
                    fs::write(&ready, b"payload written before commit")
                        .expect("write readiness marker");
                    loop {
                        std::thread::park();
                    }
                }
            });
            vault::encrypt_with_control(&input, &output, PASSWORD, &c)?;
            anyhow::bail!("child unexpectedly committed")
        }
    }
}

fn control(registry: &Path) -> Control {
    let last = Mutex::new(Instant::now());
    Control::with_recovery_dir(registry.to_path_buf(), move |p| {
        let mut last = last.lock().unwrap();
        if last.elapsed() >= Duration::from_secs(5)
            || matches!(p.phase.as_str(), "completed" | "committing")
        {
            eprintln!("{}: {} / {}", p.phase, p.completed, p.total);
            *last = Instant::now();
        }
    })
}

fn generate(path: &Path, size: u64) -> Result<String> {
    let mut block = vec![0; MIB];
    rand::rngs::OsRng.try_fill_bytes(&mut block)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut hash = Sha256::new();
    let mut remaining = size;
    while remaining > 0 {
        let count = remaining.min(block.len() as u64) as usize;
        file.write_all(&block[..count])?;
        hash.update(&block[..count]);
        remaining -= count as u64;
    }
    file.sync_all()?;
    Ok(format!("{:x}", hash.finalize()))
}

fn hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; MIB];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(windows)]
fn peak_memory() -> Result<usize> {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // 零初始化的输出结构和伪进程句柄均在同步调用期间有效。
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = size;
    if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } == 0 {
        return Err(std::io::Error::last_os_error()).context("读取进程峰值内存失败");
    }
    Ok(counters.PeakWorkingSetSize)
}
#[cfg(not(windows))]
fn peak_memory() -> Result<usize> {
    anyhow::bail!("此验证程序仅支持 Windows")
}

fn save(report: &Path, value: serde_json::Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report)?;
    file.write_all(serde_json::to_string_pretty(&value)?.as_bytes())?;
    file.sync_all()?;
    println!("PASS: {}", report.display());
    Ok(())
}

fn large(scratch: &Path, report: &Path) -> Result<()> {
    let mut results = Vec::new();
    for (label, directory) in [("file-4GiB", false), ("folder-10GiB", true)] {
        let temporary = tempfile::Builder::new()
            .prefix("foldsentinel-large-validation-")
            .tempdir_in(scratch)?;
        println!("{label}: {}", temporary.path().display());
        let input = temporary.path().join("source");
        let mut hashes = Vec::new();
        let size = if directory {
            fs::create_dir_all(input.join("中文 空目录"))?;
            for i in 0..10 {
                let name = format!("文件-{i:02}.bin");
                hashes.push((name.clone(), generate(&input.join(name), GIB)?));
            }
            GIB * 10
        } else {
            hashes.push((String::new(), generate(&input, GIB * 4)?));
            GIB * 4
        };
        let registry = temporary.path().join("jobs");
        let sealed = temporary.path().join("sealed.fse");
        let restored = temporary.path().join("restored");
        let start = Instant::now();
        let encrypted =
            vault::encrypt_with_control(&input, &sealed, PASSWORD, &control(&registry))?;
        let encrypt_seconds = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let decrypted =
            vault::decrypt_with_control(&sealed, &restored, PASSWORD, &control(&registry))?;
        let decrypt_seconds = start.elapsed().as_secs_f64();
        ensure!(
            encrypted.input_bytes == size && decrypted.output_bytes == size,
            "字节计数不符"
        );
        for (name, expected) in &hashes {
            let source = if directory {
                input.join(name)
            } else {
                input.clone()
            };
            let output = if directory {
                restored.join(name)
            } else {
                restored.clone()
            };
            ensure!(&hash(&source)? == expected, "源文件 SHA256 发生变化");
            ensure!(&hash(&output)? == expected, "恢复内容 SHA256 不匹配");
        }
        if directory {
            ensure!(restored.join("中文 空目录").is_dir(), "空目录丢失");
        }
        ensure!(
            vault::recovery::interrupted_jobs(&registry)?.is_empty(),
            "存在残留任务"
        );
        let peak = peak_memory()?;
        ensure!(peak <= 256 * MIB, "核心进程峰值内存超过 256 MiB: {peak}");
        results.push(json!({"case":label,"content_bytes":size,"encrypted_bytes":encrypted.output_bytes,"encrypt_seconds":encrypt_seconds,"decrypt_seconds":decrypt_seconds,"peak_working_set_bytes":peak,"sha256_verified":true,"hashes":hashes}));
        println!("{label}: PASS, peak {} MiB", peak as f64 / MIB as f64);
        temporary.close()?;
    }
    save(
        report,
        json!({"version":"0.2.0","mode":"release","measurement":"Windows GetProcessMemoryInfo PeakWorkingSetSize; core validation process, excludes WebView2","fixtures":"Actual bytes written; random 1 MiB block repeated per file; no compression","results":results}),
    )
}

fn kill_at_sync(input: &Path, output: &Path, registry: &Path, ready: &Path) -> Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("crash-child")
        .arg(input)
        .arg(output)
        .arg(registry)
        .arg(ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let start = Instant::now();
    while !ready.exists() && start.elapsed() < Duration::from_secs(30) {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("child exited before readiness: {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let ready_seen = ready.exists();
    child.kill().context("终止验证子进程失败")?;
    child.wait()?;
    ensure!(ready_seen, "子进程写入超时");
    ensure!(!output.exists(), "中断任务不应提交输出");
    Ok(())
}

fn crash(scratch: &Path, report: &Path) -> Result<()> {
    let temporary = tempfile::Builder::new()
        .prefix("foldsentinel-crash-validation-")
        .tempdir_in(scratch)?;
    let input = temporary.path().join("source");
    let expected = generate(&input, 32 * MIB as u64)?;
    let output = temporary.path().join("sealed.fse");
    let registry = temporary.path().join("jobs");
    kill_at_sync(&input, &output, &registry, &temporary.path().join("ready"))?;
    let jobs = vault::recovery::interrupted_jobs(&registry)?;
    ensure!(jobs.len() == 1, "应有一项中断任务");
    ensure!(
        jobs[0].staging.join("payload").metadata()?.len() > MIB as u64,
        "未写入真实 payload"
    );
    vault::recovery::cleanup_interrupted(&registry, &jobs[0].id)?;
    ensure!(!jobs[0].staging.exists(), "残留目录未清理");
    ensure!(hash(&input)? == expected, "源文件被修改");
    vault::encrypt_with_control(&input, &output, PASSWORD, &control(&registry))?;
    let restored = temporary.path().join("restored");
    vault::decrypt_with_control(&output, &restored, PASSWORD, &control(&registry))?;
    ensure!(hash(&restored)? == expected, "重试恢复不匹配");
    ensure!(
        vault::recovery::interrupted_jobs(&registry)?.is_empty(),
        "中断记录未清理"
    );
    temporary.close()?;
    save(
        report,
        json!({"version":"0.2.0","real_process_termination":true,"payload_bytes":32 * MIB,"output_not_committed":true,"interrupted_job_detected":true,"owned_staging_cleaned":true,"source_unchanged":true,"retry_round_trip_sha256":expected}),
    )
}
