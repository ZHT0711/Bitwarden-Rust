# Bitwarden Rust

一个以 Rust 为安全与业务核心、以 WinUI 3 为 Windows 桌面界面的 Bitwarden 客户端重写工程。

当前版本是第一阶段骨架，目标是先把边界固定下来：

- crates/bitwarden-core：Rust 核心库，提供保险库模型、原型本地加密和密码生成；通过 C ABI 提供 WinUI 调用入口。
- apps/Bitwarden.WinUI：WinUI 3 / Windows App SDK 桌面应用，使用 Fluent 导航、Mica 窗口和系统控件。
- scripts/build-rust-core.ps1：定位已安装的 MSVC 与 Windows SDK，并在 WinUI 构建时生成 Rust 核心 DLL。

## 当前状态

当前为第一阶段预览版：WinUI 外壳和密码生成器已接入 Rust 核心。真实账户登录、官方加密格式、同步、自动填充、通行密钥、导入导出和完整条目 CRUD 尚未实现；本地快照加密是独立原型，不能与 Bitwarden 官方保险库互操作。请勿输入真实账户凭据或存放真实机密。

对照基线为 Bitwarden Desktop 2026.10.4，本机 MSIX 包版本为 2026.10.4.0。

## 构建

安装 Rust stable、.NET 10 SDK、MSVC x64/x86 工具和 Windows SDK 后，在 PowerShell 中运行：

~~~powershell
cd E:\\Bitwarden-Rust
cargo test -p bitwarden-core
dotnet build apps\\Bitwarden.WinUI\\Bitwarden.WinUI.csproj
dotnet run --project apps\\Bitwarden.WinUI\\Bitwarden.WinUI.csproj
~~~

后续按里程碑接入认证与账户会话、官方密码学与数据协议、加密本地缓存、同步冲突处理、条目 CRUD、剪贴板自动清理、自动锁定、导入导出和 MSIX 打包。
