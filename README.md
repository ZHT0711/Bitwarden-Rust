# Bitwarden Rust

以 Rust 为安全与业务核心、以 WinUI 3 为 Windows 桌面界面的 Bitwarden 客户端重写工程。目标是逐项复刻官方最新 MSIX 桌面端，完成桌面端后再优化 Firefox 扩展联动。

- `crates/bitwarden-core`：安全与业务核心，通过 C ABI 供 WinUI 调用。
- `apps/Bitwarden.WinUI`：WinUI 3 / Windows App SDK、Fluent 导航和 Mica 窗口。
- `scripts/build-rust-core.ps1`：定位已安装的 MSVC 和 Windows SDK，生成 x64 Rust DLL。
- 官方 SDK 固定为 `sdk-internal` 提交 `56c7617b23501af70221094cdee59e10b2a9d8d2`，本仓库采用 GPL-3.0 许可。

## 当前状态

当前是早期开发预览，已接入以下代码路径：

- 密码 / 密码短语生成，直接调用官方 Rust SDK。
- 主密码登录、身份验证器 / 邮箱 / YubiKey 二步验证；支持美国、欧盟和自托管服务器，记住所选服务器，支持请求取消和超时。自托管 URL 使用根地址并映射到 `/api` 与 `/identity`，HTTP 仅允许本机回环地址。
- 通过 SDK API 同步保险库、初始化组织密钥并解密个人 / 组织条目。密文只缓存在进程内存；界面列表接收显示元数据，选中条目才解密详情。
- 分类、收藏、回收站、归档、文件夹筛选、名称 / 副标题搜索与详情读取。读取覆盖 SDK 中的登录、安全笔记、银行卡、身份信息、SSH 密钥、银行账户、驾驶执照和护照八种类型。
- Rust 端执行主密码复验和隐藏密码权限；详情 DTO 显式排除保险库密钥和 FIDO2 私密材料，损坏条目单独计数。
- 密码栏右侧可点击显示或隐藏内容。登录条目的图标使用 Bitwarden 或自托管服务器的图标端点，请求只包含网站主机名。可在设置里关闭网站图标。
- 登录条目可保存 Base32 密钥、`otpauth://totp` 链接和 Steam 验证器密钥。满足个人或组织会员权限时，详情显示本机生成的验证码与倒计时，并支持复制。条目复验、锁定和隐藏字段权限同样适用于验证码。
- 登录条目显示已保存通行密钥的网站、用户名和创建时间。编辑默认保留通行密钥，勾选移除并保存后才删除。私钥、凭据 ID 和用户句柄保留在 Rust 核心内。Windows 提供程序的创建与网站登录仍在接入中。
- 新增和编辑条目、收藏、归档、软删除、还原和永久删除。遇到版本冲突或无法确认写入结果时，先同步再继续操作。
- 手动锁定清除 SDK 密钥和保险库界面内容。应用仍在运行时，可使用主密码从内存中的密文恢复已同步保险库。退出应用后需要重新登录。
- 复制内容禁止进入系统剪贴板历史 / 云同步，30 秒后仅清理本应用仍然拥有的内容。退出时清理界面与内存会话。

自动测试使用官方公开合成账户和模拟服务器。真实账户端到端验证、磁盘加密缓存、重启会话恢复、自动锁定、Duo / WebAuthn、SSO、V2 登录与复验、Send、附件、通行密钥操作、导入导出及 Firefox 联动仍待完成。完整差距见 [功能对照](docs/PARITY.md)。

本地快照加密模块是独立原型，不能与官方保险库互操作，没有用于账户保险库。请勿把此开发预览作为真实密码库的唯一客户端。

对照基线为 Bitwarden Desktop 2026.11.0，GitHub 发布时间为 UTC 2026-11-19 20:49，即北京时间 2026-11-20 04:49；本机官方 MSIX 为 2026.10.4.0。仍需以新版实际安装包逐项验收，不能把 SDK 支持范围当作整个桌面端已完成。

## 构建与验证

需要 Rust stable、.NET 10 SDK、MSVC x64 工具、Windows SDK；界面验证使用 `winapp` CLI。先检查现有工具，只安装缺少的组件：

```powershell
rustc --version
cargo --version
dotnet --list-sdks
winapp --version
```

从仓库根目录运行，点加载构建脚本会为同一 PowerShell 会话配置 MSVC 链接库路径：

```powershell
. ./scripts/build-rust-core.ps1
cargo test --locked -p bitwarden-core@0.2.0
cargo clippy --locked -p bitwarden-core@0.2.0 --all-targets -- -D warnings
dotnet run --project tests/Bitwarden.Interop.Tests -- target/debug/bitwarden_core.dll
dotnet build apps/Bitwarden.WinUI/Bitwarden.WinUI.csproj -p:Platform=x64 -p:GenerateAppxPackageOnBuild=true -p:AppxPackageSigningEnabled=false
winapp run apps/Bitwarden.WinUI/Bitwarden.WinUI.csproj --no-build -p Platform=x64
```

使用独立包身份，不覆盖官方应用。当前仅验证 x64；原生构建脚本尚不支持 x86 / ARM64。运行与打包必须使用相同的 `Platform=x64`，否则可能启动之前注册的旧 `AppX` 布局。

GitHub Actions 执行 Rust 格式 / 测试 / Clippy、C# 筛选及实际 DLL 互操作检查，并构建未签名 x64 MSIX 开发包。未签名包不能直接作为正式签名安装包分发。
