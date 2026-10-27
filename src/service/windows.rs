use super::*;
use crate::managed::{AuthorizedPath, ManagedStore};
use std::collections::BTreeMap;
use std::ffi::{OsStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::os::windows::{
    ffi::OsStrExt,
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::ptr::{null, null_mut};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{Com::CoTaskMemFree, IO::*, Pipes::*, Services::*, Threading::*},
    UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath},
};

const SYSTEM_SID: &str = "S-1-5-18";
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_AUTHORIZED_ENTRIES: usize = 100_000;
const MAX_ACCOUNTS: usize = 256;
static SERVICE_STOP: AtomicBool = AtomicBool::new(false);
static SERVICE_STATUS_HANDLE: AtomicUsize = AtomicUsize::new(0);

struct Handle(OwnedHandle);
impl Handle {
    unsafe fn new(value: HANDLE) -> Result<Self> {
        if value.is_null() || value == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(value) }))
    }
    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}
fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn checked(value: BOOL) -> Result<()> {
    if value == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
fn code_checked(value: u32) -> Result<()> {
    if value == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(value as i32).into())
    }
}
fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn token_sid(token: HANDLE) -> Result<String> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
    }
    if size == 0 || size > 64 * 1024 {
        bail!(ErrorCode::PermissionDenied);
    }
    let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
    unsafe {
        checked(GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        ))?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = null_mut();
        checked(ConvertSidToStringSidW(user.User.Sid, &mut text))?;
        let text = Local(text.cast());
        let mut length = 0;
        while *text.0.cast::<u16>().add(length) != 0 {
            length += 1;
        }
        Ok(String::from_utf16(std::slice::from_raw_parts(
            text.0.cast(),
            length,
        ))?)
    }
}
pub(super) fn process_sid() -> Result<String> {
    unsafe {
        let mut token = null_mut();
        checked(OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        ))?;
        token_sid(Handle::new(token)?.raw())
    }
}

pub(super) fn service_is_installed() -> Result<bool> {
    struct ServiceHandle(SC_HANDLE);
    impl Drop for ServiceHandle {
        fn drop(&mut self) {
            unsafe {
                CloseServiceHandle(self.0);
            }
        }
    }
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(std::io::Error::last_os_error()).context(ErrorCode::ServiceUnavailable);
    }
    let manager = ServiceHandle(manager);
    let service =
        unsafe { OpenServiceW(manager.0, wide(SERVICE_NAME).as_ptr(), SERVICE_QUERY_STATUS) };
    if service.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
            return Ok(false);
        }
        return Err(error).context(ErrorCode::ServiceUnavailable);
    }
    let _service = ServiceHandle(service);
    Ok(true)
}
fn valid_sid(sid: &str) -> bool {
    if !sid.starts_with("S-1-")
        || sid.len() > 184
        || !sid
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
    {
        return false;
    }
    let mut value = null_mut();
    let ok = unsafe { ConvertStringSidToSidW(wide(sid).as_ptr(), &mut value) };
    let _value = Local(value);
    ok != 0
}
struct Impersonating;
impl Drop for Impersonating {
    fn drop(&mut self) {
        if unsafe { RevertToSelf() } == 0 {
            // 继续运行会混淆服务与客户端身份；Microsoft要求此时终止进程。
            std::process::abort();
        }
    }
}
fn as_client<T>(pipe: HANDLE, action: impl FnOnce(&str) -> Result<T>) -> Result<T> {
    unsafe {
        checked(ImpersonateNamedPipeClient(pipe)).context(ErrorCode::PermissionDenied)?;
    }
    let _impersonating = Impersonating;
    let mut token = null_mut();
    unsafe {
        checked(OpenThreadToken(
            GetCurrentThread(),
            TOKEN_QUERY,
            1,
            &mut token,
        ))?;
    }
    let token = unsafe { Handle::new(token)? };
    let mut level = 0i32;
    let mut returned = 0;
    unsafe {
        checked(GetTokenInformation(
            token.raw(),
            TokenImpersonationLevel,
            (&mut level as *mut i32).cast(),
            4,
            &mut returned,
        ))?;
    }
    if level < SecurityImpersonation {
        bail!(ErrorCode::PermissionDenied);
    }
    let sid = token_sid(token.raw())?;
    if !valid_sid(&sid) || sid == SYSTEM_SID {
        bail!(ErrorCode::PermissionDenied);
    }
    action(&sid)
}

