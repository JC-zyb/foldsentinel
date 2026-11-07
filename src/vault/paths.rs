use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) fn output_path(path: &Path) -> Result<PathBuf> {
    ensure!(path.file_name().is_some(), "请指定完整输出名称");
    match fs::symlink_metadata(path) {
        Ok(_) => bail!(super::ErrorCode::OutputExists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("检查输出位置失败"),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).context("输出父目录不存在或无法访问")?;
    ensure!(parent.is_dir(), "输出父路径不是目录");
    Ok(parent.join(path.file_name().context("缺少输出名称")?))
}

pub(crate) fn reject_link(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("读取输入元数据失败")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "首版不处理符号链接、目录联接或其他重解析点: {}",
            path.display()
        );
    }
    ensure!(!metadata.file_type().is_symlink(), "首版不处理符号链接");
    Ok(())
}

pub(super) fn validate_archive_path(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "归档路径为空");
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("加密包包含不安全的路径");
        };
        let name = name.to_str().context("首版不支持非 Unicode 文件名")?;
        ensure!(
            !name.ends_with(['.', ' '])
                && !name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
            "不支持的 Windows 文件名: {name}"
        );
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        ensure!(
            !reserved.contains(&stem.as_str()),
            "Windows 保留文件名: {name}"
        );
    }
    Ok(())
}
