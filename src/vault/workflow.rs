use super::{ErrorCode, legacy};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Encrypt,
    Decrypt,
    Verify,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedTask {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub operation: Operation,
}

/// 只生成明确可见的输出建议；执行时仍独占创建并禁止覆盖。
pub fn prepare(
    inputs: &[PathBuf],
    operation: Operation,
    directory: Option<&Path>,
    reserved: &[PathBuf],
) -> Result<Vec<PreparedTask>> {
    ensure!(
        !inputs.is_empty() && inputs.len() <= 200 && reserved.len() <= 500,
        ErrorCode::InvalidInput
    );
    let directory = directory.map(fs::canonicalize).transpose()?;
    ensure!(
        directory.as_ref().is_none_or(|d| d.is_dir()),
        ErrorCode::InvalidInput
    );
    let mut used: HashSet<String> = reserved.iter().map(|p| key(p)).collect();
    let mut tasks = Vec::new();
    for input in inputs {
        legacy::reject_link(input)?;
        let input = fs::canonicalize(input)?;
        ensure!(
            input.is_file() || (operation == Operation::Encrypt && input.is_dir()),
            ErrorCode::InvalidInput
        );
        let output = if operation == Operation::Verify {
            None
        } else {
            let parent = directory
                .as_deref()
                .unwrap_or(input.parent().context(ErrorCode::InvalidInput)?);
            let name = input
                .file_name()
                .and_then(|n| n.to_str())
                .context(ErrorCode::InvalidInput)?;
            let name =
                if operation == Operation::Decrypt && name.to_ascii_lowercase().ends_with(".fse") {
                    &name[..name.len() - 4]
                } else {
                    name
                };
            let mut units = 0;
            let base: String = name
                .chars()
                .take_while(|c| {
                    units += c.len_utf16();
                    units <= 180
                })
                .collect();
            let suffix = if operation == Operation::Encrypt {
                ".fse"
            } else {
                ".restored"
            };
            let mut chosen = None;
            for number in 1..=10_000 {
                let path = parent.join(if number == 1 {
                    format!("{base}{suffix}")
                } else {
                    format!("{base}-{number}{suffix}")
                });
                if used.contains(&key(&path)) {
                    continue;
                }
                match fs::symlink_metadata(&path) {
                    Ok(_) => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                let path = legacy::output_path(&path)?;
                if operation == Operation::Encrypt {
                    ensure!(
                        !PathBuf::from(key(&path)).starts_with(PathBuf::from(key(&input))),
                        ErrorCode::InvalidInput
                    );
                }
                used.insert(key(&path));
                chosen = Some(path);
                break;
            }
            Some(chosen.context(ErrorCode::OutputExists)?)
        };
        tasks.push(PreparedTask {
            input,
            output,
            operation,
        });
    }
    Ok(tasks)
}

fn key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .replace('/', "\\")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_avoid_existing_and_reserved_outputs_and_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("资料.txt");
        fs::write(&input, b"source").unwrap();
        fs::write(dir.path().join("资料.txt.fse"), b"keep").unwrap();
        let tasks = prepare(
            &[input.clone(), input.clone()],
            Operation::Encrypt,
            None,
            &[],
        )
        .unwrap();
        assert!(
            tasks[0]
                .output
                .as_ref()
                .unwrap()
                .ends_with("资料.txt-2.fse")
        );
        assert!(
            tasks[1]
                .output
                .as_ref()
                .unwrap()
                .ends_with("资料.txt-3.fse")
        );
        let more = prepare(
            std::slice::from_ref(&input),
            Operation::Encrypt,
            None,
            &[tasks[0].output.clone().unwrap()],
        )
        .unwrap();
        assert_eq!(more[0].output, tasks[1].output);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        assert!(prepare(&[dir.path().to_owned()], Operation::Verify, None, &[]).is_err());
        assert!(
            prepare(
                &[dir.path().to_owned()],
                Operation::Encrypt,
                Some(dir.path()),
                &[]
            )
            .is_err()
        );
        assert!(
            prepare(&[input], Operation::Verify, None, &[]).unwrap()[0]
                .output
                .is_none()
        );
    }
}
