use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use foldsentinel::{ProtectionMode, StateStore, default_state_path};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "foldsentinel",
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
    }
    Ok(())
}
