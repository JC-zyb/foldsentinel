use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use foldsentinel::{ProtectionMode, StateStore, default_state_path, vault};
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(
    name = "foldsentinel-cli",
    version,
    about = "Windows folder protection CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Apply a reversible file-attribute policy to a file or directory tree.
    Protect {
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = ModeArg::HiddenReadonly)]
        mode: ModeArg,
    },
    /// Restore attributes saved before protection was applied.
    Unprotect { path: PathBuf },
    /// List tracked protection entries.
    Status { path: Option<PathBuf> },
    /// Print the state file location.
    StatePath,
    /// Encrypt a file/folder or restore an authenticated FSEAL package.
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
}

#[derive(Debug, Subcommand)]
enum VaultCommand {
    /// Encrypt a file or folder into the FoldSentinel FSEAL format.
    Encrypt { input: PathBuf, output: PathBuf },
    /// Decrypt a FoldSentinel FSEAL file.
    Decrypt { input: PathBuf, output: PathBuf },
    /// List inactive interrupted jobs; no password or key is stored.
    Interrupted,
    /// Remove only the owned staging data of an inactive interrupted job.
    Cleanup { id: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ModeArg {
    Hidden,
    Readonly,
    #[value(name = "hidden-readonly")]
    HiddenReadonly,
}

impl From<ModeArg> for ProtectionMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Hidden => Self::Hidden,
            ModeArg::Readonly => Self::Readonly,
            ModeArg::HiddenReadonly => Self::HiddenReadonly,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Protect { path, mode } => {
            let mut store = StateStore::load_default()?;
            let report = store.protect(&path, mode.into())?;
            println!(
                "已保护 {}，修改 {} 项，状态文件: {}",
                path.display(),
                report.changed,
                store.state_path().display()
            );
        }
        Command::Unprotect { path } => {
            let mut store = StateStore::load_default()?;
            let report = store.unprotect(&path)?;
            println!(
                "已解除保护 {}，恢复 {} 项，缺失 {} 项，状态文件: {}",
                path.display(),
                report.changed,
                report.missing,
                store.state_path().display()
            );
        }
        Command::Status { path } => {
            let store = StateStore::load_default()?;
            let entries = store.matching_entries(path.as_deref())?;
            println!("状态文件: {}", store.state_path().display());
            println!("受保护项目: {}", entries.len());
            for entry in entries {
                println!("- {} [{:?}]", entry.path, entry.mode);
            }
        }
        Command::StatePath => println!("{}", default_state_path()?.display()),
        Command::Vault { command } => match command {
            VaultCommand::Encrypt { input, output } => {
                let password = read_password(true)?;
                let control = cli_control()?;
                let report = vault::encrypt_with_control(&input, &output, &password, &control)?;
                println!(
                    "已加密 {} -> {}（{} 字节 -> {} 字节）",
                    input.display(),
                    output.display(),
                    report.input_bytes,
                    report.output_bytes
                );
            }
            VaultCommand::Decrypt { input, output } => {
                let password = read_password(false)?;
                let control = cli_control()?;
                let report = vault::decrypt_with_control(&input, &output, &password, &control)?;
                println!(
                    "已解密 {} -> {}（{} 字节 -> {} 字节）",
                    input.display(),
                    output.display(),
                    report.input_bytes,
                    report.output_bytes
                );
            }
            VaultCommand::Interrupted => {
                let registry = vault::recovery::default_recovery_dir()?;
                for job in vault::recovery::interrupted_jobs(&registry)? {
                    println!(
                        "{}\n  输入: {}\n  输出: {}\n  临时数据: {}",
                        job.id,
                        job.input.display(),
                        job.output.display(),
                        job.staging.display()
                    );
                }
            }
            VaultCommand::Cleanup { id } => {
                let registry = vault::recovery::default_recovery_dir()?;
                vault::recovery::cleanup_interrupted(&registry, &id)?;
                println!("已清理 {id} 的临时数据；源文件及最终输出保留。可重新输入密码重试。");
            }
        },
    }
    Ok(())
}

fn cli_control() -> Result<vault::Control> {
    let control = vault::Control::new(|p| {
        eprintln!(
            "{}: {} / {} 字节 {}",
            p.phase, p.completed, p.total, p.current
        );
    })?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
        };
        static CONTROL: std::sync::OnceLock<vault::Control> = std::sync::OnceLock::new();
        unsafe extern "system" fn handler(event: u32) -> i32 {
            if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
                if let Some(control) = CONTROL.get() {
                    control.cancel();
                }
                return 1;
            }
            0
        }
        CONTROL
            .set(control.clone())
            .map_err(|_| anyhow::anyhow!("取消处理器已初始化"))?;
        // 回调只访问静态原子取消标记；进程内仅运行一次 vault 命令。
        if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(control)
}

fn read_password(confirm: bool) -> Result<Zeroizing<String>> {
    let password = Zeroizing::new(rpassword::prompt_password("密码: ")?);
    if password.is_empty() {
        bail!("密码不能为空");
    }
    if confirm {
        let repeated = Zeroizing::new(rpassword::prompt_password("确认密码: ")?);
        if password != repeated {
            bail!("两次输入的密码不一致");
        }
    }
    Ok(password)
}