fn sd(text: &str) -> Result<Local> {
    let mut descriptor = null_mut();
    unsafe {
        checked(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(text).as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ))?;
    }
    Ok(Local(descriptor))
}
fn sddl(descriptor: *const c_void, information: u32) -> Result<String> {
    let mut text = null_mut();
    let mut size = 0;
    unsafe {
        checked(ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor as _,
            1,
            information,
            &mut text,
            &mut size,
        ))?;
        let text = Local(text.cast());
        Ok(
            String::from_utf16(std::slice::from_raw_parts(text.0.cast(), size as usize))?
                .trim_end_matches('\0')
                .to_owned(),
        )
    }
}
fn private_sddl(sid: &str) -> String {
    if sid == SYSTEM_SID {
        "O:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)".to_owned()
    } else {
        format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
    }
}
fn private_directory(path: &Path, owner: &str) -> Result<File> {
    let descriptor = sd(&private_sddl(owner))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) };
    if created == 0 && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return Err(std::io::Error::last_os_error().into());
    }
    let directory = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let info = file_info(&directory)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        bail!(ErrorCode::ProtectionConflict);
    }
    let mut actual = null_mut();
    let mask = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    unsafe {
        code_checked(GetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            mask,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut actual,
        ))?;
    }
    let actual = Local(actual);
    if sddl(actual.0, mask)? != sddl(descriptor.0, mask)? {
        // 不接管可被普通用户写过的旧目录，也不信任其中既有恢复数据。
        bail!(ErrorCode::ProtectionConflict);
    }
    Ok(directory)
}
fn default_directory() -> Result<PathBuf> {
    let mut value = null_mut();
    let result = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, null_mut(), &mut value) };
    if result < 0 {
        bail!("读取系统服务数据目录失败: {result}");
    }
    let mut length = 0;
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
    }
    let value_string = unsafe { String::from_utf16(std::slice::from_raw_parts(value, length)) };
    unsafe {
        CoTaskMemFree(value.cast());
    }
    Ok(PathBuf::from(value_string?).join(SERVICE_NAME))
}
fn file_info(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = unsafe { std::mem::zeroed() };
    unsafe {
        checked(GetFileInformationByHandle(file.as_raw_handle(), &mut info))?;
    }
    Ok(info)
}
fn identity(info: &BY_HANDLE_FILE_INFORMATION) -> [u64; 3] {
    [
        info.dwVolumeSerialNumber as u64,
        (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
        (info.ftCreationTime.dwHighDateTime as u64) << 32
            | info.ftCreationTime.dwLowDateTime as u64,
    ]
}
fn authorization_file(path: &Path, modifying: bool) -> Result<(File, BY_HANDLE_FILE_INFORMATION)> {
    let access = READ_CONTROL
        | FILE_READ_ATTRIBUTES
        | if modifying {
            WRITE_DAC | FILE_WRITE_ATTRIBUTES
        } else {
            0
        };
    let file = OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let info = file_info(&file)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.nNumberOfLinks > 1 {
        bail!(ErrorCode::ProtectionUnsupported);
    }
    Ok((file, info))
}
fn authorize_tree(path: &Path) -> Result<AuthorizedTree> {
    use std::path::{Component, Prefix};
    match path.components().next() {
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
        _ => bail!(ErrorCode::ProtectionLocation),
    }
    let mut held = Vec::new();
    // 在解析前拒绝输入中任何重解析祖先；持有句柄避免后续路径替换。
    for ancestor in path.ancestors().filter(|p| p.parent().is_some()) {
        held.push(authorization_file(ancestor, false)?.0);
    }
    let canonical = fs::canonicalize(path)?;
    for ancestor in canonical.ancestors() {
        held.push(authorization_file(ancestor, false)?.0);
    }
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(&canonical).follow_links(false) {
        let entry = entry?;
        if paths.len() >= MAX_AUTHORIZED_ENTRIES {
            bail!(ErrorCode::InvalidInput);
        }
        let (file, info) = authorization_file(entry.path(), true)?;
        paths.push(AuthorizedPath {
            path: entry.path().to_owned(),
            identity: identity(&info),
        });
        held.push(file);
    }
    Ok(AuthorizedTree { paths, _held: held })
}

fn validate_pipe_name(name: &str, console: bool) -> Result<()> {
    let suffix = name
        .strip_prefix(r"\\.\pipe\")
        .context(ErrorCode::InvalidInput)?;
    if suffix.is_empty()
        || suffix.len() > 180
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        || (console && name.eq_ignore_ascii_case(PIPE_NAME))
    {
        bail!(ErrorCode::InvalidInput);
    }
    Ok(())
}
fn create_pipe(name: &str, owner: &str, console: bool) -> Result<Handle> {
    validate_pipe_name(name, console)?;
    // 不给AU FILE_CREATE_PIPE_INSTANCE（与FILE_APPEND_DATA同位），防止第二个服务端实例。
    let acl = if console {
        format!("D:P(A;;FA;;;{owner})(A;;FA;;;SY)")
    } else {
        "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x00120183;;;AU)".to_owned()
    };
    let descriptor = sd(&acl)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    unsafe {
        Handle::new(CreateNamedPipeW(
            wide(name).as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            1000,
            &attributes,
        ))
    }
}
fn interrupted() -> anyhow::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "服务停止").into()
}
fn io_overlapped(
    handle: HANDLE,
    timeout: Option<Duration>,
    stop: &AtomicBool,
    begin: impl FnOnce(*mut OVERLAPPED, *mut u32) -> BOOL,
) -> Result<u32> {
    let event = unsafe { Handle::new(CreateEventW(null(), 1, 0, null()))? };
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.raw();
    let mut transferred = 0;
    if begin(&mut overlapped, &mut transferred) != 0 {
        return Ok(transferred);
    }
    if unsafe { GetLastError() } != ERROR_IO_PENDING {
        return Err(std::io::Error::last_os_error().into());
    }
    let deadline = timeout.map(|duration| Instant::now() + duration);
    loop {
        let waiting = unsafe { WaitForSingleObject(event.raw(), 25) };
        if waiting == WAIT_OBJECT_0 {
            unsafe {
                checked(GetOverlappedResult(
                    handle,
                    &overlapped,
                    &mut transferred,
                    0,
                ))?;
            }
            return Ok(transferred);
        }
        let expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if waiting == WAIT_FAILED || stop.load(Ordering::Acquire) || expired {
            // 必须等待取消完成，才能释放OVERLAPPED、事件和读写缓冲区。
            unsafe {
                CancelIoEx(handle, &overlapped);
                GetOverlappedResult(handle, &overlapped, &mut transferred, 1);
            }
            if stop.load(Ordering::Acquire) {
                return Err(interrupted());
            }
            if expired {
                return Err(
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "本机服务通信超时").into(),
                );
            }
            return Err(std::io::Error::last_os_error().into());
        }
    }
}
fn connect(pipe: HANDLE, stop: &AtomicBool) -> Result<()> {
    io_overlapped(pipe, None, stop, |overlapped, _| unsafe {
        let result = ConnectNamedPipe(pipe, overlapped);
        if result == 0 && GetLastError() == ERROR_PIPE_CONNECTED {
            1
        } else {
            result
        }
    })?;
    Ok(())
}
fn read_exact(
    handle: HANDLE,
    buffer: &mut [u8],
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut offset = 0;
    while offset < buffer.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("本机服务通信超时")?;
        let size = io_overlapped(handle, Some(remaining), stop, |overlapped, count| unsafe {
            ReadFile(
                handle,
                buffer[offset..].as_mut_ptr(),
                (buffer.len() - offset) as u32,
                count,
                overlapped,
            )
        })? as usize;
        if size == 0 {
            return Err(
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "服务通信中断").into(),
            );
        }
        offset += size;
    }
    Ok(())
}
fn read_frame(handle: HANDLE, timeout: Duration, stop: &AtomicBool) -> Result<Zeroizing<Vec<u8>>> {
    let mut header = [0; 4];
    read_exact(handle, &mut header, timeout, stop)?;
    let size = u32::from_le_bytes(header) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        bail!(ErrorCode::InvalidInput);
    }
    let mut frame = Zeroizing::new(vec![0; size]);
    read_exact(handle, &mut frame, timeout, stop)?;
    Ok(frame)
}
fn write_frame(handle: HANDLE, bytes: &[u8], timeout: Duration, stop: &AtomicBool) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        bail!(ErrorCode::InvalidInput);
    }
    let mut frame = Zeroizing::new(Vec::with_capacity(bytes.len() + 4));
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(bytes);
    let deadline = Instant::now() + timeout;
    let mut offset = 0;
    while offset < frame.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("本机服务通信超时")?;
        let size = io_overlapped(handle, Some(remaining), stop, |overlapped, count| unsafe {
            WriteFile(
                handle,
                frame[offset..].as_ptr(),
                (frame.len() - offset) as u32,
                count,
                overlapped,
            )
        })? as usize;
        if size == 0 {
            bail!(ErrorCode::WriteFailed);
        }
        offset += size;
    }
    Ok(())
}
fn verify_server(pipe: HANDLE, expected: &str) -> Result<()> {
    let mut pid = 0;
    unsafe {
        checked(GetNamedPipeServerProcessId(pipe, &mut pid))?;
    }
    if expected == SYSTEM_SID {
        return verify_service_process(pid);
    }
    let process = unsafe { Handle::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid))? };
    let mut token = null_mut();
    unsafe {
        checked(OpenProcessToken(process.raw(), TOKEN_QUERY, &mut token))?;
    }
    let token = unsafe { Handle::new(token)? };
    if token_sid(token.raw())? != expected {
        bail!(ErrorCode::PermissionDenied);
    }
    Ok(())
}

