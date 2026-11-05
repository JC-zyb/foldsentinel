//! 安装器调用的原生服务部署；源程序散列由 GUI 构建时绑定。
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};

const PROGRAM_DIRECTORY_SDDL: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const MAX_SERVICE_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(30);

fn state_from_scm(state: u32) -> ServiceInstallationState {
    match state {
        SERVICE_STOPPED => ServiceInstallationState::Stopped,
        SERVICE_START_PENDING => ServiceInstallationState::Starting,
        SERVICE_RUNNING => ServiceInstallationState::Running,
        SERVICE_STOP_PENDING => ServiceInstallationState::Stopping,
        _ => ServiceInstallationState::Invalid,
    }
}

fn fixed_config(handle: SC_HANDLE, binary: &Path) -> Result<bool> {
    match service_binary_path(handle) {
        Ok(path) => Ok(path.eq_ignore_ascii_case(&format!("\"{}\"", binary.display()))),
        Err(error) if error_code(&error) == ErrorCode::ProtectionConflict => Ok(false),
        Err(error) => Err(error),
    }
}

pub(in crate::service) fn installation_state() -> Result<ServiceInstallationState> {
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(std::io::Error::last_os_error()).context(ErrorCode::ServiceUnavailable);
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
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
            Ok(ServiceInstallationState::Absent)
        } else {
            Err(error).context(ErrorCode::ServiceUnavailable)
        };
    }
    let service = IdentityServiceHandle(service);
    let binary = program_files_directory()?.join("foldsentinel-service.exe");
    if !fixed_config(service.0, &binary)? {
        return Ok(ServiceInstallationState::Invalid);
    }
    Ok(state_from_scm(service_status(service.0)?.dwCurrentState))
}

fn refuse_reparse_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        if has_reparse_point(&metadata) {
            bail!(ErrorCode::ProtectionConflict);
        }
    }
    Ok(())
}

fn file_sha256(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verified_source(source: &Path, expected_sha256: &str) -> Result<File> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        bail!("服务程序缺少有效的构建绑定散列，拒绝安装");
    }
    refuse_reparse_ancestors(source)?;
    // 散列与复制使用同一文件句柄，阻止检查后替换、写入或删除源程序。
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(source)?;
    let info = file_info(&file)?;
    let length = file.metadata()?.len();
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || info.nNumberOfLinks != 1
        || !(1024..=MAX_SERVICE_BINARY_BYTES).contains(&length)
        || file_sha256(&mut file)? != expected_sha256
    {
        bail!("服务程序与本次应用构建不一致，拒绝安装");
    }
    Ok(file)
}

fn verify_program_acl(directory: &File) -> Result<()> {
    let mut actual = null_mut();
    unsafe {
        code_checked(GetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut actual,
        ))?;
    }
    let actual = Local(actual);
    if !restricted_program_descriptor(actual.0, true)? {
        bail!("服务程序目录权限不是仅 SYSTEM 与管理员可写，拒绝接管");
    }
    Ok(())
}

