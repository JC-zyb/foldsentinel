use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockRecord {
    pub version: u32,
    pub identity: [u64; 3],
    pub original: String,
    pub locked: String,
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
}

#[cfg(windows)]
mod windows {
    use super::*;
    use anyhow::{Context, bail};
    use std::fs::{File, OpenOptions};
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
                ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
                GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo,
            },
            *,
        },
        Storage::FileSystem::*,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    // 保留 READ_CONTROL / WRITE_DAC 和元数据访问，允许原账户恢复权限。
    // 不拒绝 GENERIC_READ/WRITE 共用的 SYNCHRONIZE。
    const DENIED: u32 = FILE_READ_DATA
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | DELETE
        | FILE_DELETE_CHILD;

    // 权限只读保留内容读取、目录枚举及执行；只拒绝实际写入和删除位。
    const READONLY_DENIED: u32 =
        FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | DELETE | FILE_DELETE_CHILD;

    struct Local(*mut core::ffi::c_void);
    impl Drop for Local {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }
    fn checked(code: u32) -> Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(code as i32).into())
        }
    }
    fn checked_bool(value: i32) -> Result<()> {
        if value != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().into())
        }
    }
    fn sddl(descriptor: *const core::ffi::c_void) -> Result<String> {
        let mut text = null_mut();
        let mut size = 0;
        unsafe {
            checked_bool(ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor as _,
                1,
                DACL_SECURITY_INFORMATION,
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
    fn descriptor(text: &str) -> Result<Local> {
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let mut value = null_mut();
        unsafe {
            checked_bool(ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut value,
                null_mut(),
            ))?;
        }
        Ok(Local(value))
    }
    fn dacl(value: &Local) -> Result<*mut ACL> {
        let mut present = 0;
        let mut acl = null_mut();
        let mut defaulted = 0;
        unsafe {
            checked_bool(GetSecurityDescriptorDacl(
                value.0,
                &mut present,
                &mut acl,
                &mut defaulted,
            ))?;
        }
        if present == 0 || acl.is_null() {
            bail!("不支持没有 DACL 的文件权限");
        }
        Ok(acl)
    }
    fn user_sid() -> Result<Vec<u32>> {
        unsafe {
            let mut token = null_mut();
            checked_bool(OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY,
                &mut token,
            ))?;
            let result = (|| {
                let mut size = 0;
                GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
                if size == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
                checked_bool(GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size,
                ))?;
                let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
                let length = GetLengthSid(user.User.Sid);
                let mut sid = vec![0u32; (length as usize).div_ceil(4)];
                checked_bool(CopySid(length, sid.as_mut_ptr().cast(), user.User.Sid))?;
                Ok(sid)
            })();
            CloseHandle(token);
            result
        }
    }

    fn explicit_sid(text: &str) -> Result<Vec<u32>> {
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let mut value = null_mut();
        unsafe {
            checked_bool(ConvertStringSidToSidW(wide.as_ptr(), &mut value))?;
            let value = Local(value);
            let length = GetLengthSid(value.0);
            let mut sid = vec![0u32; (length as usize).div_ceil(4)];
            checked_bool(CopySid(length, sid.as_mut_ptr().cast(), value.0))?;
            Ok(sid)
        }
    }

    pub fn current_sid() -> Result<String> {
        let sid = user_sid()?;
        let mut text = null_mut();
        unsafe {
            checked_bool(ConvertSidToStringSidW(sid.as_ptr() as _, &mut text))?;
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

    pub struct Object {
        file: File,
        path: std::path::PathBuf,
    }
    impl Object {
        pub fn open(path: &Path) -> Result<Self> {
            let file = OpenOptions::new()
                .access_mode(
                    READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
                )
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)
                .context("打开权限恢复句柄失败")?;
            let object = Self {
                file,
                path: path.to_owned(),
            };
            let info = object.info()?;
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.nNumberOfLinks > 1
            {
                bail!("锁定不支持重解析点或多个硬链接");
            }
            let mut name = [0u16; 32];
            unsafe {
                checked_bool(GetVolumeInformationByHandleW(
                    object.handle(),
                    null_mut(),
                    0,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                ))?;
            }
            if String::from_utf16_lossy(&name[..4]) != "NTFS" {
                bail!("锁定仅支持 NTFS 文件系统");
            }
            Ok(object)
        }
        fn handle(&self) -> *mut core::ffi::c_void {
            self.file.as_raw_handle()
        }
        fn info(&self) -> Result<BY_HANDLE_FILE_INFORMATION> {
            let mut info = unsafe { std::mem::zeroed() };
            unsafe {
                checked_bool(GetFileInformationByHandle(self.handle(), &mut info))?;
            }
            Ok(info)
        }
        pub fn identity(&self) -> Result<[u64; 3]> {
            let info = self.info()?;
            Ok([
                info.dwVolumeSerialNumber as u64,
                (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
                (info.ftCreationTime.dwHighDateTime as u64) << 32
                    | info.ftCreationTime.dwLowDateTime as u64,
            ])
        }
        pub fn attributes(&self) -> Result<u32> {
            Ok(self.info()?.dwFileAttributes)
        }
        pub fn write_attributes(&self, attributes: u32) -> Result<()> {
            let info = FILE_BASIC_INFO {
                CreationTime: 0,
                LastAccessTime: 0,
                LastWriteTime: 0,
                ChangeTime: 0,
                FileAttributes: attributes,
            };
            unsafe {
                checked_bool(SetFileInformationByHandle(
                    self.handle(),
                    FileBasicInfo,
                    (&info as *const FILE_BASIC_INFO).cast(),
                    size_of::<FILE_BASIC_INFO>() as u32,
                ))?;
            }
            if self.attributes()? != attributes {
                bail!("属性写入结果与恢复计划不符");
            }
            Ok(())
        }
        pub fn resolved_path(&self) -> Result<std::path::PathBuf> {
            let length = unsafe { GetFinalPathNameByHandleW(self.handle(), null_mut(), 0, 0) };
            if length == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = unsafe {
                GetFinalPathNameByHandleW(
                    self.handle(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    0,
                )
            };
            if written == 0 || written as usize >= buffer.len() {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(std::path::PathBuf::from(String::from_utf16(
                &buffer[..written as usize],
            )?))
        }
        pub fn read(&self) -> Result<String> {
            let mut value = null_mut();
            unsafe {
                checked(GetSecurityInfo(
                    self.handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    &mut value,
                ))?;
            }
            let value = Local(value);
            sddl(value.0)
        }
        pub fn read_parent(path: &Path) -> Result<String> {
            let parent = path.parent().context("路径缺少父目录")?;
            let file = OpenOptions::new()
                .access_mode(READ_CONTROL)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(parent)?;
            let mut value = null_mut();
            unsafe {
                checked(GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    &mut value,
                ))?;
            }
            let value = Local(value);
            sddl(value.0)
        }
        pub fn prepare(&self) -> Result<LockRecord> {
            self.prepare_readonly(false)
        }
        pub fn prepare_readonly(&self, readonly: bool) -> Result<LockRecord> {
            self.prepare_policy(readonly, None, None)
        }
        pub fn prepare_for_user(&self, readonly: bool, sid: &str) -> Result<LockRecord> {
            self.prepare_policy(readonly, None, Some(sid))
        }
        pub fn replace_policy(&self, record: &LockRecord, readonly: bool) -> Result<LockRecord> {
            self.verify(record)?;
            self.prepare_policy(readonly, Some(record), None)
        }
        fn prepare_policy(
            &self,
            readonly: bool,
            previous: Option<&LockRecord>,
            target_sid: Option<&str>,
        ) -> Result<LockRecord> {
            let original = match previous {
                Some(record) => record.original.clone(),
                None => self.read()?,
            };
            let value = descriptor(&original)?;
            let original_acl = dacl(&value)?;
            let sid = match target_sid {
                Some(sid) => explicit_sid(sid)?,
                None => user_sid()?,
            };
            unsafe {
                let length =
                    (*original_acl).AclSize as usize + 8 + GetLengthSid(sid.as_ptr() as _) as usize;
                let mut buffer = vec![0u32; length.div_ceil(4)];
                let locked_acl = buffer.as_mut_ptr().cast::<ACL>();
                checked_bool(InitializeAcl(
                    locked_acl,
                    (buffer.len() * 4) as u32,
                    ACL_REVISION_DS,
                ))?;
                // 子对象自底向上应用；锁定 ACE 不继承，不修改范围外对象。
                let denied = if readonly { READONLY_DENIED } else { DENIED }
                    | if !readonly && self.info()?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
                    {
                        FILE_EXECUTE
                    } else {
                        0
                    };
                checked_bool(AddAccessDeniedAceEx(
                    locked_acl,
                    ACL_REVISION_DS,
                    0,
                    denied,
                    sid.as_ptr() as _,
                ))?;
                for index in 0..(*original_acl).AceCount {
                    let mut ace = null_mut();
                    checked_bool(GetAce(original_acl, index as u32, &mut ace))?;
                    let header = &*ace.cast::<ACE_HEADER>();
                    if target_sid.is_none() && header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
                        continue;
                    }
                    let mut copy =
                        std::slice::from_raw_parts(ace.cast::<u8>(), header.AceSize as usize)
                            .to_vec();
                    if target_sid.is_none() {
                        copy[1] &= !(OBJECT_INHERIT_ACE
                            | CONTAINER_INHERIT_ACE
                            | NO_PROPAGATE_INHERIT_ACE
                            | INHERITED_ACE) as u8;
                    } else {
                        // 服务逐项补查新增内容；保留原允许ACE的继承规则，避免重锁父目录时
                        // 将尚未登记新子项的继承权限剥空。新增拒绝ACE自身仍不继承。
                        copy[1] &= !(INHERITED_ACE as u8);
                    }
                    checked_bool(AddAce(
                        locked_acl,
                        ACL_REVISION_DS,
                        u32::MAX,
                        copy.as_ptr().cast(),
                        copy.len() as u32,
                    ))?;
                }
                let mut sd: SECURITY_DESCRIPTOR = std::mem::zeroed();
                checked_bool(InitializeSecurityDescriptor(
                    (&mut sd as *mut SECURITY_DESCRIPTOR).cast(),
                    1,
                ))?;
                checked_bool(SetSecurityDescriptorDacl(
                    (&mut sd as *mut SECURITY_DESCRIPTOR).cast(),
                    1,
                    locked_acl,
                    0,
                ))?;
                checked_bool(SetSecurityDescriptorControl(
                    (&mut sd as *mut SECURITY_DESCRIPTOR).cast(),
                    SE_DACL_PROTECTED,
                    SE_DACL_PROTECTED,
                ))?;
                Ok(LockRecord {
                    version: 1,
                    identity: self.identity()?,
                    original,
                    parent: match previous {
                        Some(record) => record.parent.clone(),
                        None => Some(Self::read_parent(&self.path)?),
                    },
                    locked: sddl((&sd as *const SECURITY_DESCRIPTOR).cast())?,
                    previous: previous.map(|record| record.locked.clone()),
                })
            }
        }
        pub fn review_restore(&self, record: &LockRecord) -> Result<String> {
            if record.version != 1 || self.identity()? != record.identity {
                bail!(crate::vault::ErrorCode::ProtectionConflict);
            }
            // 确认恢复允许覆盖 DACL 漂移，但原始描述符和对象身份必须有效。
            let original = descriptor(&record.original)?;
            dacl(&original)?;
            self.read()
        }
        pub fn verify(&self, record: &LockRecord) -> Result<String> {
            if record.version != 1 || self.identity()? != record.identity {
                bail!(crate::vault::ErrorCode::ProtectionConflict);
            }
            let current = self.read()?;
            if !equivalent(&current, &record.original)
                && !equivalent(&current, &record.locked)
                && !record
                    .previous
                    .as_ref()
                    .is_some_and(|previous| equivalent(&current, previous))
            {
                bail!(crate::vault::ErrorCode::ProtectionConflict);
            }
            Ok(current)
        }
        pub fn write(&self, text: &str) -> Result<()> {
            let value = descriptor(text)?;
            let acl = dacl(&value)?;
            let mut control = 0;
            let mut revision = 0;
            unsafe {
                checked_bool(GetSecurityDescriptorControl(
                    value.0,
                    &mut control,
                    &mut revision,
                ))?;
                let flags = DACL_SECURITY_INFORMATION
                    | if control & SE_DACL_PROTECTED != 0 {
                        PROTECTED_DACL_SECURITY_INFORMATION
                    } else {
                        UNPROTECTED_DACL_SECURITY_INFORMATION
                    };
                checked(SetSecurityInfo(
                    self.handle(),
                    SE_FILE_OBJECT,
                    flags,
                    null_mut(),
                    null_mut(),
                    acl,
                    null(),
                ))?;
            }
            if !equivalent(&self.read()?, text) {
                bail!("权限写入结果与恢复记录不符，操作已停止");
            }
            Ok(())
        }
    }
}

// AI/AR 是 Windows 的自动继承计算标记，不改变 DACL 或是否禁止继承。
pub fn equivalent(a: &str, b: &str) -> bool {
    fn normalized(text: &str) -> String {
        let split = text.find('(').unwrap_or(text.len());
        text[..split].replace("AI", "").replace("AR", "") + &text[split..]
    }
    normalized(a) == normalized(b)
}
#[cfg(windows)]
pub use windows::{Object, current_sid};
#[cfg(not(windows))]
pub fn current_sid() -> Result<String> {
    anyhow::bail!(crate::vault::ErrorCode::ProtectionUnsupported)
}
#[cfg(not(windows))]
pub struct Object;
#[cfg(not(windows))]
impl Object {
    pub fn review_restore(&self, _: &LockRecord) -> Result<String> {
        anyhow::bail!("权限恢复仅支持 Windows NTFS")
    }
    pub fn read_parent(_: &Path) -> Result<String> {
        anyhow::bail!("锁定仅支持 Windows NTFS")
    }
    pub fn open(_: &Path) -> Result<Self> {
        anyhow::bail!("锁定仅支持 Windows NTFS")
    }
    pub fn prepare(&self) -> Result<LockRecord> {
        anyhow::bail!("锁定仅支持 Windows NTFS")
    }
    pub fn prepare_readonly(&self, _: bool) -> Result<LockRecord> {
        anyhow::bail!("权限只读仅支持 Windows NTFS")
    }
    pub fn prepare_for_user(&self, _: bool, _: &str) -> Result<LockRecord> {
        anyhow::bail!("权限保护仅支持 Windows NTFS")
    }
    pub fn resolved_path(&self) -> Result<std::path::PathBuf> {
        anyhow::bail!("权限保护仅支持 Windows NTFS")
    }
    pub fn attributes(&self) -> Result<u32> {
        anyhow::bail!("权限保护仅支持 Windows NTFS")
    }
    pub fn write_attributes(&self, _: u32) -> Result<()> {
        anyhow::bail!("权限保护仅支持 Windows NTFS")
    }
    pub fn replace_policy(&self, _: &LockRecord, _: bool) -> Result<LockRecord> {
        anyhow::bail!("权限只读仅支持 Windows NTFS")
    }
    pub fn read(&self) -> Result<String> {
        anyhow::bail!("锁定仅支持 Windows NTFS")
    }
    pub fn verify(&self, _: &LockRecord) -> Result<String> {
        anyhow::bail!("锁定仅支持 Windows NTFS")
    }
    pub fn write(&self, _: &str) -> Result<()> {
        anyhow::bail!("锁定仅支持 Windows NTFS")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn readonly_acl_allows_reading_and_listing_but_rejects_content_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("权限只读");
        fs::create_dir(&root).unwrap();
        let file = root.join("原始内容.txt");
        fs::write(&file, "保持原内容").unwrap();
        let directory = Object::open(&root).unwrap();
        let content = Object::open(&file).unwrap();
        let directory_plan = directory.prepare_readonly(true).unwrap();
        let content_plan = content.prepare_readonly(true).unwrap();
        content.write(&content_plan.locked).unwrap();
        directory.write(&directory_plan.locked).unwrap();

        let read = fs::read_to_string(&file);
        let list = fs::read_dir(&root);
        let write = fs::write(&file, "不应成功");
        let create = fs::write(root.join("不应新增.txt"), "不应成功");
        let delete = fs::remove_file(&file);

        directory.write(&directory_plan.original).unwrap();
        content.write(&content_plan.original).unwrap();
        assert_eq!(read.unwrap(), "保持原内容");
        assert!(list.is_ok());
        assert_eq!(
            write.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            create.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            delete.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "保持原内容");
        fs::write(&file, "恢复可写").unwrap();
    }

    #[test]
    fn switching_acl_policies_uses_original_and_accepts_saved_transition() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("切换.txt");
        fs::write(&file, "可恢复").unwrap();
        let object = Object::open(&file).unwrap();
        let locked = object.prepare().unwrap();
        object.write(&locked.locked).unwrap();
        let denied_read = fs::read(&file);
        let readonly = object.replace_policy(&locked, true).unwrap();
        assert!(
            readonly
                .previous
                .as_ref()
                .is_some_and(|old| equivalent(old, &locked.locked))
        );
        object.verify(&readonly).unwrap();
        object.write(&readonly.locked).unwrap();
        let allowed_read = fs::read_to_string(&file);
        let relocked = object.replace_policy(&readonly, false).unwrap();
        object.verify(&relocked).unwrap();
        object.write(&relocked.locked).unwrap();
        let denied_again = fs::read(&file);
        object.write(&relocked.original).unwrap();
        assert!(denied_read.is_err());
        assert_eq!(allowed_read.unwrap(), "可恢复");
        assert!(denied_again.is_err());
        assert!(equivalent(&locked.original, &relocked.original));
        assert!(equivalent(&locked.locked, &relocked.locked));
        assert_eq!(fs::read_to_string(&file).unwrap(), "可恢复");
    }
}
