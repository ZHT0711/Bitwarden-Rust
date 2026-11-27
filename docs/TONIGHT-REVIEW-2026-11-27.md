# 2026-11-26 晚至 27 日代码复审与开发记录

本轮任务是重新审视当晚工作树并继续开发。审查基线为 `ed231db8b56e382a78ab2234ab6f273581a695a8`。仓库原有未提交修改保留；GPUI、桌面适配层和 vendor GPUI 均含未跟踪的新实现，审查范围包含这些源码，而不只是 tracked diff。本记录不表示完整 GPUI 迁移、真实服务验收或所有历史性能目标已完成，也不表示已提交、推送或运行远程 CI。

## 审查发现、处置与回归

四份只读审查报告共提出 15 项发现，列表导航与命令面板的草稿保护存在重叠。报告均是静态证据；以下测试和运行证据另行采集。报告副本保留于 `work/tonight-review/reviews/`。

| 发现 | 实际处置 | 验证位置／方式 |
| --- | --- | --- |
| 关闭主密码复验后，迟到回复重新打开编辑器 | `EditorLoad` 绑定表单代次、条目 ID、克隆意图；关闭使代次失效 | `editor_workflow_tests::a_queued_editor_reply_cannot_reopen_a_dismissed_prompt_for_the_same_item` |
| 系统锁屏事件取消不匹配策略的当前账户请求 | 入口只记录事件，调度按账户策略撤销；忙碌时保留事件 | `timeout_workflow_tests::system_event_entry_preserves_a_nonmatching_worker_and_revokes_a_pending_active_target`，及 timeout 状态测试 |
| 点击列表丢弃未保存草稿 | `choose_item()` 拒绝 editor 存在时导航 | `editor_workflow_tests::ordinary_and_unconfirmed_drafts_reject_list_and_palette_navigation`，含普通与 `must_resync` 草稿 |
| 文件夹未确认创建在同步后可重复提交 | 表单 `must_reconcile` 粘性禁重提；全局同步成功不恢复旧操作；禁写时仍可取消 | `folder_workflow_tests` 实际表单／取消入口；生产同步复用 `accept_synced_overview()` |
| 类型切换保存不兼容关联字段 | 保存前检查当前条目类型的关联 ID，拒绝保存并保留原选择 | `editor_workflow_tests::linked_field_type_switch_blocks_save_and_preserves_the_original_choice` |
| URI 保存额外普通明文副本 | `original: Option<String>` 改为 `original_missing: bool` | `editor_workflow_tests::uri_construct_collect_keeps_missing_empty_and_present_values_distinct`，含缺失、显式空、非空及匹配模式 |
| 隐藏窗口清理后，编辑输入仍被强引用持有 | `Editor::clear()` 显式清理全部当前／缓存类型输入；关闭前清理再移除；保存时 editor 可达且冻结 | `closing_and_account_cleanup_wipe_all_retained_input_entities_before_another_frame`；保存失败恢复、保存成功和陈旧回复回归 |
| 显露秘密进入普通文本缓存 | 新增可撤销 `Zeroizing` 渲染缓冲和 uncached shaping；详情、TOTP、生成结果及历史使用该路径；输入原值借用整形 | `sensitive_text`、`generator_security_tests`、Input 回归；检查保留缓冲同步为空及普通帧缓存不持有合成标记；中文／emoji／多行几何对照 |
| 自动生成归一化丢失关闭类别前的 minimum | 禁用时保留并限制 `last_minimum_*`，不再用 0 覆盖旧值 | generator 的关闭→归一化→重开回归，数字和特殊字符都恢复 5 |
| 自启动检测只看注册表值存在 | 校验 REG_SZ、完整读取长度、终止符、当前程序路径及完整 `--start-in-tray` 命令 | startup 纯函数回归，覆盖旧路径、空值、截断、额外／缺少参数和带空格路径；不修改真实 Run 项 |
| 命令面板跳转丢弃草稿 | 打开和执行均复核统一导航守卫 | 上述草稿入口回归；`palette_submit_ignores_empty_results_and_rechecks_state_after_opening` |
| message-only 托盘窗口收不到 Explorer 广播 | 改用隐藏顶层 tool window；后续广播允许重新注册，失败时恢复应用窗口 | tray 的隔离 desktop 真实 HWND_BROADCAST 测试，shell 调用使用注入结果，覆盖成功、失败恢复及再次成功 |
| 面板 Enter 被全局 Submit 消费 | 全局 action 与局部 Enter 复用 `submit_command_palette()`；IME 未提交不执行 | `palette_enter_uses_the_bound_submit_action_exactly_once` 和 `palette_enter_during_ime_composition_does_not_execute`，使用生产 `bind_shortcuts()` |
| 面板跳转被原搜索／归档筛选立即关闭 | 清空主搜索和查询；归档目标切到 Archived；刷新投影 | `palette_jump_clears_stale_search_and_keeps_archived_target_in_projection` |
| 初始化失败无重试入口 | 未初始化且非忙碌时显示真实重试按钮；初始化拒绝 closing 状态 | 初始化一次失败、点击重试、第二次成功的 GPUI 控件回归 |

