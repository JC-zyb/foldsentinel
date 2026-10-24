# FoldSentinel

文件夹哨兵：使用 Rust + Tauri 2 构建的开源 Windows 文件保护工具。首版支持文件 / 文件夹密码加密包，以及可恢复的隐藏、只读属性管理。

## 使用首版

在 [Releases](https://github.com/JC-zyb/foldsentinel/releases) 下载 Windows x64 安装包或便携版。系统需要 Windows 10/11 与 Microsoft Edge WebView2 Runtime；安装器可按需获取 Runtime，便携版需提前具备 Runtime。首版未做代码签名。

1. 打开 **加密与恢复 → 创建加密包**，选择文件或文件夹，指定源文件夹之外的新 `.fse` 路径，输入并确认密码。
2. 使用 **恢复加密包**，输入密码，指定尚不存在的新文件或新文件夹名称。程序自动识别原内容类型。
3. **先实际恢复并核对内容**。加密后源数据仍然保留，应用不会自动删除原件。密码遗失无法恢复。

**隐藏与只读** 页面提供递归属性管理与原始属性恢复，适合减少误操作。属性不是密码保护，也不能阻止管理员或其他有权限的软件访问。管理记录存储于 `%APPDATA%\FoldSentinel\state.json`；需要恢复前请保留这份记录。它记录原始属性，不代表当前属性一定未被其他软件改变。

## 功能与边界

- Argon2id 派生密钥，AES-256-GCM 认证加密；随机 salt/nonce，认证整个格式头部。
- 文件与文件夹均支持，目录包含相对结构、Unicode 文件名和空目录；错误密码、篡改或截断不会生成恢复输出。
- **文件或完整目录归档最多 64 MiB、10,000 项**。首版整包在内存中处理，不压缩。
- 拒绝覆盖任何已有输出；源文件和加密包均保留。属性操作提前持久化恢复记录，互斥锁防止多个进程覆盖状态。
- 拒绝目录联接、符号链接及其他重解析点。操作期间请勿改动源数据；不提供文件系统快照。
- 加密目录保留内容及结构，**不保留 NTFS ACL、备用数据流、硬链接、原始时间戳或其他特殊元数据**。不适合系统目录、程序目录和完整备份。
- 首版不含内核 minifilter、后台服务、透明挂载、原位加密、安全擦除或密码找回。不是 Anvi 的格式兼容实现。
- 自有容器格式尚未经过独立安全审计。临时恢复明文、进程中断和内存清理的具体边界见 [格式说明](docs/FORMAT.md)。

## 开发与构建

Windows 环境需要 Rust stable（MSVC，至少 1.89）、Visual Studio C++ Build Tools、Windows SDK、Node.js 22.12+ 和 WebView2。桌面依赖仅用于 UI 构建；核心 CLI 可单独构建。

```powershell
cargo run -- state-path
cargo run -- protect C:\Private\Notes --mode hidden-readonly
cargo run -- status C:\Private\Notes
cargo run -- unprotect C:\Private\Notes
cargo run -- vault encrypt C:\Private\Notes C:\Private\Notes.fse
cargo run -- vault decrypt C:\Private\Notes.fse C:\Private\Notes-restored
cargo test -p foldsentinel
cargo clippy --workspace --all-targets -- -D warnings
npm ci
npm run tauri dev
npm run tauri build
```

CLI 通过终端无回显输入密码，不支持把密码放进命令行参数。默认 `cargo run` 运行 CLI；桌面开发使用 `npm run tauri dev`。构建结果位于 `target/release/`，安装器位于 `target/release/bundle/nsis/`。

核心在 `src/`，桌面桥接在 `src-tauri/`，TypeScript 界面在 `ui/`。未来的大文件流式格式、挂载和驱动必须分别设计与审查，不应改变现有 v1 加密包参数。

## License

MIT，见 [LICENSE](LICENSE)。
