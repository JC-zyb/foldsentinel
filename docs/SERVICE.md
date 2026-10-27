# 持续防护服务

服务代码提供本机命名管道、按Windows账户隔离的恢复状态、临时访问截止处理和持续扫描。主窗口关闭不停止服务。服务默认以SYSTEM运行，限制ACE使用经过管道令牌验证的目标账户SID；密码只传递到本机服务并在内存使用，不写命令行、日志或历史。

本轮经用户明确确认，已在本机安装并验证LocalSystem自动启动服务。[真实SYSTEM验收](validation/managed-system-service-windows.json)通过普通账户生产管道完成密码、到期重锁、后台漂移修复、SCM停止/启动及异常退出自动恢复；最后全部测试项目解除，服务保留运行且当前账户登记为空。第二账户、整机重启及卸载尚未验证。其他机器安装仍须核对来源、影响及用户授权。

## 安装和运行

构建 `cargo build --release --bin foldsentinel-service --locked` 后，先核对来源、SHA256和要运行的代码。安装脚本把服务程序复制到Program Files下仅SYSTEM和Administrators可写的独立目录，拒绝覆盖既有目录或服务。管理员PowerShell可预览：

```powershell
.\scripts\manage-protection-service.ps1 -Action Install -ServiceBinary .\target\release\foldsentinel-service.exe -WhatIf
```

移除 `-WhatIf` 才执行安装。脚本不自动提权，服务设为自动启动，并配置失败重启。安装失败会明确退出，可能留下受保护的程序目录或已登记服务；先检查实际状态再恢复，脚本不会为回滚删除恢复资料。

上述命令针对源码目录。便携包将 `foldsentinel-service.exe` 和 `manage-protection-service.ps1` 放在解压根目录；在那里使用 `./manage-protection-service.ps1`，并将来源改为 `./foldsentinel-service.exe`。普通GUI安装包不自动安装服务，卸载GUI也不会停止或删除独立服务。

```powershell
.\scripts\manage-protection-service.ps1 -Action Status
.\scripts\manage-protection-service.ps1 -Action Stop
.\scripts\manage-protection-service.ps1 -Action Start
```

停止时先终止临时访问并重锁。已锁项目保持锁定，停止后不再补查新增项；必须检查服务退出码与应用逐项错误，不能把“进程已退出”当作全部重锁成功。无法重锁时保留恢复计划，重启服务继续恢复。直接结束进程或机器断电后，服务下次启动先恢复未完成的临时访问。

## 卸载和升级

服务恢复状态独立于旧版当前账户的 `state.json`，保存在系统ProgramData目录下，按认证后的SID分隔，仅SYSTEM与Administrators可访问。**不要手动删除或替换服务状态**，不要导入来源不可信的原始ACL。旧版保护需要先在旧恢复入口解除，再明确登记为服务管理；服务不会自动接管用户可修改的ACL基线。

卸载前分别由所有登记账户验证密码、解除全部服务受管项目。然后停止服务，管理员执行：

```powershell
.\scripts\manage-protection-service.ps1 -Action Uninstall
```

脚本以安装位置的程序只读核对所有账户均无受管根；存在项目、状态损坏、正在使用或无法验证时拒绝删除服务。成功仅删除服务登记，保留程序目录和所有状态供审计。当前脚本不覆盖安装目录，因此不自动升级；升级需先完成全部恢复与卸载，再在明确核对与备份后人工处理保留的程序目录。未来安装包流程不能绕过此恢复门。

## 隔离调试

不安装服务时，可用当前账户显式启动前台宿主：

```powershell
.\target\debug\foldsentinel-service.exe --console --state-dir D:\isolated-fixture\service-state --pipe-name \\.\pipe\FoldSentinel.debug.unique
```

父目录必须已经存在，状态目录由宿主以私有DACL创建。禁止SYSTEM使用调试入口，禁止调试管道使用正式名称。调试客户端必须显式调用 `request_console`，只信任同一账户的宿主；正式客户端只信任SYSTEM管道服务器，不会自动降级到前台进程。用Ctrl+C直接结束前台宿主属于异常退出，下次启动会立即处理临时解锁的恢复计划；生产SCM停止路径则先执行重锁。

## 保护边界

服务每秒尝试一次扫描；单根扫描或文件系统调用耗时会增加间隔，范围内逐项失败有独立错误。它不是内核驱动，存在检测窗口，也不能撤销其他程序已打开的句柄。原所有者、同账户的其他程序和管理员仍可能修改DACL；密码是本应用服务接口的访问门，不能替代内容加密、Windows账户隔离或离线保护。

服务协议拒绝远程管道客户端、未知字段、客户端指定SID/状态路径/原ACL和超过1MiB的消息。新登记必须先以客户端模拟身份打开全树及祖先，拒绝重解析点、多硬链接和缺少实际权限的对象，持有不共享删除的句柄，再核对对象身份。跨账户范围重叠拒绝。服务没有任意备份导入/文件输出RPC，因此不会用SYSTEM向客户端指定目的地写恢复文件。

身份验证依据Microsoft的[命名管道模拟客户端](https://learn.microsoft.com/en-us/windows/win32/ipc/impersonating-a-named-pipe-client)、[管道安全与访问权限](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)及[服务入口](https://learn.microsoft.com/en-us/windows/win32/services/writing-a-servicemain-function)。模拟失败不会继续执行请求；`RevertToSelf`失败按[官方要求](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-reverttoself)终止进程，避免继续使用混淆的身份。

生产客户端发送密码前，将内核返回的管道服务端PID与SCM固定服务的Running/PID核对，并要求OwnProcess和LocalSystem配置。普通账户读取SYSTEM进程令牌需要额外特权，因此不采用该路径、不授予调试权限。实现依据[服务状态与PID](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-queryservicestatusex)、[服务配置查询](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-queryserviceconfigw)及[跨账户令牌查询限制](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocesstoken)。管理员修改服务配置属于管理员边界。