最后一次清理路径复核又修复了删除行的同类缺口：URI 和自定义字段移除前显式清空其 Input。否则被输入处理器额外持有的实体已不在 editor 遍历范围内，后续锁定清理也无法触及。`removed_uri_and_custom_rows_wipe_retained_inputs_before_another_frame` 覆盖 URI、自定义字段名称和值、越界不误删及移除后关闭；无需推进下一帧。

保存期间的补充保护使输入保持可达并冻结，worker 持有完整 Draft 和清零密码，UI 只留下必要元数据。成功、失败、关闭及 queued lock／陈旧代次均检查对应生命周期。批量操作另经源码核对：清空回收站每组至多 500，元数据操作首错停止，仅移除确认成功项；部分成功或未确认结果要求同步核对，未把这次静态核对称为新的完整批量 UI 验收。

`initial_state()` 提取仅用于复用状态构造；生产 `new()` 仍负责设置加载、焦点、窗口图标、输入观察者、外观观察、关闭决策、系统事件、生成器观察和初始化。生产快捷键集中于 `bind_shortcuts()`，测试复用真实绑定。Escape 关闭弹层／认证表单；内联编辑器仍通过显式取消退出，不新增静默放弃草稿的导航行为。编辑器忙碌期间输入只读、鼠标事件捕获和取消入口的 ready 复核均已核对。

## 认证集成的附加修正

- 托管生命周期测试原来要求第二个账户的预登录 403 返回通用 `Failed`；现在明确断言 `ServerError`，另行验证当前账户和缓存未被替换。HTTP 错误本身不足以认定主密码错误。
- 记住二步验证的错误密码 fixture 原来用非 identity 格式的通用错误体，新的分类器会将其识别为服务器响应问题。fixture 改为明确的 `invalid_grant` identity 拒绝体；原生托管测试明确断言 `AuthenticationRejected`，同时保留不建立认证会话的不变量。
- 新 HTTP 自测确认有效 remembered token 不能绕过不匹配的主密码哈希，返回 400／`invalid_grant` 且不增加接受计数。没有放宽为“任意非成功状态”。
- 旧认证状态数值保持不变；新增 NetworkError／ServerError／AuthenticationRejected 的 Rust 与托管映射由现有分类、消息和 ABI 检查覆盖。

## 验证与制品索引

本机原始证据位于 ignored 的 `work/tonight-review/`。最终一组编译、测试、格式与 Clippy 命令由 `run-audit-gates.ps1` 顺序执行，命令、UTC 起止时间及退出码写入 `audit-gates.json`；`audit-gates-finished.json` 同时记录失败数、源码前后是否相同及 Release SHA-256。不得把较早 `current-gates-finished.json` 中未通过的历史尝试称为最终结果。

