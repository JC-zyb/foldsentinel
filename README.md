# FoldSentinel

文件夹哨兵：使用 Rust + Tauri 2 构建的开源 Windows 文件保护工具。支持中英双语、文件 / 文件夹流式密码加密包、串行任务队列、只读验证、历史与重试，以及可恢复的隐藏、只读属性管理。

## 使用 v0.3

在 [Releases](https://github.com/JC-zyb/foldsentinel/releases) 下载 Windows x64 安装包或便携版。系统需要 Windows 10/11 与 Microsoft Edge WebView2 Runtime；安装器可按需获取 Runtime，便携版需提前具备 Runtime。当前版本未做代码签名。

1. 右上角选择 **简体中文 / English**；首次使用遵循系统首选语言，选择会在重启后保留。
2. 在 **创建加密包 / Encrypt** 中选择多个文件、文件夹或直接拖入窗口；分次选择或拖入会追加并去重，也可每行填写一个完整路径或直接编辑列表。可选输出目录，留空使用各源文件的父目录；加密文件夹的输出必须位于源目录之外。
3. 输入并确认这一批任务使用的密码，点击 **加入队列 / Add to queue**，检查每项输出建议，再点击 **开始 / 继续队列 / Start / resume**。自动避开已有名称，最终仍禁止覆盖。
4. **恢复加密包 / Restore** 与 **验证加密包 / Verify** 也支持批量加入。验证会完整认证 v1/v2 内容并检查归档结构，不创建恢复文件或临时明文；仍建议实际恢复并核对重要文件。
5. 长任务显示阶段、字节、速度及预计剩余时间，可取消当前任务、取消等待任务，或在当前任务后暂停队列。提交开始后不可取消；关闭窗口先暂停队列并请求取消，清理后可再次关闭。
6. 队列与历史记录保存在本机 WebView2 用户数据中，最多保留 200 项；可清空历史，不会删除源文件。密码和密钥不写入历史；等待任务只在进程内存中暂存密码，结束或取消后释放引用，退出后重试须重新输入。
7. 异常退出后先在 **中断任务临时数据 / Interrupted task data** 清理临时数据，再从历史或清理结果重填并输入密码重试。不支持断点续传；请同时检查最终输出是否已生成。

加密后源数据仍然保留，应用不会自动删除原件。密码遗失无法恢复。界面与应用生成的文件选择标题遵循语言选择，Windows 文件对话框的系统按钮和系统错误由操作系统语言决定。

**隐藏与只读** 页面提供递归属性管理与原始属性恢复，适合减少误操作。属性不是密码保护，也不能阻止管理员或其他有权限的软件访问。管理记录存储于 `%APPDATA%\FoldSentinel\state.json`；需要恢复前请保留这份记录。它记录原始属性，不代表当前属性一定未被其他软件改变。

## 功能与边界

- Argon2id 派生密钥，RustCrypto STREAM BE32 + AES-256-GCM 分块认证加密；随机 salt/nonce 前缀，每块认证整个格式头部及顺序、终块状态。
- 文件与文件夹均支持，目录包含相对结构、Unicode 文件名和空目录；错误密码、篡改或截断不会生成恢复输出。
- **v2 文件内容总量最多 1 TiB、目录最多 100,000 项**，固定 1 MiB 分块，不压缩；单路径最多 4096 UTF-8 字节、相对路径总量最多 16 MiB。内容内存不随文件大小增长，目录清单内存有上限。这是格式/实现限制，实测规模见验证记录。
- 新建加密包使用 v2，兼容恢复 v0.1 的 v1 加密包；v0.1 无法读取 v2。旧包仍受 v1 的 64 MiB / 10,000 项限制。
- 拒绝覆盖任何已有输出；源文件和加密包均保留。属性操作提前持久化恢复记录，互斥锁防止多个进程覆盖状态。
- 拒绝目录联接、符号链接及其他重解析点。操作期间请勿改动源数据；不提供文件系统快照。
- 加密目录保留内容及结构，**不保留 NTFS ACL、备用数据流、硬链接、原始时间戳或其他特殊元数据**。不适合系统目录、程序目录和完整备份。
- 不含内核 minifilter、后台服务、透明挂载、原位加密、安全擦除或密码找回。不是 Anvi 的格式兼容实现。
- 自有容器格式尚未经过独立安全审计。临时恢复明文、进程中断和内存清理的具体边界见 [格式说明](docs/FORMAT.md)。

## 开发与构建

Windows 环境需要当前 Rust stable（MSVC）、Visual Studio C++ Build Tools、Windows SDK、Node.js 22.12+ 和 WebView2。依赖版本锁定于 Cargo.lock 和 package-lock.json；桌面依赖仅用于 UI 构建，核心 CLI 可单独构建。

```powershell
cargo run -- state-path
cargo run -- protect C:\Private\Notes --mode hidden-readonly
cargo run -- status C:\Private\Notes
cargo run -- unprotect C:\Private\Notes
cargo run -- vault encrypt C:\Private\Notes C:\Private\Notes.fse
cargo run -- vault decrypt C:\Private\Notes.fse C:\Private\Notes-restored
cargo run -- vault verify C:\Private\Notes.fse
cargo run -- vault interrupted
cargo run -- vault cleanup <中断任务ID>
cargo test -p foldsentinel
cargo clippy --workspace --all-targets -- -D warnings
npm ci
npm test
npm run tauri dev
npm run tauri build
```

CLI 通过终端无回显输入密码，不支持把密码放进命令行参数；进度输出到标准错误，Windows 终端 Ctrl+C / Ctrl+Break 请求安全取消。默认 `cargo run` 运行 CLI；桌面开发使用 `npm run tauri dev`。桌面程序为 `FoldSentinel.exe`，命令行为 `foldsentinel-cli.exe`，位于 `target/release/`，安装器位于 `target/release/bundle/nsis/`。

核心在 `src/`，桌面桥接在 `src-tauri/`，TypeScript 界面在 `ui/`。格式与兼容性约定见 [格式说明](docs/FORMAT.md)。

Windows CI 会保留安装包和便携包构建产物。手动执行 `./scripts/package.ps1` 可完成构建、打包和 SHA-256 生成，结果位于 `artifacts/`。已验证内容、重现大数据验证的命令及未覆盖场景见 [验证记录](docs/VALIDATION.md)。

安装、升级、便携版数据位置与校验方式见 [安装与发布](docs/INSTALL.md)。磁盘空间预检采用保守估计，不能保证其他程序随后不占用空间；运行中仍会明确报告写入失败。JavaScript 字符串无法保证原地清零；Rust 密码与明文缓冲使用 Zeroizing，前端仅保留当前队列所需的内存引用。

## License

MIT，见 [LICENSE](LICENSE)。
