use super::VaultReport;
use serde::Serialize;

pub const MAX_PREVIEW_ENTRIES: usize = 1000;
const MAX_PREVIEW_NAMES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct PreviewEntry {
    /// 单文件包不保存原始文件名，使用 None，不能从包名猜测归档元数据。
    pub path: Option<String>,
    pub directory: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackagePreview {
    pub format_version: u8,
    pub total_entries: usize,
    pub file_count: usize,
    pub directory_count: usize,
    pub content_bytes: u64,
    pub entries: Vec<PreviewEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultInspection {
    #[serde(flatten)]
    pub report: VaultReport,
    pub preview: PackagePreview,
}

#[derive(Default)]
pub(super) struct PreviewBuilder {
    entries: Vec<PreviewEntry>,
    total_entries: usize,
    file_count: usize,
    directory_count: usize,
    content_bytes: u64,
    names: usize,
    truncated: bool,
}

impl PreviewBuilder {
    // 仅由已有有界解析器在条目校验后调用；结果在完整认证后才发布。
    pub(super) fn record(&mut self, path: Option<&str>, directory: bool, size: u64) {
        self.total_entries += 1;
        if directory {
            self.directory_count += 1;
        } else {
            self.file_count += 1;
            self.content_bytes += size;
        }
        let bytes = path.map_or(0, str::len);
        if self.truncated
            || self.entries.len() >= MAX_PREVIEW_ENTRIES
            || self.names + bytes > MAX_PREVIEW_NAMES
        {
            self.truncated = true;
            return;
        }
        self.names += bytes;
        self.entries.push(PreviewEntry {
            path: path.map(|name| name.replace('\\', "/")),
            directory,
            size,
        });
    }

    pub(super) fn finish(self, format_version: u8, report: VaultReport) -> VaultInspection {
        VaultInspection {
            report,
            preview: PackagePreview {
                format_version,
                total_entries: self.total_entries,
                file_count: self.file_count,
                directory_count: self.directory_count,
                content_bytes: self.content_bytes,
                entries: self.entries,
                truncated: self.truncated,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_limits_display_but_counts_all_entries() {
        let mut builder = PreviewBuilder::default();
        for _ in 0..MAX_PREVIEW_ENTRIES + 3 {
            builder.record(Some("资料\\内容.txt"), false, 7);
        }
        assert_eq!(builder.entries.len(), MAX_PREVIEW_ENTRIES);
        assert_eq!(builder.entries[0].path.as_deref(), Some("资料/内容.txt"));
        assert_eq!(builder.file_count, MAX_PREVIEW_ENTRIES + 3);
        assert_eq!(builder.content_bytes, (MAX_PREVIEW_ENTRIES as u64 + 3) * 7);
        assert!(builder.truncated);
    }

    #[test]
    fn preview_name_budget_keeps_a_prefix_and_does_not_invent_file_names() {
        let mut builder = PreviewBuilder::default();
        let name = "x".repeat(4096);
        for _ in 0..65 {
            builder.record(Some(&name), true, 0);
        }
        builder.record(None, false, 9);
        assert_eq!(builder.entries.len(), 64);
        assert_eq!(builder.directory_count, 65);
        assert_eq!(builder.file_count, 1);
        assert_eq!(builder.content_bytes, 9);
        assert!(builder.truncated);
        let mut file = PreviewBuilder::default();
        file.record(None, false, 0);
        assert!(file.entries[0].path.is_none());
        assert!(!file.truncated);
    }
}