struct IdentityServiceHandle(SC_HANDLE);
impl Drop for IdentityServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

fn verify_service_process(pid: u32) -> Result<()> {
    // 普通账户无权查询SYSTEM进程令牌；由SCM核对固定服务的配置和当前PID。
    // 服务配置默认仅管理员可改，管道PID来自内核，不接受服务器自报身份。
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let manager = IdentityServiceHandle(manager);
    let service = unsafe {
        OpenServiceW(
            manager.0,
            wide(SERVICE_NAME).as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
        )
    };
    if service.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let service = IdentityServiceHandle(service);
    let mut needed = 0;
    unsafe {
        QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
    }
    if !(size_of::<QUERY_SERVICE_CONFIGW>() as u32..=8192).contains(&needed) {
        bail!(ErrorCode::PermissionDenied);
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    unsafe {
        checked(QueryServiceConfigW(service.0, config, needed, &mut needed))?;
        if (*config).dwServiceType != SERVICE_WIN32_OWN_PROCESS
            || (*config).lpServiceStartName.is_null()
        {
            bail!(ErrorCode::PermissionDenied);
        }
        let account = (*config).lpServiceStartName;
        let mut length = 0;
        while length < 128 && *account.add(length) != 0 {
            length += 1;
        }
        if length == 128
            || !String::from_utf16(std::slice::from_raw_parts(account, length))?
                .eq_ignore_ascii_case("LocalSystem")
        {
            bail!(ErrorCode::PermissionDenied);
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        checked(QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        ))?;
        if pid == 0
            || status.dwCurrentState != SERVICE_RUNNING
            || status.dwProcessId != pid
            || status.dwServiceType != SERVICE_WIN32_OWN_PROCESS
        {
            bail!(ErrorCode::PermissionDenied);
        }
    }
    Ok(())
}
pub(super) fn request_to(
    request: &Request,
    name: &str,
    console_sid: Option<&str>,
) -> Result<Response> {
    request.command.validate()?;
    validate_pipe_name(name, console_sid.is_some())?;
    if request.version != PROTOCOL_VERSION {
        bail!(ErrorCode::UnsupportedFormat);
    }
    let bytes = Zeroizing::new(serde_json::to_vec(request)?);
    if bytes.len() > MAX_FRAME_BYTES {
        bail!(ErrorCode::InvalidInput);
    }
    let deadline = Instant::now() + IO_TIMEOUT;
    let pipe = loop {
        let handle = unsafe {
            CreateFileW(
                wide(name).as_ptr(),
                FILE_READ_DATA | FILE_WRITE_DATA | SYNCHRONIZE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION,
                null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            break unsafe { Handle::new(handle)? };
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) || Instant::now() >= deadline {
            return Err(error).context("持续防护服务未运行或不可访问");
        }
        unsafe {
            WaitNamedPipeW(wide(name).as_ptr(), 100);
        }
    };
    // 密码写入管道之前验证服务进程身份，拒绝普通账户抢占服务名。
    verify_server(pipe.raw(), console_sid.unwrap_or(SYSTEM_SID))?;
    let stop = AtomicBool::new(false);
    write_frame(pipe.raw(), &bytes, IO_TIMEOUT, &stop)?;
    let response = read_frame(pipe.raw(), CLIENT_TIMEOUT, &stop)?;
    // 服务端收到确认后才断开，避免DisconnectNamedPipe丢弃尚未读取的响应。
    write_frame(pipe.raw(), b"ok", IO_TIMEOUT, &stop)?;
    let response: Response = serde_json::from_slice(&response).context(ErrorCode::InvalidInput)?;
    if response.version != PROTOCOL_VERSION {
        bail!(ErrorCode::UnsupportedFormat);
    }
    Ok(response)
}

struct Stores {
    root: PathBuf,
    owner: String,
    _directory: File,
    directories: BTreeMap<String, File>,
    stores: BTreeMap<String, ManagedStore>,
}
impl Stores {
    fn open(root: PathBuf, owner: String, recover: bool) -> Result<Self> {
        let directory = private_directory(&root, &owner)?;
        let mut stores = Self {
            root,
            owner,
            _directory: directory,
            directories: BTreeMap::new(),
            stores: BTreeMap::new(),
        };
        for entry in fs::read_dir(&stores.root)? {
            let entry = entry?;
            let sid = entry
                .file_name()
                .to_str()
                .context(ErrorCode::ProtectionConflict)?
                .to_owned();
            if !valid_sid(&sid)
                || !entry.file_type()?.is_dir()
                || stores.stores.len() >= MAX_ACCOUNTS
            {
                bail!(ErrorCode::ProtectionConflict);
            }
            stores.load_with_recovery(&sid, recover)?;
        }
        Ok(stores)
    }
    fn load(&mut self, sid: &str) -> Result<&mut ManagedStore> {
        self.load_with_recovery(sid, true)
    }
    fn load_with_recovery(&mut self, sid: &str, recover: bool) -> Result<&mut ManagedStore> {
        if !valid_sid(sid) {
            bail!(ErrorCode::PermissionDenied);
        }
        if !self.stores.contains_key(sid) {
            if self.stores.len() >= MAX_ACCOUNTS {
                bail!(ErrorCode::Busy);
            }
            let directory_path = self.root.join(sid);
            let directory = private_directory(&directory_path, &self.owner)?;
            let mut store = ManagedStore::load(directory_path.join("managed.json"), sid)?;
            if recover {
                let recovered = store.recover_startup();
                if sweep_failed(&recovered)? {
                    eprintln!("managed_service_startup_recovery_pending");
                }
            }
            self.directories.insert(sid.to_owned(), directory);
            self.stores.insert(sid.to_owned(), store);
        }
        self.stores.get_mut(sid).context(ErrorCode::Internal)
    }
    fn all_empty(&self) -> bool {
        self.stores.values().all(|store| store.list().is_empty())
    }
}
fn sweep_failed(report: &crate::managed::SweepReport) -> Result<bool> {
    Ok(report.failed > 0)
}
fn overlapping(a: &Path, b: &Path) -> bool {
    // 与核心保持verbatim磁盘前缀、大小写和分隔符边界一致。
    crate::is_same_or_child(&crate::path_key(a), &crate::path_key(b))
        || crate::is_same_or_child(&crate::path_key(b), &crate::path_key(a))
}
impl Backend for Stores {
    fn handle(
        &mut self,
        client: &ClientContext,
        command: &Command,
        authorization: Option<&AuthorizedTree>,
    ) -> Result<Value> {
        if client.state_dir != self.root.join(&client.sid) {
            bail!(ErrorCode::PermissionDenied);
        }
        if let Command::CheckLocal { path } = command {
            if !valid_local_check_path(path) {
                bail!(ErrorCode::InvalidInput);
            }
            if self.stores.values().any(|store| {
                store
                    .registered_paths()
                    .iter()
                    .any(|registered| overlapping(path, registered))
            }) {
                bail!(ErrorCode::ProtectionServiceOverlap);
            }
            // 只读比较，尤其不能为新客户端创建SID目录或state。
            return Ok(serde_json::json!({"available": true}));
        }
        if let Command::Enroll { .. } = command {
            let authorized = authorization.context(ErrorCode::PermissionDenied)?;
            let root = &authorized
                .paths
                .first()
                .context(ErrorCode::ProtectionInput)?
                .path;
            for (sid, store) in &self.stores {
                if sid != &client.sid
                    && store
                        .registered_paths()
                        .iter()
                        .any(|existing| overlapping(root, existing))
                {
                    bail!(ErrorCode::ProtectionConflict);
                }
            }
        }
        let store = self.load(&client.sid)?;
        match command {
            Command::CheckLocal { .. } => unreachable!("CheckLocal必须在load前处理"),
            Command::Status => {
                Ok(serde_json::json!({ "registered": store.list().len(), "roots": store.list() }))
            }
            Command::List => Ok(serde_json::to_value(store.list())?),
            Command::Enroll {
                mode,
                note,
                password,
                ..
            } => {
                let authorized = authorization.context(ErrorCode::PermissionDenied)?;
                let root = &authorized
                    .paths
                    .first()
                    .context(ErrorCode::ProtectionInput)?
                    .path;
                Ok(serde_json::to_value(store.enroll_authorized(
                    root,
                    *mode,
                    note,
                    password.as_deref(),
                    authorized.paths(),
                )?)?)
            }
            Command::Unlock {
                id,
                password,
                duration_seconds,
            } => {
                store.unlock(id, password, *duration_seconds, now()?)?;
                Ok(serde_json::json!({}))
            }
            Command::Relock { id } => {
                store.relock(id)?;
                Ok(serde_json::json!({}))
            }
            Command::Remove { id, password } => {
                store.restore(id, password.as_deref())?;
                Ok(serde_json::json!({}))
            }
            Command::SetPassword {
                id,
                password,
                new_password,
            } => {
                store.change_password(id, password, new_password.as_deref())?;
                Ok(serde_json::json!({}))
            }
            Command::SetContinuous {
                id,
                password,
                enabled,
            } => {
                store.set_monitoring(id, *enabled, password.as_deref())?;
                Ok(serde_json::json!({}))
            }
        }
    }
    fn tick(&mut self) -> Result<()> {
        let now = now()?;
        let mut failed = false;
        for store in self.stores.values_mut() {
            failed |= sweep_failed(&store.tick(now))?;
        }
        if failed {
            bail!(ErrorCode::Recovery);
        }
        Ok(())
    }
    fn shutdown(&mut self) -> Result<()> {
        let mut failed = false;
        for store in self.stores.values_mut() {
            failed |= sweep_failed(&store.prepare_stop())?;
        }
        if failed {
            bail!(ErrorCode::Recovery);
        }
        Ok(())
    }
}

fn serve<B: Backend>(
    mut backend: B,
    root: PathBuf,
    owner: String,
    name: &str,
    console: bool,
    stop: Arc<AtomicBool>,
    ready: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let pipe = create_pipe(name, &owner, console)?;
    // 启动核对成功后才报告RUNNING。
    let initial_error = backend.tick().err().map(|error| error_code(&error));
    let backend = Arc::new(Mutex::new(backend));
    let health = Arc::new(Mutex::new(initial_error));
    let ticker_backend = Arc::clone(&backend);
    let ticker_health = Arc::clone(&health);
    let ticker_stop = Arc::clone(&stop);
    let ticker = thread::spawn(move || -> Result<()> {
        let mut last_tick = Instant::now();
        while !ticker_stop.load(Ordering::Acquire) {
            if last_tick.elapsed() >= Duration::from_secs(1) {
                let result = ticker_backend
                    .lock()
                    .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))?
                    .tick();
                *ticker_health
                    .lock()
                    .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))? =
                    result.err().map(|error| error_code(&error));
                last_tick = Instant::now();
            }
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    });
    let run_result = (|| -> Result<()> {
        ready()?;
        while !stop.load(Ordering::Acquire) {
            if let Err(error) = connect(pipe.raw(), &stop) {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if error
                    .downcast_ref::<std::io::Error>()
                    .and_then(|error| error.raw_os_error())
                    .is_some_and(|code| {
                        [ERROR_NO_DATA, ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED]
                            .contains(&(code as u32))
                    })
                {
                    unsafe {
                        DisconnectNamedPipe(pipe.raw());
                    }
                    continue;
                }
                return Err(error).context("建立本机管道连接失败");
            }
            let result = (|| -> Result<Value> {
                let bytes = read_frame(pipe.raw(), IO_TIMEOUT, &stop)?;
                let request = decode_request(&bytes)?;
                let (sid, authorization) = as_client(pipe.raw(), |sid| {
                    if console && sid != owner {
                        bail!(ErrorCode::PermissionDenied);
                    }
                    let authorization = match &request.command {
                        Command::Enroll { path, .. } => Some(authorize_tree(path)?),
                        _ => None,
                    };
                    Ok((sid.to_owned(), authorization))
                })?;
                let client = ClientContext {
                    state_dir: root.join(&sid),
                    sid,
                };
                backend
                    .lock()
                    .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))?
                    .handle(&client, &request.command, authorization.as_ref())
            })();
            let last_tick_error = *health
                .lock()
                .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))?;
            let response = Response {
                version: PROTOCOL_VERSION,
                transport: TransportStatus {
                    host: if console { "console" } else { "service" }.to_owned(),
                    healthy: last_tick_error.is_none(),
                    last_tick_error,
                },
                data: result.as_ref().ok().cloned(),
                error: result.as_ref().err().map(error_code),
            };
            if let Err(error) = write_frame(
                pipe.raw(),
                &serde_json::to_vec(&response)?,
                IO_TIMEOUT,
                &stop,
            ) {
                if !stop.load(Ordering::Acquire) {
                    eprintln!("managed_service_reply_failed: {:?}", error_code(&error));
                }
            } else if let Err(error) = read_frame(pipe.raw(), IO_TIMEOUT, &stop).and_then(|ack| {
                if ack.as_slice() != b"ok" {
                    bail!(ErrorCode::InvalidInput);
                }
                Ok(())
            }) && !stop.load(Ordering::Acquire)
            {
                eprintln!("managed_service_ack_failed: {:?}", error_code(&error));
            }
            unsafe {
                DisconnectNamedPipe(pipe.raw());
            }
        }
        Ok(())
    })();
    stop.store(true, Ordering::Release);
    let ticker_result = ticker
        .join()
        .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))?;
    let shutdown_result = backend
        .lock()
        .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))?
        .shutdown();
    run_result?;
    ticker_result?;
    shutdown_result
}

