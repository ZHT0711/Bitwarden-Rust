# Bitwarden Rust

以 Rust 为安全与业务核心、以 WinUI 3 为 Windows 桌面界面的 Bitwarden 客户端重写工程。目标是逐项复刻官方最新 MSIX 桌面端，完成桌面端后再优化 Firefox 扩展联动。

- `crates/bitwarden-core`：安全与业务核心，通过 C ABI 供 WinUI 调用。
- `apps/Bitwarden.WinUI`：WinUI 3 / Windows App SDK、Fluent 导航和 Mica 窗口。
- `scripts/build-rust-core.ps1`：定位已安装的 MSVC 和 Windows SDK，生成 x64 Rust DLL。
- 官方 SDK 固定为 `sdk-internal` 提交 `56c7617b23501af70221094cdee59e10b2a9d8d2`，本仓库采用 GPL-3.0 许可。

## 当前状态

当前是早期开发预览，已接入以下代码路径：

- 密码和密码短语直接由官方 Rust SDK 生成。生成参数按账户与服务器保存，未登录时使用独立设置。
- 主密码登录、身份验证器 / 邮箱 / YubiKey 二步验证；支持美国、欧盟和自托管服务器，记住所选服务器，支持请求取消和超时。自托管 URL 使用根地址并映射到 `/api` 与 `/identity`，HTTP 仅允许本机回环地址。身份验证器验证码无效、过期或已使用时，请确认设备时间正确，等待下一组 6 位验证码再试。
- 登录二步验证页可勾选「记住我（30 天）」（默认不勾选）。成功验证后仅保存服务器签发的免二步验证令牌，按账户、服务器与设备绑定，并用当前 Windows 用户 DPAPI 加密；退出账户或重启后仍可使用，最长 30 天且服务器可提前撤销。到期、撤销或损坏时恢复正常二步验证；仍需主密码，不影响保险库锁定。
- 右上角的账户菜单可添加及切换最多 5 个账户。自托管和官方 US／EU 支持并存，每个账户单独保存会话、保险库缓存、锁定状态及超时设置。同一进程中切换到已同步的账户可以立即读取本机缓存。
- 通过 SDK API 同步保险库、初始化组织密钥并解密个人 / 组织条目。保险库服务端密文在进程内存中缓存，并在 Windows 上写入当前用户 DPAPI 保护的本机快照；跨重启恢复时账户保持锁定，主密码或已启用的 Windows Hello 解锁后只读，重新连接并成功同步后才可写。界面列表接收显示元数据，选中条目才解密详情。
- 分类、收藏、回收站、归档、文件夹筛选、名称 / 副标题搜索与详情读取。读取覆盖 SDK 中的登录、安全笔记、银行卡、身份信息、SSH 密钥、银行账户、驾驶执照和护照八种类型。
- “我的文件夹”支持按名称搜索、打开文件夹查看条目、新建、重命名、单个或批量删除。删除会保留条目并移至“无文件夹”。缺少批量接口的自托管服务器可逐个删除，若中途失败则先同步核对。
- Rust 端执行主密码复验和隐藏密码权限；详情 DTO 显式排除保险库密钥和 FIDO2 私密材料，损坏条目单独计数。
- 密码栏右侧可点击显示或隐藏内容。登录条目的图标使用 Bitwarden 或自托管服务器的图标端点，请求只包含网站主机名。可在设置里关闭网站图标。
- 登录条目可保存 Base32 密钥、`otpauth://totp` 链接和 Steam 验证器密钥。满足个人或组织会员权限时，详情显示本机生成的验证码与倒计时，并支持复制。条目复验、锁定和隐藏字段权限同样适用于验证码。
- 条目详情支持上传、下载和经确认后删除附件。Rust 通过 SDK 加密文件及名称后上传，下载则在完整解密后保存。上传失败或取消时会尝试移除本次创建的附件，无法确认清理时须先同步核对。
- 登录条目显示已保存通行密钥的网站、用户名和创建时间。编辑默认保留通行密钥，勾选移除并保存后才删除。WinUI 详情只接收显示信息，私钥由 Rust 核心处理。系统发现缓存接收凭据 ID、用户句柄和网站等公共元数据。
- 实验性 Windows 通行密钥提供程序使用本项目独立的 COM 标识与 AAGUID。设置页可注册提供程序并打开系统启用页面，状态以系统 API 返回值为准。网站请求经 Windows 签名校验后进入 WinUI 账户选择，并通过绑定本次请求的 Windows Hello 验证。Rust 调用官方 FIDO 认证器，创建结果在服务器确认保存私钥后才返回网站。不会把登录当前保险库的通行密钥存回它自己；遇到此限制时，请选择 Windows Hello、安全密钥或其他独立密码库。系统交互与真实网站的完整流程仍待端到端验收。
- 新增和编辑条目、批量移动到文件夹、收藏、归档、软删除、还原和永久删除。遇到版本冲突或无法确认写入结果时，先同步再继续操作。
- 手动锁定和自动超时会清除 SDK 密钥和保险库界面内容。超时可按账户设置为应用闲置、系统空闲、锁屏、休眠或应用重启，并选择锁定或退出登录。应用仍在运行时，可用主密码从内存中的密文恢复已同步保险库。退出应用时会释放进程内会话密钥；应用再次启动后仍保持锁定。
- 「设置 → 安全 → 启用 Windows Hello 解锁」按账户启用：首次需同步、确认主密码并通过 Windows Hello。之后在解锁对话框选择「Windows Hello」，不保存主密码，也不替代条目的主密码复验或重新连接服务器所需的二步验证。显式退出登录或检测到密钥包装变化时会撤销配置；若拒绝或无法使用 Windows Hello，仍可输入主密码。
- 「设置 → 启动与托盘」可启用 Windows 登录后启动、自启时隐藏，以及关闭窗口后留在托盘。托盘图标可恢复窗口，也可右键选择彻底退出；留在托盘时保险库继续遵守当前账户的锁定与超时设置。
- 「Send」支持端到端加密的文本与文件分享，可配置到期和删除时间、访问次数、停用、密码保护及服务器支持时的接收邮箱验证。文件在 Rust 中加密后上传，桌面端上限为 500 MiB，服务器的配额和策略可能更严格。链接只在明确复制时返回界面；上传失败或取消时会尝试清理本次创建的 Send，不能确认结果时须先刷新核对。
- 「设置 → 保险库数据」可导入和导出个人保险库的 Bitwarden JSON：明文、账户限定加密、独立密码加密。导出需在线检查策略和复验主密码，明文需额外确认，文件不会覆盖现有文件。导入限制为 32 MiB、共 7,000 个条目与文件夹，只新增，不覆盖；遇到未确认结果必须先同步。JSON 不包含附件文件、回收站或组织保险库。
- 复制内容禁止进入系统剪贴板历史 / 云同步，30 秒后仅清理本应用仍然拥有的内容。退出时清理界面与内存会话。

