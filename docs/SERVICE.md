# 持续防护服务

服务代码提供本机命名管道、按Windows账户隔离的恢复状态、临时访问截止处理和持续扫描。主窗口关闭不停止服务。服务默认以SYSTEM运行，限制ACE使用经过管道令牌验证的目标账户SID；密码只传递到本机服务并在内存使用，不写命令行、日志或历史。

历史上曾经用户明确确认，在本机安装并验证过 LocalSystem 自动启动服务。[真实SYSTEM验收](validation/managed-system-service-windows.json)通过普通账户生产管道完成密码、到期重锁、后台漂移修复、SCM停止/启动及异常退出自动恢复，测试结束时全部项目解除。此报告只记录当时状态，不代表当前仍已安装，也不覆盖 0.9.1 新安装流程。第二账户、整机重启及新服务安装/卸载验收见[最新验证记录](VALIDATION.md)。

## 安装和运行

0.9.1 安装器已包含服务载荷。在选项页勾选“安装 SYSTEM 持续防护服务”并同意 UAC，主程序内置入口会验证同批服务程序散列，复制到固定 Program Files 私有目录，配置 LocalSystem、自动启动和失败重启，并进行真实管道身份与协议验证。默认不勾选；静默/被动安装不安装服务。取消授权或服务部署失败会明确报告主程序已安装、服务未完成，可保留资料后重试。整个安装不需要另下修补程序或安装 PowerShell。

若安装时未选择服务，也可在应用“文件保护 → SYSTEM 服务与密码访问”中按需安装。SCM 明确显示未安装时出现“安装服务”按钮，点击才请求管理员授权，复用上述固定服务载荷和原生部署逻辑。安装期间显示进度并禁止重复操作；取消、失败和安装完成但尚未验证连接分别反馈，实际连接通过后才允许登记。页面浏览和刷新只查询状态，服务已存在、配置异常或查询权限不足时不会按未安装处理。

界面在连接失败后只读核对 Windows 服务状态，区分未安装、已停止、启动/停止中、配置异常、查询受限与运行但无法验证。只有原有管道请求通过才显示已连接。已有兼容官方服务不覆盖，停止状态可在明确选择安装服务的流程中启动；不兼容旧服务按下文先解除全部保护并正常卸载后更新。服务启动会按原有逻辑读取恢复计划并处理未完成临时访问，不能删除 ProgramData 来换取启动成功。

取消 UAC 时服务部署尚未执行，可重新选择安装。若复制程序后、登记服务前发生失败，固定程序目录会保留，重试安装会明确拒绝接管；可通过完整应用卸载器请求管理员清理已知程序目录，再重新安装。未知内容会阻止清理，不会递归删除。已有服务在注册后失败时也保留，按具体退出码核查后再启动或正常卸载。

便携包也可通过同目录的 `FoldSentinel.exe --install-service` 请求管理员安装；仅接受该固定参数，服务来源固定为同目录程序。以下 PowerShell 脚本保留为明确的管理员运维入口：

构建 `cargo build --release --bin foldsentinel-service --locked` 后，先核对来源、SHA256和要运行的代码。安装脚本把服务程序复制到Program Files下仅SYSTEM和Administrators可写的独立目录，拒绝覆盖既有目录或服务。管理员PowerShell可预览：

```powershell
.\scripts\manage-protection-service.ps1 -Action Install -ServiceBinary .\target\release\foldsentinel-service.exe -WhatIf
```

移除 `-WhatIf` 才执行安装。脚本不自动提权，服务设为自动启动，并配置失败重启。安装失败会明确退出，可能留下受保护的程序目录或已登记服务；先检查实际状态再恢复，脚本不会为回滚删除恢复资料。

上述脚本命令针对源码目录。便携包将 `foldsentinel-service.exe` 和 `manage-protection-service.ps1` 放在解压根目录；在那里使用 `./manage-protection-service.ps1`，并将来源改为 `./foldsentinel-service.exe`。普通 GUI 启动不会自动安装服务，安装器只按明确选项安装。正常卸载器发现服务后会另行请求管理员授权，只有所有账户均解除保护且全账户审计为空才移除服务、固定服务程序和对应 SCM 注册表登记，并核对登记实际消失。图形强制卸载也会请求管理员删除服务，但保留所有恢复资料，不恢复文件权限。应用随包携带的服务载荷、产品安装和启动登记也在卸载中清理并检查结果；未知注册表内容保留。

