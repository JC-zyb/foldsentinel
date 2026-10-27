# 安装、升级与发布

## Windows 安装与便携版

Windows 10/11 x64 需要 WebView2 Runtime。v0.5 的安装包名为 `FoldSentinel_0.5.0_x64-setup.exe`，按当前用户安装，可选择简体中文或 English；安装器语言与应用右上角语言独立。便携包解压后运行 `FoldSentinel.exe`，同目录的 `foldsentinel-cli.exe` 提供命令行入口。便携版仍把偏好和管理记录保存在当前 Windows 用户数据中，不是无痕模式。v0.5 当前为本地开发版本，是否已发布及可下载附件以实际 Release 为准。

升级前结束队列并关闭旧窗口，保留 `%APPDATA%\FoldSentinel\state.json`（原始属性）、`jobs`（中断记录），以及 WebView2 用户数据。用新安装包覆盖安装，或把便携版解压到新的空目录；不要在运行中替换 exe。v1/v2 加密格式及 KDF 参数保持兼容，新建仍为 v2。升级不会自动转换或删除加密包。

语言和最多 200 项任务描述保存在应用 WebView2 的 localStorage，键为 `foldsentinel.language` 与 `foldsentinel.history.v1`；用户数据目录由 WebView2/Tauri 管理。历史包含本机路径，不含密码或密钥。等待 / 运行记录在重启后显示为中断，重新输入密码后可重试。应用中的“清空历史”只清除终态任务描述，保留等待和运行任务；不会删除源数据、加密包、属性恢复记录或中断任务临时数据。

v0.5 保持上述标识、路径、键及记录结构。预览不写入历史、用户目录或导出诊断；重启、关闭预览、切换操作或运行新任务后须重新验证。单文件包格式没有原文件名，不会从包名称猜测原名。超过展示上限时仍完整验证整包，清单仅展示前 1,000 项和 256 KiB 名称。

隐藏策略会写入 Windows Hidden + System 属性。资源管理器默认隐藏受保护的操作系统文件，因此开启“显示隐藏的项目”时通常仍不会列出这些项目；关闭该保护选项或使用其他文件管理器后仍可见。需要内容机密性时请使用加密包，属性操作不提供安全边界。

异常退出可能留下已认证的恢复明文。启动提示引导查看中断记录：正在运行的任务不能清理；可清理记录展示源、输出和临时位置；损坏或位置异常的记录保留并提示人工核查，不影响其他记录列出。只通过中断任务清理入口或 CLI `vault cleanup <ID>` 清理所属临时数据；程序检查锁与所有者，不会删除最终输出。清理失败会保留记录，解除占用或恢复目标父目录后重试。不要手动删除仍在运行的任务目录。没有断点续传，也不承诺断电事务或安全擦除。

错误详情及主动导出的 JSON 采用字段白名单，不包含密码、密钥或本机完整路径；本机任务历史和中断列表仍需显示路径来定位文件。导出仅创建新的 `.json` 文件，不覆盖已有报告。默认卸载不等于清理所有用户数据：卸载器若提供“删除应用数据”选项，应先评估备份，特别是原始属性与中断恢复记录。

## 校验下载

同一 Release 下载安装包或便携 ZIP 和 `SHA256SUMS.txt`，在 PowerShell 中执行：

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath .\FoldSentinel_0.5.0_x64-setup.exe
Get-FileHash -Algorithm SHA256 -LiteralPath .\FoldSentinel_0.5.0_windows-x64_portable.zip
```

逐项比对名称与哈希，须与发布页及校验文件一致。SHA-256 用于检测下载内容变化，不等价于代码签名或独立安全审计。当前产物未签名。

## 可重现的发布流程

使用锁定依赖安装 `npm ci`，运行 `npm run check:versions`、`npm test`、`npm run build`、`cargo fmt --all -- --check`、`cargo test --workspace --locked` 和 `cargo clippy --workspace --all-targets --locked -- -D warnings`。Windows CI 执行相同检查、真实 IO/隔离卷写满，以及安装、固定 SHA-256 的 v0.4 基线升级和默认卸载检查；调用 `scripts/package.ps1` 生成两个包及校验和。安装验证检查真实 exe 启动及创建 WebView2 子进程和用户目录哨兵保留，不能替代交互验证、真实偏好兼容或无 WebView2 的干净虚拟机。

本地执行 `./scripts/package.ps1` 构建 Rust GUI、CLI 与 NSIS，然后把 README、LICENSE、CHANGELOG 和 docs 一起打入便携包，产物位于 `artifacts/<版本>/`，保留其他版本。脚本自动检查版本、ZIP 内容与两个 SHA-256；也可单独运行 `./scripts/verify-artifacts.ps1 -Directory ./artifacts/0.5.0`。版本须在 Cargo、npm、锁文件、tauri.conf.json 与界面中一致。发布前验证双语完整流程、重启偏好和历史、取消与恢复；提交版本、等待对应提交的 Windows CI 成功，再将同一提交打版本标签并发布产物。记录实际验证与已知限制，不把旧提交的 CI 当作当前版本证据。