pub(super) fn run_console(
    state_dir: PathBuf,
    pipe_name: &str,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let sid = process_sid()?;
    if sid == SYSTEM_SID || !state_dir.is_absolute() {
        bail!(ErrorCode::PermissionDenied);
    }
    validate_pipe_name(pipe_name, true)?;
    let stores = Stores::open(state_dir.clone(), sid.clone(), true)?;
    serve(stores, state_dir, sid, pipe_name, true, stop, || {
        eprintln!("managed_console_ready");
        Ok(())
    })
}
pub(super) fn audit_empty() -> Result<()> {
    let directory = default_directory()?;
    if !directory.exists() {
        return Ok(());
    }
    let stores = Stores::open(directory, SYSTEM_SID.to_owned(), false)?;
    if !stores.all_empty() {
        bail!("仍有受管项目。请先验证密码并解除全部账户的服务保护，再卸载服务。");
    }
    Ok(())
}
fn report_status(state: u32, exit_code: u32) -> Result<()> {
    let handle = SERVICE_STATUS_HANDLE.load(Ordering::Acquire) as *mut c_void;
    if handle.is_null() {
        bail!(ErrorCode::Internal);
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: u32::from(state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING),
        dwWaitHint: if state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING {
            10_000
        } else {
            0
        },
    };
    unsafe { checked(SetServiceStatus(handle, &status)) }
}
unsafe extern "system" fn handler(
    control: u32,
    _event: u32,
    _data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            SERVICE_STOP.store(true, Ordering::Release);
            if report_status(SERVICE_STOP_PENDING, NO_ERROR).is_err() {
                return ERROR_SERVICE_SPECIFIC_ERROR;
            }
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let status = unsafe {
        RegisterServiceCtrlHandlerExW(wide(SERVICE_NAME).as_ptr(), Some(handler), null())
    };
    if status.is_null() {
        return;
    }
    SERVICE_STATUS_HANDLE.store(status as usize, Ordering::Release);
    let result = std::panic::catch_unwind(|| -> Result<()> {
        report_status(SERVICE_START_PENDING, NO_ERROR)?;
        if process_sid()? != SYSTEM_SID {
            bail!(ErrorCode::PermissionDenied);
        }
        let root = default_directory()?;
        let stores = Stores::open(root.clone(), SYSTEM_SID.to_owned(), true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let mirror = Arc::clone(&stop);
        let mirror_thread = thread::spawn(move || {
            while !mirror.load(Ordering::Acquire) {
                if SERVICE_STOP.load(Ordering::Acquire) {
                    mirror.store(true, Ordering::Release);
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        let result = serve(
            stores,
            root,
            SYSTEM_SID.to_owned(),
            PIPE_NAME,
            false,
            Arc::clone(&stop),
            || report_status(SERVICE_RUNNING, NO_ERROR),
        );
        stop.store(true, Ordering::Release);
        mirror_thread
            .join()
            .map_err(|_| anyhow::anyhow!(ErrorCode::Internal))?;
        result
    });
    let exit_code = if matches!(result, Ok(Ok(()))) {
        NO_ERROR
    } else {
        ERROR_SERVICE_SPECIFIC_ERROR
    };
    if let Err(error) = report_status(SERVICE_STOPPED, exit_code) {
        eprintln!("managed_service_status_failed: {:?}", error_code(&error));
    }
}
pub(super) fn run_service() -> Result<()> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    unsafe {
        checked(StartServiceCtrlDispatcherW(table.as_ptr()))
            .context("必须由Windows服务管理器启动；本地调试请显式使用--console")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn test_pipe() -> String {
        let nonce: u64 = rand::random();
        format!(
            r"\\.\pipe\FoldSentinel.test.{}.{}",
            std::process::id(),
            nonce
        )
    }

    #[test]
    fn real_console_check_local_is_read_only_and_rejects_all_overlapping_ranges() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let state = fixture.path().join("state");
        let target = fixture.path().join("service-root");
        fs::create_dir(&target)?;
        fs::write(target.join("original.txt"), "original-content")?;
        let canonical_target = fs::canonicalize(&target)?;
        let sid = process_sid()?;
        let pipe = test_pipe();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_sender, ready_receiver) = mpsc::channel();
        let server_state = state.clone();
        let server_sid = sid.clone();
        let server_pipe = pipe.clone();
        let server_stop = Arc::clone(&stop);
        let server = thread::spawn(move || -> Result<()> {
            let stores = Stores::open(server_state.clone(), server_sid.clone(), true)?;
            serve(
                stores,
                server_state,
                server_sid,
                &server_pipe,
                true,
                server_stop,
                || ready_sender.send(()).context("test ready closed"),
            )
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            ready_receiver.recv_timeout(Duration::from_secs(10))?;
            let call = |command| request_to(&Request::new(command), &pipe, Some(&sid));
            let missing = canonical_target
                .parent()
                .unwrap()
                .join("missing-not-created")
                .join("child");
            assert_eq!(
                call(Command::CheckLocal {
                    path: missing.clone()
                })?
                .into_result()?,
                serde_json::json!({"available": true})
            );
            assert_eq!(
                fs::read_dir(&state)?.count(),
                0,
                "read-only check must not create per-SID state"
            );
            assert!(!missing.exists());
            call(Command::Enroll {
                path: target.clone(),
                mode: ProtectionMode::ReadonlyAcl,
                note: String::new(),
                password: None,
            })?
            .into_result()?;
            let local_state = fixture.path().join("local-state.json");
            let mut local = crate::StateStore::load_from(local_state.clone())?;
            let check_pipe = pipe.clone();
            let check_sid = sid.clone();
            local.local_check = Some(Box::new(move |paths| {
                ensure_local_available_using(
                    paths,
                    || Ok(true),
                    |path| {
                        let mut response = request_to(
                            &Request::new(Command::CheckLocal {
                                path: path.to_owned(),
                            }),
                            &check_pipe,
                            Some(&check_sid),
                        )?;
                        // 仅此隔离测试把已验证同账户的console标签适配为正式契约。
                        response.transport.host = "service".to_owned();
                        Ok(response)
                    },
                )
            }));
            let root_attributes = crate::attributes::read(&canonical_target)?;
            for path in [
                canonical_target.clone(),
                canonical_target.parent().unwrap().to_owned(),
                canonical_target.join("original.txt"),
            ] {
                let error = local
                    .protect(&path, ProtectionMode::Hidden)
                    .expect_err("服务已登记时，本地同根及父子范围必须在写入前拒绝");
                assert_eq!(error_code(&error), ErrorCode::ProtectionServiceOverlap);
                assert!(!local_state.exists());
                assert_eq!(local.entries().count(), 0);
                assert_eq!(crate::attributes::read(&canonical_target)?, root_attributes);
            }
            for path in [
                canonical_target.clone(),
                canonical_target.parent().unwrap().to_owned(),
                canonical_target.join("missing-child"),
                PathBuf::from(
                    canonical_target
                        .to_string_lossy()
                        .trim_start_matches(r"\\?\")
                        .to_lowercase(),
                ),
            ] {
                let response = call(Command::CheckLocal { path })?;
                assert!(response.data.is_none());
                assert_eq!(response.error, Some(ErrorCode::ProtectionServiceOverlap));
                assert!(!serde_json::to_string(&response)?.contains("original.txt"));
            }
            let sibling = canonical_target.with_file_name("service-root-sibling");
            assert_eq!(
                call(Command::CheckLocal { path: sibling })?.into_result()?,
                serde_json::json!({"available":true})
            );
            assert_eq!(
                call(Command::CheckLocal {
                    path: missing.clone()
                })?
                .into_result()?,
                serde_json::json!({"available":true})
            );
            assert!(!missing.exists());
            Ok(())
        }));
        stop.store(true, Ordering::Release);
        let server_result = server
            .join()
            .map_err(|_| anyhow::anyhow!("console test thread failed"));
        let cleanup = (|| -> Result<()> {
            let mut recovery = ManagedStore::load(state.join(&sid).join("managed.json"), &sid)?;
            for root in recovery.list() {
                recovery.restore(&root.id, None)?;
            }
            Ok(())
        })();
        if let Err(error) = cleanup {
            let retained = fixture.keep();
            bail!(
                "read-only check fixture recovery failed, retained {}: {error:#}",
                retained.display()
            );
        }
        server_result??;
        match result {
            Ok(result) => result?,
            Err(panic) => std::panic::resume_unwind(panic),
        }
        assert_eq!(
            fs::read_to_string(target.join("original.txt"))?,
            "original-content"
        );
        Ok(())
    }

    #[test]
    fn check_local_compares_other_sid_without_loading_requester() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let sid = process_sid()?;
        let other_sid = "S-1-5-21-100-200-300-400";
        let mut stores = Stores::open(fixture.path().join("state"), sid.clone(), false)?;
        let target = fixture.path().join("other-account-root");
        fs::create_dir(&target)?;
        let authorization = authorize_tree(&target)?;
        stores.load(other_sid)?.enroll_authorized(
            &target,
            ProtectionMode::ReadonlyAcl,
            "",
            None,
            authorization.paths(),
        )?;
        drop(authorization);
        let client = ClientContext {
            sid: sid.clone(),
            state_dir: stores.root.join(&sid),
        };
        let check = stores.handle(
            &client,
            &Command::CheckLocal {
                path: fs::canonicalize(&target)?.join("not-existing"),
            },
            None,
        );
        let cleanup = stores.stores.get_mut(other_sid).unwrap();
        for root in cleanup.list() {
            cleanup.restore(&root.id, None)?;
        }
        assert_eq!(
            error_code(&check.unwrap_err()),
            ErrorCode::ProtectionServiceOverlap
        );
        assert!(!stores.stores.contains_key(&sid));
        assert!(!stores.root.join(&sid).exists());
        Ok(())
    }

    #[test]
    fn private_directory_refuses_preexisting_inherited_acl() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let sid = process_sid()?;
        let private = fixture.path().join("private");
        drop(private_directory(&private, &sid)?);
        drop(private_directory(&private, &sid)?);
        let unsafe_directory = fixture.path().join("inherited");
        fs::create_dir(&unsafe_directory)?;
        assert!(private_directory(&unsafe_directory, &sid).is_err());
        Ok(())
    }

    #[test]
    fn authorization_refuses_hardlinks_and_untrusted_server() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("original.txt");
        let linked = fixture.path().join("linked.txt");
        fs::write(&original, "unchanged")?;
        fs::hard_link(&original, &linked)?;
        assert!(authorize_tree(&original).is_err());
        assert!(authorize_tree(Path::new(r"\\server\share\root")).is_err());
        let sid = process_sid()?;
        let name = test_pipe();
        let pipe = create_pipe(&name, &sid, true)?;
        let server = thread::spawn(move || -> Result<()> {
            let stop = AtomicBool::new(false);
            if let Err(error) = connect(pipe.raw(), &stop) {
                if error
                    .downcast_ref::<std::io::Error>()
                    .and_then(|error| error.raw_os_error())
                    == Some(ERROR_NO_DATA as i32)
                {
                    return Ok(());
                }
                return Err(error);
            }
            // 伪服务未获得任何密码或请求字节，客户端即拒绝宿主SID。
            let error = read_frame(pipe.raw(), IO_TIMEOUT, &stop)
                .err()
                .context("客户端不应发送请求")?;
            assert!(
                error
                    .chain()
                    .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
            );
            Ok(())
        });
        let request = Request::new(Command::Unlock {
            id: "abc".to_owned(),
            password: "never-send-this".to_owned(),
            duration_seconds: 300,
        });
        let result = request_to(&request, &name, None);
        assert_eq!(
            error_code(&result.err().context("必须拒绝普通账户宿主")?),
            ErrorCode::PermissionDenied
        );
        server
            .join()
            .map_err(|_| anyhow::anyhow!("伪服务测试线程失败"))??;
        Ok(())
    }

    #[test]
    fn real_console_pipe_authenticates_enrolls_unlocks_relocks_and_recovers() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let state = fixture.path().join("state");
        let target = fixture.path().join("原生服务测试");
        fs::create_dir(&target)?;
        let file = target.join("original.txt");
        fs::write(&file, "original-content")?;
        let sid = process_sid()?;
        let pipe = test_pipe();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server_state = state.clone();
        let server_pipe = pipe.clone();
        let server_sid = sid.clone();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let server = thread::spawn(move || -> Result<()> {
            let stores = Stores::open(server_state.clone(), server_sid.clone(), true)?;
            serve(
                stores,
                server_state,
                server_sid,
                &server_pipe,
                true,
                server_stop,
                || ready_sender.send(()).context("测试就绪通道已关闭"),
            )
        });
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<String> {
                ready_receiver.recv_timeout(Duration::from_secs(10))?;
                let call = |command| -> Result<Value> {
                    request_to(&Request::new(command), &pipe, Some(&sid))?.into_result()
                };
                assert_eq!(call(Command::List)?, serde_json::json!([]));
                let enrolled = call(Command::Enroll {
                    path: target.clone(),
                    mode: ProtectionMode::ReadonlyAcl,
                    note: "测试备注".to_owned(),
                    password: Some("password-123".to_owned()),
                })?;
                let id = enrolled
                    .get("id")
                    .and_then(Value::as_str)
                    .context("未返回登记ID")?
                    .to_owned();
                assert_eq!(fs::read_to_string(&file)?, "original-content");
                assert!(fs::write(&file, "forbidden").is_err());
                assert!(fs::write(target.join("new.txt"), "forbidden").is_err());
                assert!(
                    call(Command::Unlock {
                        id: id.clone(),
                        password: "wrong-pass".to_owned(),
                        duration_seconds: 300
                    })
                    .is_err()
                );
                assert!(fs::write(&file, "forbidden").is_err());
                call(Command::Unlock {
                    id: id.clone(),
                    password: "password-123".to_owned(),
                    duration_seconds: 300,
                })?;
                fs::write(&file, "modified-after-authentication")?;
                call(Command::Relock { id: id.clone() })?;
                assert!(fs::write(&file, "forbidden").is_err());
                assert!(
                    call(Command::Remove {
                        id: id.clone(),
                        password: Some("wrong-pass".to_owned())
                    })
                    .is_err()
                );
                let list = call(Command::List)?;
                let serialized = serde_json::to_string(&list)?;
                assert!(
                    !serialized.contains("original.txt")
                        && !serialized.contains(&target.to_string_lossy().to_string())
                );
                assert!(!serialized.contains("D:P") && !serialized.contains("password-123"));
                call(Command::Unlock {
                    id: id.clone(),
                    password: "password-123".to_owned(),
                    duration_seconds: 300,
                })?;
                Ok(id)
            }));
        stop.store(true, Ordering::Release);
        let server_result = server.join().map_err(|_| anyhow::anyhow!("宿主线程失败"));
        // 无论断言结果如何都从隔离恢复记录恢复，避免测试故障留下受限临时文件。
        let was_relocked = fs::write(&file, "forbidden").is_err();
        let recovery_result = (|| -> Result<()> {
            let mut recovery = ManagedStore::load(state.join(&sid).join("managed.json"), &sid)?;
            for root in recovery.list() {
                recovery.restore(&root.id, Some("password-123"))?;
            }
            Ok(())
        })();
        if let Err(error) = recovery_result {
            let retained = fixture.keep();
            bail!(
                "服务隔离夹具恢复失败，已保留完整记录 {}: {error:#}",
                retained.display()
            );
        }
        server_result??;
        match result {
            Ok(result) => {
                result?;
            }
            Err(panic) => std::panic::resume_unwind(panic),
        }
        assert!(was_relocked, "停止宿主应在临时访问期间重锁");
        assert_eq!(fs::read_to_string(&file)?, "modified-after-authentication");
        fs::write(&file, "restored-write-access")?;
        Ok(())
    }
}