```powershell
.\scripts\manage-protection-service.ps1 -Action Status
.\scripts\manage-protection-service.ps1 -Action Stop
.\scripts\manage-protection-service.ps1 -Action Start
```

停止时先终止临时访问并重锁。已锁项目保持锁定，停止后不再补查新增项；必须检查服务退出码与应用逐项错误，不能把“进程已退出”当作全部重锁成功。无法重锁时保留恢复计划，重启服务继续恢复。直接结束进程或机器断电后，服务下次启动先恢复未完成的临时访问。

## 卸载和升级

服务恢复状态独立于旧版当前账户的 `state.json`，保存在系统ProgramData目录下，按认证后的SID分隔，仅SYSTEM与Administrators可访问。**不要手动删除或替换服务状态**，不要导入来源不可信的原始ACL。旧版保护需要先在旧恢复入口解除，再明确登记为服务管理；服务不会自动接管用户可修改的ACL基线。

卸载前分别由所有登记账户验证密码、解除全部服务受管项目。临时解锁、切换持续防护为关闭或服务停止，都不能代替解除登记。使用脚本时先停止服务并核对退出码，再在管理员 PowerShell 7 中执行：

```powershell
pwsh -NoProfile -File .\scripts\manage-protection-service.ps1 -Action Stop
pwsh -NoProfile -File .\scripts\manage-protection-service.ps1 -Action Uninstall
```

脚本要求服务处于 Stopped 且 Win32 与服务专用退出码均为 0，通过安装位置的固定程序 `--audit-empty` 只读核对所有账户均无受管根；存在项目、状态损坏、正在使用或无法验证时拒绝删除服务。服务配置必须指向带引号的固定 Program Files 程序，目录只含白名单 exe 且不含重解析点或未知文件。成功后删除服务登记、固定 exe 和空程序目录，保留 ProgramData 恢复资料供审计，不递归删除未知内容。脚本仍不自动升级已安装服务；升级先完成全部恢复与正常卸载，再核对新程序来源后重新安装。

图形正常卸载器经 UAC 调用受控的管理员入口。运行中的服务先做在线全账户空审计，通过后才请求停止；检查正常退出后，再由固定程序离线核对全部 SID 的恢复状态，确认无登记后才移除 SCM 登记与固定程序文件。停止后的二次审计失败时，尝试重新启动原先运行中的服务并报告结果，不因普通卸载被拒绝而默认关闭持续防护；不会用当前账户列表为空替代全账户核验。老服务不支持审计、程序丢失、状态不明或管理员取消授权时正常卸载停止。

服务注销后若文件占用导致删除失败，卸载会明确停止。释放占用后再次卸载，即使 SCM 已无服务，仍会请求管理员检查和清理固定程序目录，包含程序已删除但空目录残留的情况。只处理固定白名单文件与空目录，拒绝重解析点和未知内容，保留 ProgramData 恢复资料。

只在明确理解风险时，管理员交互终端可以使用 `-Action Uninstall -Force`：脚本显示保护可能永久遗留的免责声明，要求两次精确输入 `FORCE-FOLDSENTINEL`。`-NonInteractive` 或输入/输出重定向拒绝强制分支。图形强制卸载在同窗显示中英免责声明，两次确认均默认“否”，确认后仍需管理员 UAC 删除服务。强制路径须确认服务实际停止，但允许服务退出报告有错；不会将错误当作解除保护成功。两种强制入口都不恢复文件权限；ProgramData 与本地恢复资料保留，受保护文件可能继续不可访问，需重新安装相容版本并使用原密码和恢复资料处理。不能把静默 `-Force` 当作授权通道；服务删除取消或失败也不会报告成功。

脚本文件为 UTF-8 无 BOM；建议用 PowerShell 7 的 `pwsh -File` 执行，避免 Windows PowerShell 5.1 将中文脚本按系统 ANSI 解码。本轮脚本和 Rust 服务卸载检查只在隔离夹具中验证；真实 SCM 删除、UAC 和第二账户卸载仍见[验证记录](VALIDATION.md)中的未覆盖范围。

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