自动测试使用官方公开合成账户和模拟服务器。真实账户及网站通行密钥的端到端验证、真实账户跨重启验证、Duo / WebAuthn 登录验证、SSO、V2 登录与复验、云端 Send 和附件、组织策略、其他导入导出格式及 Firefox 联动仍待完成。完整差距见 [功能对照](docs/PARITY.md)。

`crates/bitwarden-core/src/storage.rs` 中的本机快照加密器是未用于账户保险库的独立原型，不能与官方保险库互操作。Windows 账户缓存另行保存官方 SDK 同步得到的服务器密文，并以当前用户 DPAPI 加密；该开发预览仍不得作为真实密码库的唯一客户端。

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

使用固定的预览包身份 `ZHT0711.BitwardenRustLocalPreview`，不覆盖官方应用。请勿为常规预览使用 `winapp run --unique-identity`，否则同名条目会出现在开始菜单。当前仅验证 x64；原生构建脚本尚不支持 x86 / ARM64。运行与打包必须使用相同的 `Platform=x64`，否则可能启动之前注册的旧 `AppX` 布局。可向预览传入 `--start-in-tray` 检查隐藏启动，不必注销当前 Windows 会话。

GitHub Actions 执行 Rust 格式 / 测试 / Clippy、C# 筛选及实际 DLL 互操作检查，并构建未签名 x64 MSIX 开发包。未签名包不能直接作为正式签名安装包分发。

可重复的生成器与大库写入投影测量见 [性能测量](docs/PERFORMANCE.md)。
