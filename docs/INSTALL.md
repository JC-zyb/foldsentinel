# 安装、升级与发布

## Windows 安装与便携版

Windows 10/11 x64 需要 WebView2 Runtime。安装包 `FoldSentinel_0.3.0_x64-setup.exe` 按当前用户安装，可选择简体中文或 English；安装器语言与应用右上角语言独立。便携包解压后运行 `FoldSentinel.exe`，同目录的 `foldsentinel-cli.exe` 提供命令行入口。便携版仍把偏好和管理记录保存在当前 Windows 用户数据中，不是无痕模式。

升级前结束队列并关闭旧窗口，保留 `%APPDATA%\FoldSentinel\state.json`（原始属性）、`jobs`（中断记录），以及 WebView2 用户数据。用新安装包覆盖安装，或把便携版解压到新的空目录；不要在运行中替换 exe。v1/v2 加密格式及 KDF 参数保持兼容，新建仍为 v2。升级不会自动转换或删除加密包。

语言和最多 200 项任务描述保存在应用 WebView2 的 localStorage，键为 `foldsentinel.language` 与 `foldsentinel.history.v1`；用户数据目录由 WebView2/Tauri 管理。历史包含本机路径，不含密码或密钥。等待 / 运行记录在重启后显示为中断，重新输入密码后可重试。应用中的“清空历史”只清除终态任务描述，保留等待和运行任务；不会删除源数据、加密包、属性恢复记录或中断任务临时数据。

异常退出可能留下已认证的恢复明文。只通过中断任务清理入口或 CLI `vault cleanup <ID>` 清理所属临时数据；程序检查锁与所有者，不会删除最终输出。不要手动删除仍在运行的任务目录。没有断点续传，也不承诺断电事务或安全擦除。

## 校验下载

同一 Release 下载安装包或便携 ZIP 和 `SHA256SUMS.txt`，在 PowerShell 中执行：

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath .\FoldSentinel_0.3.0_x64-setup.exe
Get-FileHash -Algorithm SHA256 -LiteralPath .\FoldSentinel_0.3.0_windows-x64_portable.zip
```

逐项比对名称与哈希，须与发布页及校验文件一致。SHA-256 用于检测下载内容变化，不等价于代码签名或独立安全审计。当前产物未签名。

## 可重现的发布流程

使用锁定依赖安装 `npm ci`，运行 `npm test`、`npm run build`、`cargo fmt --all -- --check`、`cargo test --workspace --locked` 和 `cargo clippy --workspace --all-targets --locked -- -D warnings`。Windows CI 执行相同检查并调用 `scripts/package.ps1` 生成两个包及校验和。

本地执行 `./scripts/package.ps1` 构建 Rust GUI、CLI 与 NSIS，然后把 README、LICENSE、CHANGELOG 和 docs 一起打入便携包，产物位于 `artifacts/`。版本须在 Cargo、npm、锁文件与 tauri.conf.json 中一致。发布前验证双语完整流程、重启偏好和历史、取消与恢复；提交版本、等待对应提交的 Windows CI 成功，再将同一提交打版本标签并发布产物。记录实际验证与已知限制，不把旧提交的 CI 当作当前版本证据。