fn restricted_program_descriptor(descriptor: *mut c_void, directory: bool) -> Result<bool> {
    let mut system = null_mut();
    let mut administrators = null_mut();
    unsafe {
        checked(ConvertStringSidToSidW(
            wide(SYSTEM_SID).as_ptr(),
            &mut system,
        ))?;
    }
    let system = Local(system);
    unsafe {
        checked(ConvertStringSidToSidW(
            wide("S-1-5-32-544").as_ptr(),
            &mut administrators,
        ))?;
    }
    let administrators = Local(administrators);
    let mut control = 0;
    let mut revision = 0;
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = null_mut();
    let mut owner = null_mut();
    unsafe {
        checked(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))?;
        checked(GetSecurityDescriptorOwner(
            descriptor,
            &mut owner,
            &mut defaulted,
        ))?;
        checked(GetSecurityDescriptorDacl(
            descriptor,
            &mut present,
            &mut acl,
            &mut defaulted,
        ))?;
        if (directory && control & SE_DACL_PROTECTED == 0)
            || present == 0
            || acl.is_null()
            || owner.is_null()
            || (EqualSid(owner, system.0) == 0 && EqualSid(owner, administrators.0) == 0)
            || (*acl).AceCount != 2
        {
            return Ok(false);
        }
        let mut seen = 0;
        for index in 0..2 {
            let mut raw_ace = null_mut();
            checked(GetAce(acl, index, &mut raw_ace))?;
            let header = &*raw_ace.cast::<ACE_HEADER>();
            // ACCESS_ALLOWED_ACE 的类型为 0。拒绝额外权限、继承专用和非标准 ACE。
            let flags = header.AceFlags as u32;
            let flags_valid = if directory {
                flags == OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                flags & !INHERITED_ACE == 0
            };
            if header.AceType != 0
                || !flags_valid
                || (header.AceSize as usize) < size_of::<ACCESS_ALLOWED_ACE>()
            {
                return Ok(false);
            }
            let ace = &*raw_ace.cast::<ACCESS_ALLOWED_ACE>();
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            if ace.Mask != FILE_ALL_ACCESS || IsValidSid(sid) == 0 {
                return Ok(false);
            }
            let bit = if EqualSid(sid, system.0) != 0 {
                1
            } else if EqualSid(sid, administrators.0) != 0 {
                2
            } else {
                return Ok(false);
            };
            if seen & bit != 0 {
                return Ok(false);
            }
            seen |= bit;
        }
        Ok(seen == 3)
    }
}

fn hold_installed_binary(path: &Path) -> Result<File> {
    let binary = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_READ_DATA)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let info = file_info(&binary)?;
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || info.nNumberOfLinks != 1
    {
        bail!(ErrorCode::ProtectionConflict);
    }
    let mut descriptor = null_mut();
    unsafe {
        code_checked(GetSecurityInfo(
            binary.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        ))?;
    }
    let descriptor = Local(descriptor);
    if !restricted_program_descriptor(descriptor.0, false)? {
        bail!("服务程序自身权限异常，拒绝启动或复用");
    }
    Ok(binary)
}

fn hold_program_directory(path: &Path) -> Result<File> {
    refuse_reparse_ancestors(path)?;
    let directory = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let info = file_info(&directory)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        bail!(ErrorCode::ProtectionConflict);
    }
    verify_program_acl(&directory)?;
    Ok(directory)
}

fn create_program_directory(path: &Path) -> Result<File> {
    refuse_reparse_ancestors(path.parent().context(ErrorCode::InvalidInput)?)?;
    let descriptor = sd(PROGRAM_DIRECTORY_SDDL)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    unsafe {
        checked(CreateDirectoryW(wide(path).as_ptr(), &attributes))
            .context("服务程序目录已存在或无法创建；不会覆盖已有内容")?;
    }
    hold_program_directory(path)
}

fn copy_verified_binary(source: &mut File, destination: &Path, expected: &str) -> Result<()> {
    // 显式设置管理员所有者，避免特殊令牌默认所有者导致文件权限与目录不一致。
    let descriptor = sd("O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)")?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            wide(destination).as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error()).context("无法创建服务程序；拒绝覆盖已有文件");
    }
    let mut output = unsafe { File::from_raw_handle(handle) };
    source.seek(SeekFrom::Start(0))?;
    std::io::copy(source, &mut output)?;
    output.sync_all()?;
    if file_sha256(&mut output)? != expected {
        bail!("复制后的服务程序校验失败；未注册服务");
    }
    Ok(())
}

fn configure_new_service(service: SC_HANDLE) -> Result<()> {
    let mut description =
        wide("按 Windows 账户隔离的密码访问、到期重锁与持续防护；卸载前须解除全部受管项目。");
    let description = SERVICE_DESCRIPTIONW {
        lpDescription: description.as_mut_ptr(),
    };
    let mut actions = [5_000, 15_000, 60_000].map(|delay| SC_ACTION {
        Type: SC_ACTION_RESTART,
        Delay: delay,
    });
    let recovery = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: null_mut(),
        lpCommand: null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    let flag = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: 1,
    };
    unsafe {
        checked(ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_DESCRIPTION,
            (&description as *const SERVICE_DESCRIPTIONW).cast(),
        ))?;
        checked(ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            (&recovery as *const SERVICE_FAILURE_ACTIONSW).cast(),
        ))?;
        checked(ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&flag as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
        ))?;
    }
    Ok(())
}

