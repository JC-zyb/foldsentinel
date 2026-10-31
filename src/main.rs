use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use foldsentinel::{ProtectionMode, StateStore, default_state_path, service, vault};
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
    /// 应用可恢复的隐藏、锁定或只读保护方式。
    Protect {
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = ModeArg::HiddenLocked)]
        mode: ModeArg,
    },
    /// Restore attributes saved before protection was applied.
    Unprotect { path: PathBuf },
    /// List tracked protection entries.
    Status { path: Option<PathBuf> },
    /// Print the state file location.
    StatePath,
    /// 通过本机SYSTEM服务管理密码访问与持续防护，不会自动安装服务。
    Managed {
        #[command(subcommand)]
        command: ManagedCommand,
    },
    /// Encrypt a file/folder or restore an authenticated FSEAL package.
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ManagedCommand {
    /// 查询服务运行状态。
    Status,
    /// 仅列出当前Windows账户登记的外层名称和状态。
    List,
    /// 登记持续防护；--password 从终端安全读取密码，不接受命令行密码。
    Enroll {
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = ModeArg::Locked)]
        mode: ModeArg,
        #[arg(long, default_value = "")]
        note: String,
        #[arg(long)]
        password: bool,
    },
    /// 验证密码后临时访问，默认300秒。
    Unlock {
        id: String,
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(30..=3600))]
        seconds: u64,
    },
    /// 立即重新锁定。
    Relock { id: String },
    /// 解除并恢复原始状态；有密码的项目须加--password并验证。
    Remove {
        id: String,
        #[arg(long)]
        password: bool,
    },
    /// 设置或更改密码；--clear 验证当前密码后移除密码。
    Password {
        id: String,
        #[arg(long)]
        clear: bool,
    },
    /// 开关持续扫描；关闭监控不会解除已经应用的权限。
    Continuous {
        id: String,
        #[arg(value_enum)]
        state: MonitoringArg,
        #[arg(long)]
        password: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MonitoringArg {
    On,
    Off,
}

#[derive(Debug, Subcommand)]
enum VaultCommand {
    /// 扫描指定旧包目录并登记内容指纹；不解密，扫描不完整时返回错误。
    Discover {
        #[arg(required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
    },
    /// 列出独立加密资产记录，包含待恢复和无法访问的位置。
    Inventory,
    /// Encrypt a file or folder into the FoldSentinel FSEAL format.
    Encrypt { input: PathBuf, output: PathBuf },
    /// Decrypt a FoldSentinel FSEAL file.
    Decrypt { input: PathBuf, output: PathBuf },
    /// Authenticate a package and validate archive paths without restoring files.
    Verify { input: PathBuf },
    /// List inactive interrupted jobs; no password or key is stored.
    Interrupted,
    /// Remove only the owned staging data of an inactive interrupted job.
    Cleanup { id: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ModeArg {
    None,
    Hidden,
    Locked,
    Readonly,
    ReadonlyAcl,
    HiddenLocked,
    #[value(name = "hidden-readonly")]
    HiddenReadonly,
}

impl From<ModeArg> for ProtectionMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::None => Self::None,
            ModeArg::Hidden => Self::Hidden,
            ModeArg::Locked => Self::Locked,
            ModeArg::Readonly => Self::Readonly,
            ModeArg::ReadonlyAcl => Self::ReadonlyAcl,
            ModeArg::HiddenLocked => Self::HiddenLocked,
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
                "已应用保护 {}，修改 {} 项，状态文件: {}",
                path.display(),
                report.changed,
                store.state_path().display()
            );
        }
        Command::Unprotect { path } => {
            let mut store = StateStore::load_default()?;
            let report = store.unprotect(&path)?;
            println!(
                "已恢复原始状态 {}，恢复 {} 项，缺失 {} 项，状态文件: {}",
                path.display(),
                report.changed,
                report.missing,
                store.state_path().display()
            );
        }
        Command::Status { path } => {
            let store = StateStore::load_default()?;
            let entries = store.statuses(path.as_deref(), 0, usize::MAX)?;
            println!("状态文件: {}", store.state_path().display());
            if path.is_none() {
                let roots = store.roots();
                println!("已保存的最外层路径: {}", roots.len());
                for root in roots {
                    println!("  {}（{} 条恢复记录）", root.path, root.entry_count);
                }
            }
            println!("管理记录: {}", entries.len());
            for status in entries {
                println!(
                    "- {} [{:?}] {:?} current={:?} expected=0x{:X} original=0x{:X} managed=0x{:X} os={:?}",
                    status.entry.path,
                    status.entry.mode,
                    status.state,
                    status
                        .current_attributes
                        .map(|value| format!("0x{value:X}")),
                    status.expected_attributes,
                    status.entry.original_attributes,
                    status.entry.managed_attributes,
                    status.os_code
                );
            }
        }
        Command::StatePath => println!("{}", default_state_path()?.display()),
        Command::Managed { command } => run_managed(command)?,
        Command::Vault { command } => match command {
            VaultCommand::Discover { paths } => {
                let data_dir = foldsentinel::lifecycle::default_data_dir()?;
                let _guard = foldsentinel::lifecycle::OperationGuard::acquire(&data_dir)?;
                let paths = paths
                    .into_iter()
                    .map(std::fs::canonicalize)
                    .collect::<std::io::Result<Vec<_>>>()?;
                let report = vault::inventory::discover(&paths, &data_dir)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.complete || !report.issues.is_empty() {
                    bail!("旧包扫描不完整；已发现的包已登记，错误不能视为无加密项目");
                }
            }
            VaultCommand::Inventory => {
                let data_dir = foldsentinel::lifecycle::default_data_dir()?;
                let _guard = foldsentinel::lifecycle::OperationGuard::acquire(&data_dir)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&vault::inventory::snapshot(&data_dir)?)?
                );
            }
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
            VaultCommand::Verify { input } => {
                let password = read_password(false)?;
                let control = cli_control()?;
                let report = vault::verify_with_control(&input, &password, &control)?;
                println!(
                    "验证通过 {}（{} 字节，无恢复文件）",
                    input.display(),
                    report.input_bytes
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

fn run_managed(command: ManagedCommand) -> Result<()> {
    let mut registration_guard = None;
    let optional_password = |required| -> Result<Option<String>> {
        if required {
            Ok(Some(read_password(false)?.to_string()))
        } else {
            Ok(None)
        }
    };
    let request = match command {
        ManagedCommand::Status => service::Command::Status,
        ManagedCommand::List => service::Command::List,
        ManagedCommand::Enroll {
            path,
            mode,
            note,
            password,
        } => {
            let store = StateStore::load_default()?;
            store.ensure_service_registration_available(&path)?;
            registration_guard = Some(store);
            let mode = mode.into();
            if !matches!(
                mode,
                ProtectionMode::Locked | ProtectionMode::HiddenLocked | ProtectionMode::ReadonlyAcl
            ) {
                bail!("服务登记仅支持locked、hidden-locked和readonly-acl");
            }
            service::Command::Enroll {
                path: std::fs::canonicalize(path)?,
                mode,
                note,
                password: if password {
                    Some(read_password(true)?.to_string())
                } else {
                    None
                },
            }
        }
        ManagedCommand::Unlock { id, seconds } => service::Command::Unlock {
            id,
            password: read_password(false)?.to_string(),
            duration_seconds: seconds,
        },
        ManagedCommand::Relock { id } => service::Command::Relock { id },
        ManagedCommand::Remove { id, password } => service::Command::Remove {
            id,
            password: optional_password(password)?,
        },
        ManagedCommand::Password { id, clear } => {
            let password =
                Zeroizing::new(rpassword::prompt_password("当前密码（首次设置留空）：")?);
            let next = if clear {
                None
            } else {
                Some(read_password(true)?)
            };
            service::Command::SetPassword {
                id,
                password: password.to_string(),
                new_password: next.as_ref().map(|value| value.to_string()),
            }
        }
        ManagedCommand::Continuous {
            id,
            state,
            password,
        } => service::Command::SetContinuous {
            id,
            password: optional_password(password)?,
            enabled: matches!(state, MonitoringArg::On),
        },
    };
    // 持有本地状态锁至请求返回，串行化本账户的两种登记入口。
    let _registration_guard = registration_guard;
    let response = service::request(&service::Request::new(request))?;
    if !response.transport.healthy {
        eprintln!("保护服务存在待处理失败，请检查逐项状态");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&response.into_result()?)?
    );
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
