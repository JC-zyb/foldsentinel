use serde::{Deserialize, Serialize};
use std::{fmt, io};

/// IPC 使用稳定类别，不依赖底层中文或系统本地化错误文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Cancelled,
    Authentication,
    UnsupportedFormat,
    InvalidPackage,
    ReadFailed,
    WriteFailed,
    OutputExists,
    InsufficientSpace,
    PermissionDenied,
    InvalidInput,
    SourceChanged,
    Busy,
    Recovery,
    ProtectionConflict,
    ProtectionLocation,
    ProtectionUnsupported,
    ProtectionInput,
    ProtectionRecordChanged,
    ProtectionBackupInvalid,
    ProtectionBackupConflict,
    ServiceUnavailable,
    ProtectionLocalOverlap,
    ProtectionServiceOverlap,
    Internal,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "操作已取消，未提交输出",
            Self::Authentication => "密码错误或加密包认证失败",
            Self::UnsupportedFormat => "不支持的加密包格式或版本",
            Self::InvalidPackage => "加密包结构不合法或不完整",
            Self::ReadFailed => "读取数据失败",
            Self::WriteFailed => "写入或提交数据失败",
            Self::OutputExists => "输出位置已存在，请选择新名称",
            Self::InsufficientSpace => "目标磁盘空间不足",
            Self::PermissionDenied => "无权访问指定位置",
            Self::InvalidInput => "输入路径、密码或操作参数不合法",
            Self::SourceChanged => "源文件在操作期间发生变化",
            Self::Busy => "已有任务正在运行",
            Self::Recovery => "中断记录或临时数据处理失败",
            Self::ProtectionConflict => {
                "文件身份、权限或父目录继承已变化；请保留恢复记录，并从最外层项目恢复"
            }
            Self::ProtectionLocation => "此位置不能保护，请选择普通文件或子文件夹",
            Self::ProtectionUnsupported => "此项目不支持当前保护方式，请检查文件系统或链接类型",
            Self::ProtectionInput => "保护项目或备注不合法，请检查输入后重试",
            Self::ProtectionRecordChanged => "保护记录已变化，请刷新列表后重试",
            Self::ProtectionBackupInvalid => "备份无效、已被修改，或不属于本机当前 Windows 账户",
            Self::ProtectionBackupConflict => "当前已有保护记录，不能通过导入备份覆盖",
            Self::ServiceUnavailable => "本机保护服务未安装、未运行或无法验证身份，请检查服务状态",
            Self::ProtectionLocalOverlap => {
                "此范围已有本地保护记录，请先从本地恢复入口解除，再登记服务保护"
            }
            Self::ProtectionServiceOverlap => {
                "此范围已有服务保护，请先验证密码并解除服务保护，再使用本地入口"
            }
            Self::Internal => "后台任务或本地状态处理失败",
        })
    }
}
impl std::error::Error for ErrorCode {}

pub fn error_code(error: &anyhow::Error) -> ErrorCode {
    // anyhow 的类型化 context 不一定作为独立的 std::error::Error 链节点出现。
    let contextual = error.downcast_ref::<ErrorCode>().copied();
    if contextual == Some(ErrorCode::Recovery)
        || error
            .chain()
            .any(|cause| cause.downcast_ref::<ErrorCode>() == Some(&ErrorCode::Recovery))
    {
        return ErrorCode::Recovery;
    }
    if contextual == Some(ErrorCode::ServiceUnavailable)
        || error
            .chain()
            .any(|cause| cause.downcast_ref::<ErrorCode>() == Some(&ErrorCode::ServiceUnavailable))
    {
        return ErrorCode::ServiceUnavailable;
    }
    let mut code = contextual;
    let mut io_failure = false;
    for cause in error.chain() {
        if let Some(value) = cause.downcast_ref::<ErrorCode>() {
            code = Some(*value);
        }
        if let Some(value) = cause.downcast_ref::<io::Error>() {
            if let Some(inner) = value.get_ref().and_then(|e| e.downcast_ref::<ErrorCode>()) {
                return *inner;
            }
            match value.raw_os_error() {
                Some(39 | 112) => return ErrorCode::InsufficientSpace,
                Some(80 | 183) => return ErrorCode::OutputExists,
                _ => {}
            }
            match value.kind() {
                io::ErrorKind::PermissionDenied => return ErrorCode::PermissionDenied,
                io::ErrorKind::AlreadyExists => return ErrorCode::OutputExists,
                io::ErrorKind::UnexpectedEof => {
                    code.get_or_insert(ErrorCode::InvalidPackage);
                }
                _ => {
                    io_failure = true;
                }
            }
        }
    }
    code.unwrap_or(if io_failure {
        ErrorCode::ReadFailed
    } else {
        ErrorCode::InvalidInput
    })
}
