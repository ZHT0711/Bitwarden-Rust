# Bytewarden · GPUI 原生桌面预览

这是 Bytewarden 的独立 Rust / GPUI 桌面原型。界面由 GPUI 在本地渲染，不需要 WebView2、浏览器、Node.js 或前端开发服务器。外观按现有 WinUI 客户端重做：深色 Fluent 风格、左侧导航、顶部命令区、分栏列表与详情。保险库页面按 WinUI 的 720 DIP 宽度阈值切换分栏；侧栏遵循 `NavigationView.Auto` 的 1008／641／640 DIP 断点：宽窗展开、中窗默认图标栏、小窗用可覆盖内容的抽屉并支持遮罩关闭。左上角或 `Ctrl+B` 控制侧栏。过渡使用 Fluent 进入／退出缓动；关闭 Windows「动画效果」后跳过动画。

**尚未达到 WinUI 功能完整度，不建议用于主要保险库。** 该版本已静态链接现有 Rust 核心，支持主密码登录、同步、加密缓存、离线解锁和真实保险库读取，不再使用内置演示条目。GPUI 数据目录独立，不覆盖或迁移 WinUI 安装和账户。已接入与已验证的区别见 [迁移验收矩阵](../../docs/GPUI-MIGRATION.md)。

## 在 Windows x64 运行

安装 Rust stable（MSVC x64）、MSVC x64 工具和 Windows SDK，从仓库根目录执行：

```powershell
cargo run --locked --manifest-path apps/Bytewarden.Gpui/Cargo.toml
cargo test --locked --manifest-path apps/Bytewarden.Gpui/Cargo.toml
```

## 快捷键

`Ctrl+B` 切换侧栏，`Ctrl+F` 搜索，`Ctrl+N` 新增，`Ctrl+E` 编辑，`Ctrl+R` 同步，`Ctrl+L` 锁定。可用性随当前页面、登录、锁定和离线只读状态变化。
