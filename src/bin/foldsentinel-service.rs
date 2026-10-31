use anyhow::{Result, bail};
use clap::Parser;
use foldsentinel::service;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Parser)]
#[command(
    name = "foldsentinel-service",
    about = "FoldSentinel 本机持续防护服务；默认由 Windows 服务管理器启动"
)]
struct Arguments {
    /// 以前台宿主调试，不安装服务；禁止SYSTEM使用
    #[arg(long)]
    console: bool,
    #[arg(long, requires = "console")]
    state_dir: Option<PathBuf>,
    #[arg(long, requires = "console")]
    pipe_name: Option<String>,
    /// 卸载前核对所有账户均无受管项目；不会解除保护
    #[arg(long, conflicts_with = "console")]
    audit_empty: bool,
    /// 管理员执行完整正常卸载；仅删除固定服务程序文件，保留ProgramData恢复资料
    #[arg(long, conflicts_with_all = ["console", "audit_empty"])]
    uninstall_empty: bool,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    if arguments.audit_empty {
        return service::audit_empty();
    }
    if arguments.uninstall_empty {
        return service::uninstall_empty();
    }
    if arguments.console {
        let Some(path) = arguments.state_dir else {
            bail!("调试模式必须指定隔离 --state-dir");
        };
        let Some(pipe) = arguments.pipe_name else {
            bail!("调试模式必须指定隔离 --pipe-name");
        };
        return service::run_console(path, &pipe, Arc::new(AtomicBool::new(false)));
    }
    service::run_service()
}
