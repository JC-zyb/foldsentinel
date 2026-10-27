# 保护能力与锁定设计状态

v0.6 的 NTFS ACL 锁定仍处于设计阶段，没有锁定或应用解锁密码入口。

| 能力 | 当前行为 | 边界 |
| --- | --- | --- |
| 隐藏 | 新策略设置 Hidden + System | 显示受保护系统文件或使用其他工具后仍可见；路径和内容仍存在 |
| 只读 | 为文件设置 Readonly 属性 | 文件夹不受该属性写保护；属性可由有权限的程序更改 |
| 加密包 | 使用密码创建独立认证加密副本 | 源文件仍保留；先验证并试恢复，再决定原件如何保管 |
| NTFS 锁定 | 设计中，尚未实现 | 不能据此宣称防删除、防同一账户其他进程或管理员访问 |

锁定实验需要先明确目标用户 SID 和受限操作，保存可恢复的原始 DACL、继承标记与文件身份，验证普通账户、提升权限账户、既有句柄、父目录删除权限和中断恢复。不能以对 Everyone 拒绝所有权限代替设计，也不能只在应用内检查密码就声称阻止其他进程访问。现阶段没有可用的隔离多账户 Windows 环境，因此不进入权限修改实验。

技术依据见 Microsoft 的[文件属性常量](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants)、[文件安全与访问权限](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights)、[DACL 与 ACE](https://learn.microsoft.com/en-us/windows/win32/secauthz/dacls-and-aces)。这些资料描述 Windows 机制，不能据此推断特定闭源软件的内部实现。
