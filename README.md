# Bitwarden Rust

一个以 Rust 为安全与业务核心、以 WinUI 3 为 Windows 桌面界面的 Bitwarden 客户端重写工程。

当前版本是第一阶段骨架，目标是先把边界固定下来：

- crates/bitwarden-core：Rust 应用核心，保险库模型与原型本地加密仍处于实验阶段；密码和密码短语生成直接调用 Bitwarden 官方 Rust SDK。
- Bitwarden SDK 固定引用 `sdk-internal` 提交 `56c7617b23501af70221094cdee59e10b2a9d8d2`，复用官方生成算法与 EFF 长词表；SDK 与本仓库采用 GPL-3.0 许可。
- apps/Bitwarden.WinUI：WinUI 3 / Windows App SDK 桌面应用，使用 Fluent 导航、Mica 窗口和系统控件。
- scripts/build-rust-core.ps1：定位已安装的 MSVC 与 Windows SDK，并在 WinUI 构建时生成 Rust 核心 DLL。

## 当前状态

当前为早期预览版：WinUI 外壳支持官方风格的密码和密码短语生成选项，随机生成由 Bitwarden Rust SDK 提供。真实账户登录、官方加密格式、同步、自动填充、通行密钥、导入导出和完整条目 CRUD 尚未实现；本地快照加密是独立原型，不能与 Bitwarden 官方保险库互操作。请勿输入真实账户凭据或存放真实机密。

对照基线为 Bitwarden Desktop 2026.10.4，本机 MSIX 包版本为 2026.10.4.0。

## 构建

安装 Rust stable、.NET 10 SDK、MSVC x64/x86 工具和 Windows SDK 后，在 PowerShell 中运行：

~~~powershell
cd E:\\Bitwarden-Rust
cargo test -p bitwarden-core@0.2.0
dotnet build apps\\Bitwarden.WinUI\\Bitwarden.WinUI.csproj
dotnet run --project apps\\Bitwarden.WinUI\\Bitwarden.WinUI.csproj
~~~

构建独立身份的未签名 MSIX 开发包（仅用于本机打包验证，不会覆盖官方 Bitwarden 安装）：

~~~powershell
dotnet build apps\Bitwarden.WinUI\Bitwarden.WinUI.csproj -p:GenerateAppxPackageOnBuild=true -p:AppxPackageSigningEnabled=false
~~~

后续按里程碑接入认证与账户会话、官方加密格式与数据协议、加密本地缓存、同步冲突处理、条目 CRUD、剪贴板自动清理、自动锁定及导入导出。
