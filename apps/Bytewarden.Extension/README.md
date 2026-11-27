# Bytewarden Firefox 轻量级浏览器扩展

与 Bytewarden GPUI 原生桌面端实时联动的零常驻内存 Firefox WebExtension（Manifest V3）。

## 架构特点

- **零重复核心、极低内存**：扩展端不存储任何加密快照、不加载庞大 WASM 加密库（常驻内存 `< 2 MB`），所有密钥管理与加解密均保留在 Rust 桌面端核心。
- **实时双向同步**：通过 `com.bytewarden.desktop` 原生通信通道与桌面端全双工命名管道连接；桌面端解锁、锁定或同步时，Firefox 状态与图标角标毫秒级同步。
- **生物识别填充门禁**：支持在桌面端直接发起 Windows Hello 解锁，以及可选的「填充前要求生物识别或手动授权」与条目主密码二次确认（Reprompt）。
- **零卡顿网页内填充**：内容脚本不挂载全局 `MutationObserver`，仅在聚焦登录框时通过隔离的 Shadow DOM 渲染轻量级填充盾牌图标；同时支持 `Alt+Shift+L` 快捷键与右键菜单一键填充。

## 在 Firefox 中安装使用

1. 打开 Bytewarden 桌面端 → **设置 → 安全与超时 → Firefox 浏览器扩展联动**，点击 **注册到 Firefox**。
2. 在 Firefox 地址栏打开 `about:debugging#/runtime/this-firefox`。
3. 点击 **临时载入附加组件…（Load Temporary Add-on…）**，选择本目录下的 `manifest.json`（或打包生成的 `bytewarden-firefox.zip`）。
   - 若使用 **Firefox Developer Edition / Nightly**，可在 `about:config` 将 `xpinstall.signatures.required` 设为 `false`，直接拖入同目录下的 `bytewarden-firefox.xpi` 永久安装。