fn start_and_verify(service: SC_HANDLE) -> Result<()> {
    match service_status(service)?.dwCurrentState {
        SERVICE_STOPPED => unsafe { checked(StartServiceW(service, 0, null()))? },
        SERVICE_START_PENDING | SERVICE_RUNNING => {}
        _ => bail!("服务正在停止或状态异常，请稍后重试；未覆盖服务程序"),
    }
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let status = service_status(service)?;
        match status.dwCurrentState {
            SERVICE_RUNNING => break,
            SERVICE_START_PENDING if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(250));
            }
            _ => bail!(
                "服务未能运行，Win32退出码={}，服务退出码={}；服务登记及恢复资料已保留",
                status.dwWin32ExitCode,
                status.dwServiceSpecificExitCode
            ),
        }
    }
    // 此命令在载入客户端 SID 之前处理，验证连接又不创建管理员账户的状态。
    let audit = super::super::uninstall_audit()?
        .context("服务运行但未能通过身份或协议验证；未删除恢复资料")?;
    if audit.has_errors {
        bail!("服务已运行，但已有恢复状态存在异常，请在应用中核查；恢复资料已保留");
    }
    Ok(())
}

pub(in crate::service) fn install_from(source: &Path, expected_sha256: &str) -> Result<()> {
    require_administrator()?;
    let mut source = verified_source(source, expected_sha256)?;
    let directory = program_files_directory()?;
    let binary = directory.join("foldsentinel-service.exe");
    let manager = unsafe {
        OpenSCManagerW(
            null(),
            null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
    };
    if manager.is_null() {
        return Err(std::io::Error::last_os_error()).context("无法访问 Windows 服务管理器");
    }
    let manager = IdentityServiceHandle(manager);
    let service = unsafe {
        OpenServiceW(
            manager.0,
            wide(SERVICE_NAME).as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START,
        )
    };
    if !service.is_null() {
        let service = IdentityServiceHandle(service);
        if !fixed_config(service.0, &binary)? {
            bail!("现有同名服务的配置不匹配，拒绝覆盖或启动");
        }
        let _directory = hold_program_directory(&directory)?;
        verify_program_directory(&directory)?;
        let _binary = hold_installed_binary(&binary)?;
        return start_and_verify(service.0);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
        return Err(error).context("无法确认已有服务状态，拒绝安装");
    }
    let _directory = create_program_directory(&directory)?;
    copy_verified_binary(&mut source, &binary, expected_sha256)?;
    let _binary = hold_installed_binary(&binary)?;
    let registered_path = wide(format!("\"{}\"", binary.display()));
    let service = unsafe {
        CreateServiceW(
            manager.0,
            wide(SERVICE_NAME).as_ptr(),
            wide("FoldSentinel 持续防护").as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_CHANGE_CONFIG,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            registered_path.as_ptr(),
            null(),
            null_mut(),
            null(),
            wide("LocalSystem").as_ptr(),
            null(),
        )
    };
    if service.is_null() {
        return Err(std::io::Error::last_os_error())
            .context("注册服务失败；已复制的程序保留，未修改恢复资料");
    }
    let service = IdentityServiceHandle(service);
    configure_new_service(service.0).context("服务已注册但配置失败，未自动删除服务或恢复资料")?;
    start_and_verify(service.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scm_states_are_distinct_and_unknown_is_invalid() {
        assert_eq!(
            state_from_scm(SERVICE_STOPPED),
            ServiceInstallationState::Stopped
        );
        assert_eq!(
            state_from_scm(SERVICE_START_PENDING),
            ServiceInstallationState::Starting
        );
        assert_eq!(
            state_from_scm(SERVICE_RUNNING),
            ServiceInstallationState::Running
        );
        assert_eq!(
            state_from_scm(SERVICE_STOP_PENDING),
            ServiceInstallationState::Stopping
        );
        assert_eq!(
            state_from_scm(SERVICE_PAUSED),
            ServiceInstallationState::Invalid
        );
        assert_eq!(state_from_scm(0), ServiceInstallationState::Invalid);
    }

    #[test]
    fn program_acl_accepts_legacy_order_and_auto_inheritance_but_refuses_other_writers()
    -> Result<()> {
        for valid in [
            PROGRAM_DIRECTORY_SDDL,
            "O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)",
            "O:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
        ] {
            assert!(
                restricted_program_descriptor(sd(valid)?.0, true)?,
                "{valid}"
            );
        }
        for invalid in [
            "O:BAD:(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BU)",
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;BU)",
            "O:BUD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;CI;FA;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;OICI;FR;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;SY)",
            "O:BAD:NO_ACCESS_CONTROL",
        ] {
            assert!(
                !restricted_program_descriptor(sd(invalid)?.0, true)?,
                "{invalid}"
            );
        }
        Ok(())
    }

    #[test]
    fn program_file_acl_accepts_inherited_entries_and_refuses_writable_file_override() -> Result<()>
    {
        for valid in [
            "O:BAD:AI(A;ID;FA;;;SY)(A;ID;FA;;;BA)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)",
        ] {
            assert!(
                restricted_program_descriptor(sd(valid)?.0, false)?,
                "{valid}"
            );
        }
        for invalid in [
            "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FW;;;BU)",
            "O:BUD:P(A;;FA;;;SY)(A;;FA;;;BA)",
            "O:BAD:P(A;IO;FA;;;SY)(A;;FA;;;BA)",
        ] {
            assert!(
                !restricted_program_descriptor(sd(invalid)?.0, false)?,
                "{invalid}"
            );
        }
        Ok(())
    }

    #[test]
    fn bound_source_rejects_mismatch_and_holds_verified_bytes_against_mutation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source_path = temp.path().join("service.exe");
        fs::write(&source_path, vec![0x5a; 4096])?;
        let expected = file_sha256(&mut File::open(&source_path)?)?;
        assert!(verified_source(&source_path, "").is_err());
        assert!(verified_source(&source_path, &"0".repeat(64)).is_err());
        let mut source = verified_source(&source_path, &expected)?;
        assert!(OpenOptions::new().write(true).open(&source_path).is_err());
        assert!(fs::remove_file(&source_path).is_err());
        // 不执行需要管理员拥有者权限的正式复制；核对相同源句柄能重新定位并读取。
        source.seek(SeekFrom::Start(0))?;
        let mut content = Vec::new();
        source.read_to_end(&mut content)?;
        assert_eq!(content, vec![0x5a; 4096]);
        Ok(())
    }

    #[test]
    fn binary_copy_refuses_existing_file_and_keeps_its_bytes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.exe");
        let destination = temp.path().join("existing.exe");
        fs::write(&source, vec![0x51; 2048])?;
        fs::write(&destination, b"existing program")?;
        let expected = file_sha256(&mut File::open(&source)?)?;
        let mut source = verified_source(&source, &expected)?;
        assert!(copy_verified_binary(&mut source, &destination, &expected).is_err());
        assert_eq!(fs::read(&destination)?, b"existing program");
        Ok(())
    }

    #[test]
    fn bound_source_refuses_hardlinks_without_touching_destination() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("service.exe");
        let alias = temp.path().join("alias.exe");
        fs::write(&source, vec![0x51; 2048])?;
        let expected = file_sha256(&mut File::open(&source)?)?;
        fs::hard_link(&source, &alias)?;
        assert!(verified_source(&source, &expected).is_err());
        assert_eq!(fs::read(&alias)?, vec![0x51; 2048]);
        Ok(())
    }
}
