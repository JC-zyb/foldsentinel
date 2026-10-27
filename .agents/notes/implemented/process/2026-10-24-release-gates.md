# Agent Note: 发布版本与产物门禁

Status: implemented

## Problem

多处版本号可能漂移，打包成功也不证明便携 ZIP 包含两个入口及完整说明，手工校验和容易漏项。安装和真实磁盘满验证需要与普通构建证据分开记录。

## Decision

`verify-versions.mjs` 校验 Cargo、npm、两个锁文件、Tauri 配置及界面版本。CI 在构建前运行，打包脚本再次运行以覆盖本地发布。`verify-artifacts.ps1` 检查两个指定版本产物、校验和行数与散列、ZIP 必需文件、重复条目及不安全路径。产物按版本放入 `artifacts/<version>/`，保留旧版本包。

实际安装、升级、Runtime 缺失与卷写满的验证独立记录通过条件和缺失环境；编译、模拟写入器和干净 runner 构建不替代这些验证。未经实际执行的条目保持待验证，发布以同一提交的 Windows CI 为准。

`validate-volume.ps1` 仅在管理员上下文新建随机路径的 128 MiB VHD，通过镜像路径、容量、RAW 分区与非系统盘断言后初始化；不接收物理磁盘编号，最终显式确认卸载。真实 OS 写满验证要求一次性卷标记及容量上限。`validate-install.ps1` 仅允许 GitHub runner，在私有目录依次测试当前版本新装、v0.4 安装、升级及默认卸载；旧安装器固定 SHA-256。GUI 以隐藏方式启动，通过所属 WebView2 子进程及持续存活验证，避免把隐藏窗口的 MainWindowHandle 当作可见窗口；数据目录哨兵证明保留目录，二者不能推出交互、偏好语义或缺失 Runtime 已通过。

`validate-ui.mjs` 接受已有 Playwright 和浏览器路径，在隔离上下文中仅访问本机开发服务，检查中英布局、长路径、语言偏好和键盘焦点，不自动安装依赖、不模拟桌面操作成功。浏览器 deviceScaleFactor 仅是渲染检查，不能替代原生 Windows DPI 验收；真实旧版数据由独立桌面升级验证记录。

Vite 开发监听排除 `target` 构建目录、`work` 隔离测试目录和 `artifacts` 交付目录：Rust 构建替换二进制、测试用 WebView2 锁定资料及安装包生成曾导致 Windows watcher 以 `EBUSY` 退出。保留 UI 源文件热更新；不采用高 CPU 轮询或吞掉 watcher 异常。

`validate_upgrade` 使用从旧版本源码构建的 worker，在自有临时目录真实创建加密和恢复登记，检查活动锁后在提交前终止该子进程。当前核心读取旧记录、验证所有者、清理并重试，核对源与恢复内容。报告记录 worker SHA-256，验证记录另列源 tag 和 commit；此项覆盖实际旧记录语义，但不代表安装升级或真实用户文件已验收。

历史审计：[日常流程](../architecture/2026-10-24-daily-workflows.md) 部分重叠，仅描述产品行为及历史兼容；[流式任务](../architecture/2026-10-24-streaming-v2.md) 部分重叠，仅约束输出与恢复边界。两篇均保留。本篇归属构建与验证门禁，不改变格式；首版和属性恢复笔记无关。

## Alternatives considered

- 仅靠发布前手工核对最省实现，但无法阻止不同清单与锁文件发生漂移；增加无依赖脚本作为本地和 CI 共用入口。
- 每次构建删除整个 artifacts 目录可避免混包，但会丢失先前版本及验证材料；改为版本目录，并检查本次两个明确文件。

## Consequences

脚本不新增 Node 或 PowerShell 模块依赖，门禁失败会中止打包。ZIP 白名单需要在交付结构变化时同步维护，散列只能验证一致性，不能替代签名或安全审计。系统级测试仍需要适当权限与隔离环境，不能因普通自动化通过就标记完成。