| 验证范围 | 证据 |
| --- | --- |
| GPUI 完整 test-support 套件：171 passed，0 failed，1 ignored | `audit-gpui-test.log`，含本轮新增删除行回归 |
| 核心 workspace：132 passed，0 failed，5 ignored；desktop fixture 默认另有 1 ignored | `audit-core-test.log`；该 desktop fixture 后续另行显式执行通过 |
| GPUI／核心严格 Clippy、格式、vendor 局部格式及 diff check | `audit-gates.json` 中逐项退出码及同名前缀日志 |
| 托管独立检查：223 checks passed | `audit-managed-test.log` |
| 合成服务 HTTP 自测：11 passed | `audit-fixture-self-test.log`、最新 `fixture-self-test-final.log` |
| 明确执行的原生 Rust desktop ignored 集成 | `fixture-current-results.json`、`desktop-fixture-final.log`；使用公开合成 fixture 和 127.0.0.1 健康检查 |
| 托管实际 DLL 全流程：525 checks passed | 最新 `fixture-current-results.json`、`fixture-current-finished.json`（success=true）、`managed-native-final.log`；含账户隔离、离线恢复、取消、PIN、remembered-2FA 等实际入口 |
| 核心 DLL 和 GPUI Release | `audit-core-release.log`、`audit-gpui-release.log`、最终证据清单中的大小及 SHA-256 |
| 源码对应关系 | `audit-source-before.json` 与 `audit-source-after.json`，含工作树和本地 SDK 依赖源码；最终证据清单另核对交付时未漂移 |
| 最终制品原生窗口实测：退出码 0，192 DPI | 隔离 LOCALAPPDATA、未配置真实账户，实际生成／清空、窄窗渲染及隐藏后 WM_CLOSE；最新 `desktop-delivery-finished.json`、`desktop-delivery-generated.png`、`desktop-delivery-cleared.png`，图像均已实际验看 |

12 条最终本机门禁全部返回 0，前后 493 个源码、清单和本地依赖文件摘要相同。最终 GPUI Release SHA-256 为 `C7358E3C66B58445D52B954AA2FB543BD0DE661A46709EB70FDB9A9829CD0F73`，核心 DLL 为 `20E48E791753149A53E5960D240561DB711181D781D9B4BE0BDB1F5388A97768`；实际窗口和托管原生集成分别核对同一制品摘要。交付时再次核对源码、制品、命令、日志、文档和本轮进程清理，汇总于 `work/tonight-review/delivery-evidence.json`。

保留先前失败日志，以解释两条认证断言及 fixture 修正。`managed-native-first-failure.log` 和 `managed-native-malformed-fixture-failure.log` 是失败尝试，不是当前验收结果。早期 `desktop-smoke-generated.png` 未真正触发生成，因此不作为生成／清空证据；有效证据为设置线程 DPI awareness 后实际验看的 `desktop-confirm-generated.png`、`desktop-confirm-cleared.png`。

## 本轮结论

本轮已完成四份报告的逐项处置和补充清理修复，相关源码、状态／事件入口回归、核心／托管／原生集成、最终 Release 实测及交付记录形成闭环。在上述复审范围内没有未处置的 P1／P2 发现；下列未覆盖环境和上游警告保持明确边界。未提交或推送任何变更，原有未提交工作保留。

## 范围和已知限制

- GPUI 原生剪贴板测试仍为 ignored，依赖专用 Windows window station 权限；本轮未操作真实剪贴板，不能报告它已通过。核心的其他故意忽略测试也不计入通过数；desktop 集成是另行显式执行。
- 托盘测试证明隔离 desktop 的真实广播投递和恢复分支；未重启真实 Explorer，不能扩大为真实 Explorer 完整验收。
- LSP 六文件均未获得 primary server 确认；“0 diagnostics”不算 clean。编译与类型验证依据 Cargo／dotnet 的实际结果。
- `proc-macro-error2 v2.0.1` 有上游 future-incompatibility：私有 `extern crate proc_macro` 被重导出（E0365）。`future-incompatibility-current.log` 记录具体位置。当前锁定依赖和工具链可通过严格 Clippy／Release；本轮不修改用户 Cargo registry，也不为消除上游警告引入未经验证的依赖更新。未来 Rust 升级须重新确认该依赖。
- 可撤销缓冲和 uncached shaping 解决已确认的应用层所有权与普通文本缓存问题，不是进程内存转储、系统字体引擎、GPU 或显示设备的全面擦除证明。
- 真实账户、真实服务、完整 IME／可访问性、多种 DPI 与 WinUI 像素／动画对照、实际万条大库及长期 CPU／内存仍遵循 [GPUI 迁移矩阵](GPUI-MIGRATION.md)，不因本轮局部修复被改成完成。
