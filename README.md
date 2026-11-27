<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/bytewarden-banner-dark.png" />
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/bytewarden-banner-light.png" />
  <img alt="Bytewarden Logo" src="docs/assets/bytewarden-banner-light.png" width="460" />
</picture>

**简体中文（默认）** · [![Switch to English](https://img.shields.io/badge/README-English-005AA4?style=flat-square)](README.en.md)

以 Rust 为安全与业务核心的 Bitwarden 兼容客户端重写工程[[免责声明](#免责声明)]。现有功能由 WinUI 3 预览版提供；[GPUI 原生桌面预览](apps/Bytewarden.Gpui/README.md) 不使用 WebView，已接入 Rust 核心的登录、同步和浏览流程；[功能迁移与验收](docs/GPUI-MIGRATION.md) 仍在进行，尚未替代 WinUI 版。目标是逐项对照官方桌面端，之后再优化 Firefox 扩展联动。

---

[![Windows Validation](https://img.shields.io/github/actions/workflow/status/ZHT0711/Bytewarden/windows.yml?style=for-the-badge&logo=github&logoColor=fff&label=Windows%20CI)](https://github.com/ZHT0711/Bytewarden/actions/workflows/windows.yml)
[![Rust](https://img.shields.io/badge/Rust-Core-000000?style=for-the-badge&logo=rust&logoColor=fff)](#架构概览)
[![WinUI 3](https://img.shields.io/badge/WinUI%203-.NET%2010%20x64-005AA4?style=for-the-badge&logo=windows&logoColor=fff)](#构建与验证)
[![GPL-3.0 Licensed](https://img.shields.io/github/license/ZHT0711/Bytewarden.svg?style=for-the-badge&color=944000)](LICENSE) <br>
[![Contributors](https://img.shields.io/github/contributors/ZHT0711/Bytewarden.svg?style=flat-square&logo=github&logoColor=fff&color=005AA4)](https://github.com/ZHT0711/Bytewarden/graphs/contributors)
[![Forks](https://img.shields.io/github/forks/ZHT0711/Bytewarden.svg?style=flat-square&logo=github&logoColor=fff&color=005AA4)](https://github.com/ZHT0711/Bytewarden/network/members)
[![Stars](https://img.shields.io/github/stars/ZHT0711/Bytewarden.svg?style=flat-square&logo=github&logoColor=fff&color=005AA4)](https://github.com/ZHT0711/Bytewarden/stargazers)
[![Issues Open](https://img.shields.io/github/issues/ZHT0711/Bytewarden.svg?style=flat-square&logo=github&logoColor=fff&color=005AA4)](https://github.com/ZHT0711/Bytewarden/issues)
[![Issues Closed](https://img.shields.io/github/issues-closed/ZHT0711/Bytewarden.svg?style=flat-square&logo=github&logoColor=fff&color=005AA4)](https://github.com/ZHT0711/Bytewarden/issues?q=is%3Aissue+is%3Aclosed)

> [!IMPORTANT]
> **使用本项目时，请将任何缺陷或功能建议直接反馈至 [本仓库 Issue](https://github.com/ZHT0711/Bytewarden/issues)，请勿通过官方 Bitwarden 支持渠道提交。** <br>
> **本项目仍处于早期开发预览阶段，请勿将其作为真实密码库的唯一客户端。**

<br>

## 架构概览

- `crates/bytewarden-core`：安全与业务核心，通过 C ABI 供 WinUI 调用。
- `apps/Bytewarden.WinUI`：WinUI 3 / Windows App SDK、Fluent 导航和 Mica 窗口。
- `scripts/build-rust-core.ps1`：定位已安装的 MSVC 和 Windows SDK，生成 x64 Rust DLL。
- 官方 SDK 固定为 `sdk-internal` 提交 `56c7617b23501af70221094cdee59e10b2a9d8d2`，本仓库采用 GPL-3.0 许可（参见 [`LICENSE`](LICENSE) 与 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)）。

<br>

## 功能特性

当前是早期开发预览，已接入以下代码路径：

### 账户与认证

- **多区域与自托管服务器**：支持主密码登录及身份验证器 / 邮箱 / YubiKey 二步验证；支持美国、欧盟和自托管服务器，记住所选服务器并支持请求取消和超时。自托管 URL 使用根地址并映射到 `/api` 与 `/identity`，HTTP 仅允许本机回环地址。身份验证器验证码无效、过期或已使用时，请确认设备时间正确，等待下一组 6 位验证码再试。
- **记住二步验证（30 天）**：登录二步验证页可勾选「记住我（30 天）」（默认不勾选）。成功验证后仅保存服务器签发的免二步验证令牌，按账户、服务器与设备绑定，并用当前 Windows 用户 DPAPI 加密；退出账户或重启后仍可使用，最长 30 天且服务器可提前撤销。到期、撤销或损坏时恢复正常二步验证；仍需主密码，不影响保险库锁定。
- **多账户切换**：右上角的账户菜单可添加及切换最多 5 个账户。自托管和官方 US／EU 支持并存，每个账户单独保存会话、保险库缓存、锁定状态及超时设置。同一进程中切换到已同步的账户可以立即读取本机缓存。
- **保险库同步与离线快照**：通过 SDK API 同步保险库、初始化组织密钥并解密个人 / 组织条目。保险库服务端密文在进程内存中缓存，并在 Windows 上写入当前用户 DPAPI 保护的本机快照；跨重启恢复时账户保持锁定。选择「解锁并同步」只需输一次主密码；若服务器要求二步验证，在同一对话框继续。也可选择「仅离线解锁」或已启用的 Windows Hello 读取本机缓存，不连接服务器；同步成功前，缓存条目不可编辑。界面列表接收显示元数据，选中条目才解密详情。

### 保险库浏览与编辑

- **条目类型与列表筛选**：覆盖 SDK 中的登录、安全笔记、银行卡、身份信息、SSH 密钥、银行账户、驾驶执照和护照八种类型，支持分类、收藏、回收站、归档、文件夹筛选与详情读取。名称 / 副标题支持多词同时匹配和双引号短语搜索；可按名称升降序或修改时间新旧排序，排序偏好跨重启保留。搜索只使用显示元数据，不读取密码或隐藏字段；官方完整高级搜索语法仍未实现。
- **条目编辑与生命周期**：支持新增和编辑条目、批量移动到文件夹、收藏、归档、软删除、还原和永久删除。遇到版本冲突或无法确认写入结果时，先同步再继续操作。
- **自适应布局、快捷键与生成器**：窄窗口自动收起导航，保险库采用单栏列表 / 详情并可返回列表；短窗口顶部筛选与文件夹导航可独立滚动，为条目和空状态保留空间。支持 `Ctrl+F` 搜索、`Ctrl+N` 新建、`Ctrl+E` 编辑、`Ctrl+R` 同步、`Ctrl+L` 锁定，沿用当前账户权限与主密码复验；设置可选择跟随 Windows、浅色或深色主题。密码和密码短语直接由官方 Rust SDK 生成，生成参数按账户与服务器保存，未登录时使用独立设置。

### 文件夹与关联字段

- **层级文件夹管理**：「我的文件夹」按 `/` 路径展示真实层级，支持完整名称搜索、近期面包屑 / 早期真实祖先菜单与子文件夹入口、包含子文件夹的条目计数，以及新建、重命名、单个或批量删除。超深路径只渲染最后 8 个真实节点，省略的实际祖先仍可按原 ID 导航；长名称仅缩略可视标签，原名、ID 和精确条目归属不变。缺失的父路径不会生成虚拟文件夹。重命名或删除父文件夹不会级联改动未选中的子文件夹；删除只将所删文件夹的条目移至「无文件夹」，不删除条目。离线只读缓存仍可浏览。缺少批量接口的自托管服务器可逐个删除，若中途失败则先同步核对。
- **27 种关联自定义字段**：登录、银行卡和身份信息详情支持 27 种关联自定义字段。关联值随源字段编辑更新，敏感目标沿用默认遮挡和组织权限，受主密码复验保护的条目在验证后才显示。搜索仍只使用列表显示元数据。

### 安全边界、锁定与系统集成

- **访问控制与网站图标**：Rust 端执行主密码复验和隐藏密码权限；详情 DTO 显式排除保险库密钥和 FIDO2 私密材料，损坏条目单独计数。密码栏右侧可点击显示或隐藏内容；登录条目的图标使用 Bitwarden 或自托管服务器的图标端点，请求只包含网站主机名，可在设置里关闭网站图标。
- **手动锁定与自动超时**：手动锁定和自动超时会清除 SDK 密钥和保险库界面内容。超时可按账户设置为应用闲置、系统空闲、锁屏、休眠或应用重启，并选择锁定或退出登录。应用仍在运行时，可用主密码从内存中的密文恢复已同步保险库。退出应用时会释放进程内会话密钥；应用再次启动后仍保持锁定。
- **Windows Hello 本机解锁**：「设置 → 安全 → 启用 Windows Hello 解锁」按账户启用：首次需同步、确认主密码并通过 Windows Hello。之后在解锁对话框选择「Windows Hello」。在支持的 Windows 版本上，系统验证提示直接绑定 Bytewarden 窗口。Hello 不保存主密码；重启后仅用于本机解锁，不替代服务器登录、二步验证或条目的主密码复验。显式退出登录或检测到密钥包装变化时会撤销配置；若拒绝或无法使用 Windows Hello，仍可输入主密码。
- **开机自启与系统托盘**：「设置 → 启动与托盘」可启用 Windows 登录后启动、自启时隐藏，以及关闭窗口后留在托盘。托盘图标可恢复窗口，也可右键选择彻底退出；留在托盘时保险库继续遵守当前账户的锁定与超时设置。
- **剪贴板安全清理**：复制内容禁止进入系统剪贴板历史 / 云同步；默认 30 秒后仅清理本应用仍然拥有的内容。可在「设置 → 安全」修改清除时间（10 / 20 / 30 / 60 / 120 / 300 秒或不定时清除）及复制后最小化。锁定、切换账户仍清理本应用的内容；占用时持续退避重试，原子检查归属，不覆盖后续写入者或新复制。正常 / 托盘退出先清理界面与内存会话，最多再等待 5 秒清理剪贴板；如果其他程序仍占用剪贴板，或进程被强制结束，内容可能残留，需要手动清除。隐藏到托盘不是退出。

### TOTP、附件与通行密钥

- **离线 TOTP 验证码**：登录条目可保存 Base32 密钥、`otpauth://totp` 链接和 Steam 验证器密钥。满足个人或组织会员权限时，详情显示本机生成的验证码与倒计时，并支持复制。条目复验、锁定和隐藏字段权限同样适用于验证码。
- **加密附件**：条目详情支持上传、下载和经确认后删除附件。Rust 通过 SDK 加密文件及名称后上传，下载则在完整解密后保存。上传失败或取消时会尝试移除本次创建的附件，无法确认清理时须先同步核对。
- **通行密钥与实验性 Windows 提供程序**：登录条目显示已保存通行密钥的网站、用户名和创建时间；编辑默认保留通行密钥，勾选移除并保存后才删除。WinUI 详情只接收显示信息，私钥由 Rust 核心处理；系统发现缓存接收凭据 ID、用户句柄和网站等公共元数据。实验性 Windows 通行密钥提供程序使用本项目独立的 COM 标识与 AAGUID，设置页可注册提供程序并打开系统启用页面，状态以系统 API 返回值为准；网站请求经 Windows 签名校验后进入 WinUI 账户选择，并通过绑定本次请求的 Windows Hello 验证。Rust 调用官方 FIDO 认证器，创建结果在服务器确认保存私钥后才返回网站；不会把登录当前保险库的通行密钥存回它自己（遇到此限制时请选择 Windows Hello、安全密钥或其他独立密码库）。系统交互与真实网站的完整流程仍待端到端验收。

### Send 与导入导出

- **端到端加密 Send**：「Send」支持端到端加密的文本与文件分享，可配置到期和删除时间、访问次数、停用、密码保护及服务器支持时的接收邮箱验证。文件在 Rust 中加密后上传，桌面端上限为 500 MiB，服务器的配额和策略可能更严格。链接只在明确复制时返回界面；上传失败或取消时会尝试清理本次创建的 Send，不能确认结果时须先刷新核对。
- **个人保险库导入导出**：「设置 → 保险库数据」可导入和导出个人 Bitwarden JSON（明文、账户限定加密、独立密码加密）及明文 Bitwarden CSV。CSV 只支持登录和安全笔记，不能保留通行密钥、密码历史、URI 匹配规则或自定义字段类型；优先使用密码保护的 JSON 做备份。导出需在线检查策略和复验主密码，明文需额外确认，文件不会覆盖现有文件。导入上限为 32 MiB、共 7,000 个条目与文件夹，只新增；遇到未确认结果须先同步。导出不含附件文件、回收站或组织保险库。

<br>

## 当前范围与对照基线

自动测试使用官方公开合成账户和模拟服务器。真实账户及网站通行密钥的端到端验证、真实账户跨重启验证、Duo / WebAuthn 登录验证、SSO、V2 登录与复验、云端 Send 和附件、组织策略、其他导入导出格式及 Firefox 联动仍待完成。完整差距见 [功能对照](docs/PARITY.md)。

`crates/bytewarden-core/src/storage.rs` 中的本机快照加密器是未用于账户保险库的独立原型，不能与官方保险库互操作。Windows 账户缓存另行保存官方 SDK 同步得到的服务器密文，并以当前用户 DPAPI 加密；该开发预览仍不得作为真实密码库的唯一客户端。

对照基线为 Bitwarden Desktop 2026.11.0，GitHub 发布时间为 UTC 2026-11-19 20:49，即北京时间 2026-11-20 04:49；本机官方 MSIX 为 2026.10.4.0。仍需以新版实际安装包逐项验收，不能把 SDK 支持范围当作整个桌面端已完成。

<br>

## 构建与验证

### 环境准备

需要 Rust stable、.NET 10 SDK、MSVC x64 工具、Windows SDK；界面验证使用 `winapp` CLI，合成服务器验收另需 Python 3.10+。先检查现有工具，只安装缺少的组件：

```powershell
rustc --version
cargo --version
dotnet --list-sdks
winapp --version
```

### 快速构建与运行

从仓库根目录运行，点加载构建脚本会为同一 PowerShell 会话配置 MSVC 链接库路径：

```powershell
. ./scripts/build-rust-core.ps1
cargo test --locked -p bytewarden-core
cargo clippy --locked -p bytewarden-core --all-targets -- -D warnings
dotnet run --project tests/Bytewarden.Interop.Tests -- target/debug/bytewarden_core.dll
dotnet build apps/Bytewarden.WinUI/Bytewarden.WinUI.csproj -p:Platform=x64 -p:GenerateAppxPackageOnBuild=true -p:AppxPackageSigningEnabled=false
winapp run apps/Bytewarden.WinUI/Bytewarden.WinUI.csproj --no-build -p Platform=x64
```

> [!TIP]
> 使用固定的预览包身份 `ZHT0711.BytewardenLocalPreview`，不会覆盖官方应用。请勿为常规预览使用 `winapp run --unique-identity`，否则同名条目会出现在开始菜单。当前仅验证 x64；原生构建脚本尚不支持 x86 / ARM64。运行与打包必须使用相同的 `Platform=x64`，否则可能启动之前注册的旧 `AppX` 布局。可向预览传入 `--start-in-tray` 检查隐藏启动，不必注销当前 Windows 会话。

### 自动化测试与压力验收

[Windows CI](https://github.com/ZHT0711/Bytewarden/actions/workflows/windows.yml) 执行 Rust 格式 / 测试 / Clippy、模拟服务自检、C# 筛选和 Release DLL 互操作检查，并核对 MSIX 内的 DLL 与已测 DLL 一致。成功运行的 **Artifacts** 中提供 `Bytewarden-unsigned-msix-x64`，运行摘要列出 SHA-256。Release MSIX 自带 .NET 运行时，不依赖本机 `.pubxml`；它仍是未签名开发包，不是正式安装包。交互式桌面 UI 验收在本机单独运行。

可重复的生成器、大库写入投影、列表元数据搜索、文件夹索引与实际文件夹 UI / 进程资源测量见 [性能测量](docs/PERFORMANCE.md)。文件夹索引避免超长缺失父路径的逐级前缀切片；另以最新 MSIX 独立验证万级平面 / 分支、千层真实链及 8,192 段缺失路径，共 110 项具名压力检查（包含编辑 / 移动下拉和短窗口正文 / 锁定动作）。UIA 端到端耗时和真实进程内存按各自范围记录，不等同于帧率、长期常驻、10,000 条目 UI 或真实服务验收。

#### 列表、文件夹与剪贴板验收

构建最新 Release MSIX 并生成公开合成 fixture 后，可运行以下列表、嵌套文件夹、快捷键、主题和剪贴板验收：

```powershell
. ./scripts/build-rust-core.ps1 -Profile Release
New-Item -ItemType Directory -Force work | Out-Null
$env:BYTEWARDEN_FIXTURE_OUTPUT = Join-Path (Get-Location) 'work/list-fixture-current.json'
cargo test --locked --release -p bytewarden-core write_synthetic_ui_fixture -- --ignored
Remove-Item Env:BYTEWARDEN_FIXTURE_OUTPUT
dotnet build apps/Bytewarden.WinUI/Bytewarden.WinUI.csproj -c Release -p:Platform=x64 -p:GenerateAppxPackageOnBuild=true -p:AppxPackageSigningEnabled=false
./scripts/verify-vault-list-ui.ps1 -Fixture 'work/list-fixture-current.json' -IncludeFolders -IncludeClipboardStress
```

脚本从 MSIX 解包，不依赖可能陈旧的 `AppX` 布局；使用独立临时测试包并在结束时移除，期间会占用测试窗口焦点并写入合成剪贴板内容。它不验证云端、真实账户或系统扩展。`-IncludeFolders` 增加三层文件夹、条目归属、非级联修改与重启后离线浏览；`-IncludeClipboardStress` 再验证长占用、新复制、锁定、正常 / 托盘退出和五秒退出预算，额外花费几分钟。两个开关均省略时只运行原有列表验收。每轮记录实际包与 fixture 的 SHA-256，并把有名称的检查写入独立 `checks.log`。若 `python` 指向未激活的 Windows Store 执行别名，可传 `-PythonExe '实际 Python 可执行文件完整路径'`；脚本在创建测试包前验证解释器，不需要修改全局 PATH。

#### 文件夹压力模式

文件夹压力模式独立运行，不与 `-IncludeFolders` / `-IncludeClipboardStress` 混用；先按 [性能记录](docs/PERFORMANCE.md#2026-11-23-实际万级文件夹与千层路径-winui-验收) 生成四种 SDK 合成密文 fixture，再运行：

```powershell
./scripts/verify-vault-list-ui.ps1 -Fixture 'work/folder-stress-deep.json' -MeasureFolders -FolderSamples 3 -FolderRunLabel optimized
```

该模式还验证草稿默认文件夹、早期真实祖先菜单与锁定，记录 UIA / CLI 合并时间和实际测试进程内存；不读取真实账户，不运行剪贴板步骤。

#### 万条条目压力模式

条目压力模式也独立运行：按 [性能记录](docs/PERFORMANCE.md#2026-11-25-万条条目输入与筛选分配) 生成 **10,000 个真实 SDK 密文条目** fixture，再执行：

```powershell
./scripts/verify-vault-list-ui.ps1 -Fixture 'work/vault-scale-10000.json' -MeasureVault -VaultIdleSeconds 30 -VaultRunLabel optimized
```

此模式以实际 MSIX、持久化直接 UIA / 原生 Unicode 输入探针检查全量加载、搜索、末项可达、复验和锁定；只允许合成测试身份，切换到其它应用会停止输入。记录前台解锁 / 锁定的短期 CPU 与内存。输入到 UIA 就绪不是物理 IME 或帧时间，30 秒空闲不是长期稳定性；慢速输入的 CPU 代价仍需优化。不与文件夹、关联字段或剪贴板模式混用。

#### 关联字段与原生剪贴板事务检查

关联字段使用独立的六条目合成 fixture。构建 Release DLL 和 MSIX 后，从仓库根目录运行：

```powershell
New-Item -ItemType Directory -Force work | Out-Null
$env:BYTEWARDEN_FIXTURE_OUTPUT = Join-Path (Get-Location) 'work/linked-fields-fixture.json'
cargo test --locked --release -p bytewarden-core write_synthetic_linked_field_fixture -- --ignored
Remove-Item Env:BYTEWARDEN_FIXTURE_OUTPUT
./scripts/verify-linked-fields-native.ps1
./scripts/verify-vault-list-ui.ps1 -Fixture 'work/linked-fields-fixture.json' -CheckLinkedFields
```

两个关联字段验收脚本同样支持 `-PythonExe '完整的 Python 可执行文件路径'`，并在启动服务器 / 创建本机缓存之前拒绝失效或低于 3.10 的解释器。

以下原生事务检查也会写入本机合成剪贴板内容，不读取预先存在的剪贴板，不需账户 / 服务器，仅用于 Windows 本机专项验收：

```powershell
dotnet run --project tests/Bytewarden.Interop.Tests -c Release -- --clipboard-native
```

<br>

## 反馈与交流

遇到问题、崩溃或有改进建议？请先检索 [GitHub Issues](https://github.com/ZHT0711/Bytewarden/issues)，再提交复现步骤。桌面端「关于 → 复制版本信息」只复制应用版本、Rust 核心版本和 Windows / 架构信息。请勿附上主密码、令牌或保险库数据。

<br>

## 免责声明

**本项目与 [Bitwarden](https://bitwarden.com/) 或 Bitwarden, Inc. 无任何关联。**

**请注意：** Bytewarden 目前为早期开发预览版本，我们不对使用本软件过程中可能发生的任何数据丢失（包括但不限于密码、附件及其他敏感信息）承担责任。强烈建议定期备份你的保险库数据，且勿将其作为唯一客户端管理重要凭据。

<br>

## Bitwarden-Rust

本项目原名为 **Bitwarden-Rust**，现已更名为 **Bytewarden**，以明确区分官方 Bitwarden 桌面客户端并避免名称及商标混淆。
