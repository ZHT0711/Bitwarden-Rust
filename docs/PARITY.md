# Desktop parity and verification

目标：以 Rust 为安全与业务核心、WinUI 3 为 Windows 外观，逐项复刻当前官方 MSIX 桌面端；桌面端完成后，再优化 Firefox 扩展联动和重复资源占用。

## 基线

- 官方源：`https://github.com/bitwarden/clients/releases/tag/desktop-v2026.11.0`。2026-11-20 经 GitHub API 核对，是当时最新非预发布桌面版本，发布时间为 UTC 2026-11-19 20:49:07。
- 已下载官方 x64 Store APPX，清单确认身份 `8bitSolutionsLLC.bitwardendesktop`、版本 `2026.11.0.0`，包大小和 SHA-256 记录在 [基线记录](upstream-desktop-baseline.json)。仅检查安装包，没有升级本机官方 2026.10.4.0，也尚未完成新版运行界面的逐项对照。
- SDK 固定提交 `56c7617b23501af70221094cdee59e10b2a9d8d2`。SDK 数据类型存在不代表对应官方桌面功能已启用。
- 重写版独立包身份 `ZHT0711.BitwardenRustLocalPreview`，不覆盖官方应用；当前为 x64 开发预览。

## 当前证据与差距

| 范围 | 当前实现 | 仍须完成 / 验证 |
| --- | --- | --- |
| WinUI 外壳 | 原生 NavigationView、系统控件、Mica、列表详情 | 窄窗口、键盘、屏幕阅读器、亮 / 暗主题完整验收 |
| 服务器 | US / EU / 自托管，根 URL 校验和选择保存 | 拆分 API / Identity URL、代理等高级设置 |
| 登录 | 官方 SDK 主密码、认证器 / 二步邮箱 / YubiKey；自托管新设备邮箱验证、验证码重发与登录重试；兼容 NodeWarden 新旧响应字段，可取消、超时 | 云端各地区和用户 NodeWarden 实例验收；SSO、Duo、WebAuthn、设备批准、API 密钥、多账户、V2 账户登录 |
| 同步与解密 | 官方 API、组织密钥初始化、密文内存缓存、SDK 严格解密、失败计数；本机 Vaultwarden 1.37.5 已验证登录后同步和新增条目 | 用户实例、云端与组织账户；密钥轮换、通知增量同步、断网恢复、撤销登录、集合权限、大库性能 |
| 列表与读取 | 八种 SDK 类型、收藏 / 归档 / 回收站、文件夹、名称 / 副标题搜索、按需详情、密码显隐和可关闭的网站图标 | 八类真实数据、V2 图标、嵌套文件夹 / 集合、搜索语法、排序、批量选择、密码历史、关联字段 |
| 访问边界 | Rust 主密码复验、隐藏密码权限、详情 DTO 排除保险库密钥与 FIDO2 私密材料 | V2 复验、组织访问申请 / 审批、全部权限组合与策略 |
| 剪贴板 | 禁止历史 / 云同步、30 秒归属清理、退出清理 | 系统忙、多应用覆盖、退出时序的实际 UI 验证；设置可配置 |
| 生成器 | 官方密码和密码短语生成，按账户与服务器保存生成参数 | 用户名、转发服务与生成历史 |
| 条目写入 | 新增、编辑、批量移动到文件夹、收藏修改、归档、软删除、还原、永久删除；修订冲突与不确定结果同步恢复 | 真实账户覆盖与生产服务兼容性 |
| 我的文件夹 | 搜索、打开文件夹查看条目、条目计数、新建、重命名、单个与批量删除。删除保留条目并清除归属，旧服务器可逐个删除 | 嵌套导航与云端账户验收 |
| 本地状态与锁定 | 进程内会话，手动和自动锁定共用密钥与界面清理。支持按账户配置应用闲置、系统空闲、锁屏、休眠与重启触发，超时可锁定或退出登录。主密码可在同一进程中离线解锁已同步数据 | 跨重启的加密缓存与会话恢复、系统锁屏／休眠的实机验收、PIN、Windows Hello 解锁、托盘、快捷键、开机启动与真实账户验证 |
| 附件 / TOTP / 通行密钥 | 附件列表与本地解密下载。登录条目支持手动录入验证器密钥、离线生成验证码、倒计时与复制。通行密钥显示元数据、编辑保留、明确移除。实验性 Windows 提供程序已接入 COM 注册、系统状态读取、请求确认、Windows Hello、官方 SDK 创建及断言与加密保存 | Windows Hello 与真实网站端到端验收、通行密钥替换体验、附件上传和删除、云端与大型附件验收、TOTP 二维码录入、SSH 代理 |
| Send / 导入导出 | 未实现 | 完整 Send、文本 / 文件、导入格式、加密 / 明文导出与确认 |
| 其他桌面功能 | 关于页、基础设置占位 | 设置、语言、通知、更新、账户 / 组织策略、帮助等逐项对照 |
| 构建发布 | 本地 x64 WinUI / Rust、未签名 MSIX；增加 Windows CI | CI 通过、干净构建、签名分发、ARM64 / x86、更新与回滚 |
| Firefox 联动 | 尚未开始，等待桌面端完成 | 定位现有扩展源码；native messaging、可信配对、锁定传播、单一后台核心与资源对比 |

