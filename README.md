# Bitwarden Rust

一个以 Rust 为安全与业务核心、以 WinUI 3 为 Windows 桌面界面的 Bitwarden 客户端重写工程。

当前版本以 Rust 作为安全与业务边界，逐步接入 Bitwarden 官方 SDK：

- crates/bitwarden-core：Rust 应用核心，保险库模型与原型本地加密仍处于实验阶段；密码和密码短语生成直接调用 Bitwarden 官方 Rust SDK。
- Bitwarden SDK 固定引用 `sdk-internal` 提交 `56c7617b23501af70221094cdee59e10b2a9d8d2`，复用官方生成算法与 EFF 长词表；SDK 与本仓库采用 GPL-3.0 许可。
- apps/Bitwarden.WinUI：WinUI 3 / Windows App SDK 桌面应用，使用 Fluent 导航、Mica 窗口和系统控件。
- scripts/build-rust-core.ps1：定位已安装的 MSVC 与 Windows SDK，并在 WinUI 构建时生成 Rust 核心 DLL。

## 当前状态

当前为早期预览版：WinUI 支持密码和密码短语生成，并通过固定版本的 Bitwarden Rust SDK 接入主密码登录、身份验证器验证码、邮箱验证码及 YubiKey 验证；登录支持 Bitwarden.com、Bitwarden.eu 和自托管服务器 URL，邮箱二步验证码请求沿用所选服务器。自托管 URL 使用服务器根地址并映射到 /api 与 /identity；生产服务器需使用 HTTPS，HTTP 仅允许回环地址。尚未用真实账户验证登录流程。Duo 和 WebAuthn 交互、保险库同步、官方加密数据的持久化、自动填充、通行密钥、导入导出和完整条目 CRUD 尚未实现。认证会话仅保存在进程内存，退出或重启后不会恢复。本地快照加密是独立原型，不能与 Bitwarden 官方保险库互操作；请勿在此预览版中保存真实保险库数据。

对照基线为 Bitwarden Desktop 2026.11.0（2026-11-19 发布）；本机实际安装的 MSIX 仍为 2026.10.4.0。

## 构建

先检查现有工具，只为缺少的依赖安装组件；需要 Rust stable、.NET 10 SDK、MSVC x64/x86 工具和 Windows SDK。在 PowerShell 中检查 Rust 与 .NET：

~~~powershell
rustc --version
cargo --version
dotnet --list-sdks
~~~

依赖齐全后，在 PowerShell 中运行：

~~~powershell
cd E:\Bitwarden-Rust
cargo test -p bitwarden-core@0.2.0
dotnet build apps\Bitwarden.WinUI\Bitwarden.WinUI.csproj
dotnet run --project apps\Bitwarden.WinUI\Bitwarden.WinUI.csproj
~~~

构建独立身份的未签名 MSIX 开发包（仅用于本机打包验证，不会覆盖官方 Bitwarden 安装）：

~~~powershell
dotnet build apps\Bitwarden.WinUI\Bitwarden.WinUI.csproj -p:GenerateAppxPackageOnBuild=true -p:AppxPackageSigningEnabled=false
~~~

后续按里程碑接入官方保险库同步与解密、加密本地缓存、同步冲突处理、条目 CRUD、剪贴板自动清理、自动锁定及导入导出。
