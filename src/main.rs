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
                let report = vault::encrypt_file(&input, &output, &password)?;
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
                let report = vault::decrypt_file(&input, &output, &password)?;
                println!(
                    "已解密 {} -> {}（{} 字节 -> {} 字节）",
                    input.display(),
                    output.display(),
                    report.input_bytes,
                    report.output_bytes
                );
            }
        },
    }
    Ok(())
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