## 验证分层

1. Rust 测试用官方 SDK 的公开合成账户生成实际密文，覆盖 SDK API、V2 blob 解密、主密码复验、数据损坏、跨账户拒绝、组织解密和详情投影。
2. C# 控制台检查 UI 实际使用的分类 / 文件夹 / 搜索代码，并可加载实际 Rust DLL 检查 ABI 与挂起请求取消。
3. `winapp ui` 检查运行中的独立预览包；本机模拟服务器的界面测试必须标记合成数据，不能作为真实服务验证。
4. 真实账户、官方最新 MSIX 对照、性能、密钥轮换和 Firefox 资源收益是独立验收，不能用单元测试替代。

## 2026-11-20 本机验证记录

- 53 项 Rust 测试通过、1 项合成导出测试忽略；格式检查与 Clippy（warnings denied）通过。测试覆盖八种条目、组织密钥、严格解密失败计数、V2 blob、主密码复验与访问边界。
- 托管控制台通过实际 Rust DLL 和仅限回环的合成服务器完成 54 项检查，包括挂起请求取消、新设备验证挑战识别、验证码重发、错误码区分、正确验证码重试，以及读写生命周期；没有请求真实账户。
- 通过 `winapp ui` 驱动独立预览包，使用仅监听 `127.0.0.1` 的临时模拟服务器与 SDK 公开合成账户：登录后加载 2 个密文条目，普通与受保护条目详情读取成功；复制用户名后核对内容，30 秒后剪贴板为空。
- 模拟下一次同步返回 HTTP 503，界面报告同步失败且继续保留 2 个条目；搜索不匹配后数量为 0，之前的详情已移除；退出后条目为 0。
- 上述是合成账户连通性检查，不能替代真实 Bitwarden / Vaultwarden 账户、组织策略与生产加密迁移验收。
- 自托管新设备验证沿用官方客户端的独立步骤：Identity 返回新设备验证挑战时展示邮箱验证码；重发通过官方 SDK 获取 KDF、本机派生授权哈希后请求新设备验证码接口，登录重试携带 newDeviceOtp。该状态不会与普通二步验证混用。
- 将 SDK 的 Identity middleware 扩展放在仓库内固定 SDK core 副本中。WinUI x64 构建通过；新设备验证协议已在合成服务器上通过真实 DLL 验证。还没有用用户的自托管实例进行端到端确认。
- 锁定与解锁增加了真实 DLL 生命周期检查，覆盖错误主密码、锁定时拒绝读写、离线恢复已确认的编辑、组织密钥恢复、写入结果待核对及服务器更换密钥后的重新登录。另用 `winapp ui` 验证了锁定后清除条目、停止合成服务器后解锁和锁定状态下退出登录。
- 在本机 Vaultwarden 1.37.5 复现了登录成功后首次同步返回 `internal`。同步资料包含 `accountKeys` 时，旧实现会调用依赖状态桥的 SDK 同步处理器。原生客户端目前使用进程内状态，已按 SDK 状态桥的注册状态调用处理器，并继续校验账户密钥资料。修复后用同一个合成账户同步、创建条目并再次同步成功。CI 模拟的同步资料也加入了 `accountKeys`。对照了 [官方桌面共享同步实现](https://github.com/bitwarden/clients/blob/desktop-v2026.11.0/libs/common/src/platform/sync/default-sync.service.ts) 和 [Vaultwarden 1.37.5 同步实现](https://github.com/dani-garcia/vaultwarden/blob/1.37.5/src/api/core/ciphers.rs)。
- NodeWarden 的公开预登录响应同时返回两套字段名，旧的 SDK 兼容层会拒绝解析。回归测试加入两套预登录参数、令牌解锁参数和同步资料，并覆盖值为 `null` 的认证器挑战。自托管联机验收仍需用户账户确认。对照了 [NodeWarden 身份验证实现](https://github.com/shuaiplus/NodeWarden/blob/main/src/handlers/identity.ts) 和 [同步实现](https://github.com/shuaiplus/NodeWarden/blob/main/src/handlers/sync.ts)。
- 修复生成器初始化时控件尚未就绪导致的崩溃，WinUI 界面验证密码重新生成及六词密码短语。
- 对照 [官方桌面 TOTP 服务](https://github.com/bitwarden/clients/blob/desktop-v2026.11.0/libs/common/src/vault/services/totp.service.ts) 接入条目验证码。Rust 测试覆盖 RFC 向量、周期边界和 Steam，DLL 测试覆盖会员权限、主密码复验、隐藏密钥及锁定后的授权撤销。WinUI 中验证了倒计时、验证码轮换和保险库锁定后清除显示。

## 2026-11-21 文件夹验证

- 对照官方桌面版 [文件夹操作](https://github.com/bitwarden/clients/blob/desktop-v2026.11.0/libs/vault/src/services/my-folder-actions.service.ts) 和 [归属清理](https://github.com/bitwarden/clients/blob/desktop-v2026.11.0/libs/common/src/vault/services/folder/folder.service.ts)。新建与改名通过 SDK 加密名字，收到服务器响应后只更新文件夹元数据。删除只清除对应的归属，避免重新解密所有条目。
- DLL 测试覆盖新建、改名、分配条目、单个与批量删除、旧服务逐个删除和部分成功后的同步恢复。另用临时本机 Vaultwarden 1.37.5 合成账户确认兼容路径，重新同步后文件夹已删除、条目仍在。WinUI 预览验证了创建、搜索、改名和删除。
- 对照 [官方批量移动对话框](https://github.com/bitwarden/clients/blob/desktop-v2026.11.0/libs/vault/src/components/bulk-action-dialogs/bulk-move-dialog/bulk-move-dialog.component.ts) 使用 SDK 移动条目，只更新个人文件夹归属。互操作测试确认受主密码复验的条目仍保留原有密码、自定义字段和复验要求，部分移动失败后可以通过同步核对。WinUI 预览在本机 Vaultwarden 批量移动两个合成条目，重新同步后从文件夹名称直接打开并看到对应条目。

## 2026-11-21 附件下载验证

- 对照官方桌面版 [下载组件](https://github.com/bitwarden/clients/blob/desktop-v2026.11.0/libs/vault/src/components/download-attachment/download-attachment.component.ts)，通过 SDK 请求下载链接并解密内容。只有已打开的条目可授权下载。数据传输使用独立 HTTP 客户端，WinUI 接收文件名、大小和保存结果。
- 真实 DLL 与合成服务器覆盖主密码复验、下载字节匹配、内容篡改、传输取消、授权过期和锁定。WinUI 通过 Windows 保存对话框从本机 Vaultwarden 1.37.5 保存了合成附件，使用包含中文的目标路径并逐字节核对内容。

## 后续顺序

2026-11-20 本轮通行密钥验证包括 SDK 生成与加密保存、服务器确认后读取、断言签名和请求绑定、RP 过滤、排除列表、修订冲突、保留现有密码与 TOTP、不确定写入与缺失私钥阻止返回成功。DLL 检查覆盖伪造请求 ID 无法触发通行密钥操作。WinUI 已实测元数据展示、明确移除和设置注册，Windows 设置已列出独立的预览版提供程序。系统安全交互尚待本人验证，尚未宣告网站创建和登录验收通过。

Windows 包声明 COM 扩展后，请使用相同项目身份启动开发预览。WinApp 的 unique-identity 模式不支持这类系统扩展，通行密钥联调不能用该模式。参考封装固定于官方 desktop-v2026.11.0 提交 28b247ac34877882aaef4faea082a9f3bb201114，来源和改动记录在 vendor/win_webauthn/UPSTREAM.md。

通行密钥请求在首次登录后同步保险库，并继续处理同一个 Windows 请求。没有匹配凭据时返回系统凭据未找到状态，无凭据列表的请求只使用可发现凭据。创建时阻止把当前 Web Vault 的通行密钥保存在它自己的保险库中。

继续补齐跨重启的加密本地状态、附件、Send、导入导出和桌面集成，再对真实账户与官方客户端逐项验证。桌面功能验收完成后进入 Firefox 联动阶段。
