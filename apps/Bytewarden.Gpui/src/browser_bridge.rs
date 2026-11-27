//! Firefox WebExtension <-> Bytewarden Desktop Native Messaging bridge.
//!
//! Architecture (zero duplicate vault core in browser, < 2 MB relay footprint):
//! 1. Firefox spawns `Bytewarden-GPUI-Preview.exe --native-messaging` via standard
//!    WebExtension Native Messaging (`com.bytewarden.desktop`).
//! 2. `maybe_run_native_messaging_cli()` runs at line 1 of `main()` before GPUI,
//!    DirectX, or `bytewarden-core` initialize, forwarding 4-byte LE length-prefixed
//!    JSON frames over a session-scoped Win32 Named Pipe (`\\.\pipe\LOCAL\Bytewarden.BrowserBridge.<tag>`).
//! 3. Inside Bytewarden Desktop, `BrowserBridgeState` listens on the Named Pipe on a
//!    single OS thread (0 polling, 0 idle CPU), verifies client session ownership,
//!    broadcasts real-time `state_changed` events whenever Desktop locks/unlocks/syncs,
//!    and gates credential autofill/copy behind Windows Hello or an in-app approval modal
//!    when configured or when an item requires master-password reprompt.

use std::{
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bytewarden_core::desktop::{self, Draft, DraftUri, Item, Request, Session, Totp};
use futures::{
    StreamExt,
    channel::{mpsc, oneshot},
};
use gpui::{Context, Div, Entity, FontWeight, Task, Window, div, prelude::*, px};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::{VaultPreview, input::Input, theme, ui};

pub const NATIVE_HOST_NAME: &str = "com.bytewarden.desktop";
pub const FIREFOX_EXTENSION_ID: &str = "bytewarden@zht0711.github.io";
const MAX_FRAME_BYTES: usize = 256 * 1024;

pub fn read_native_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid native messaging frame length",
        ));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn write_native_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid native messaging frame length",
        ));
    }
    let len = (payload.len() as u32).to_le_bytes();
    writer.write_all(&len)?;
    writer.write_all(payload)?;
    writer.flush()
}

pub fn pipe_name_for_scope(scope_tag: &str) -> String {
    format!(r"\\.\pipe\LOCAL\Bytewarden.BrowserBridge.{scope_tag}")
}

fn default_pipe_name() -> String {
    pipe_name_for_scope(&crate::tray::current_scope_tag())
}

/// Extract normalized DNS host (lowercase, without port or trailing dot) from a URL or bare host.
pub fn extract_url_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let after_scheme = if let Some((scheme, rest)) = trimmed.split_once("://") {
        let s = scheme.to_ascii_lowercase();
        if !matches!(s.as_str(), "http" | "https") {
            return None;
        }
        rest
    } else if trimmed.contains(':') && !trimmed.starts_with('[') {
        // Reject non-web schemes like `about:debugging` or `moz-extension:...` unless `host:port`
        let (left, right) = trimmed.split_once(':')?;
        if right
            .split('/')
            .next()
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        {
            left
        } else {
            return None;
        }
    } else {
        trimmed
    };
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split_once(']')?.0
    } else {
        host_port.split(':').next()?
    };
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// Compute registrable base domain (handling common two-level public suffixes like `.co.uk`, `.com.cn`).
pub fn registrable_domain(host: &str) -> &str {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() <= 2 {
        return host;
    }
    let last = parts[parts.len() - 1];
    let second_last = parts[parts.len() - 2];
    let multi_part_tld = last.len() == 2
        && matches!(
            second_last,
            "co" | "com" | "org" | "net" | "gov" | "edu" | "ac" | "ne" | "or"
        );
    let keep = if multi_part_tld { 3 } else { 2 };
    if parts.len() <= keep {
        return host;
    }
    let skip = parts.len() - keep;
    let mut byte_offset = 0;
    for part in &parts[..skip] {
        byte_offset += part.len() + 1;
    }
    &host[byte_offset..]
}

/// Zero-allocation ASCII case-insensitive substring check, falling back to Unicode
/// lowercasing only when `haystack` contains non-ASCII characters (`needle_lower` is already lowercase).
#[inline]
fn contains_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.len() < needle_lower.len() {
        return false;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let nb = needle_lower.as_bytes();
        return haystack
            .as_bytes()
            .windows(nb.len())
            .any(|win| win.eq_ignore_ascii_case(nb));
    }
    haystack.to_lowercase().contains(needle_lower)
}

/// Score how well a vault item's `website_host` matches the active browser tab host (`0` = no match).
pub fn domain_match_score(tab_host: &str, item_host: &str, favorite: bool) -> u32 {
    let Some(item_norm) = extract_url_host(item_host).or_else(|| {
        let h = item_host.trim().trim_end_matches('.').to_ascii_lowercase();
        (!h.is_empty()).then_some(h)
    }) else {
        return 0;
    };
    let fav_bonus = if favorite { 5 } else { 0 };
    if tab_host == item_norm {
        return 100 + fav_bonus;
    }
    let tab_base = registrable_domain(tab_host);
    let item_base = registrable_domain(&item_norm);
    if !tab_base.is_empty() && tab_base.contains('.') && tab_base == item_base {
        return 80 + fav_bonus;
    }
    0
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRequest {
    #[serde(default)]
    id: u64,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeItemSummary {
    pub id: String,
    pub name: String,
    pub username: String,
    pub host: Option<String>,
    pub favorite: bool,
    pub reprompt: bool,
}

impl BridgeItemSummary {
    fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            username: item.subtitle.clone(),
            host: item.website_host.clone(),
            favorite: item.favorite,
            reprompt: item.reprompt,
        }
    }
}

struct SecretPayload {
    username: Zeroizing<String>,
    password: Zeroizing<String>,
    totp: Option<Zeroizing<String>>,
    totp_remaining: Option<i64>,
    profile_fields: Option<serde_json::Map<String, Value>>,
}

pub struct PendingBrowserApproval {
    pub item_id: String,
    pub item_name: String,
    pub username: String,
    pub origin_host: String,
    pub action_label: &'static str,
    pub needs_password: bool,
    pub password_input: Entity<Input>,
    pub error: Option<String>,
    pub busy: bool,
    reply: Option<oneshot::Sender<Result<SecretPayload, String>>>,
}

pub struct BrowserBridgeState {
    pub host_registered: bool,
    pub connected_clients: usize,
    pub pending_approval: Option<PendingBrowserApproval>,
    #[cfg(windows)]
    server: Option<win_pipe::BridgeServer>,
    task: Option<Task<()>>,
}

impl Default for BrowserBridgeState {
    fn default() -> Self {
        Self {
            host_registered: is_firefox_host_registered(),
            connected_clients: 0,
            pending_approval: None,
            #[cfg(windows)]
            server: None,
            task: None,
        }
    }
}

impl BrowserBridgeState {
    pub fn has_modal(&self) -> bool {
        self.pending_approval.is_some()
    }
}

pub fn locate_extension_directory() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let bundled = dir.join("Extension");
        if bundled.join("manifest.json").is_file() {
            return bundled;
        }
    }
    let workspace = PathBuf::from(r"e:\Bytewarden\apps\Bytewarden.Extension");
    if workspace.exists() {
        return workspace;
    }
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".bytewarden")
        .join("firefox-extension")
}

pub fn native_manifest_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
    Some(
        home.join(".bytewarden")
            .join("native-messaging")
            .join(format!("{NATIVE_HOST_NAME}.json")),
    )
}

#[cfg(windows)]
pub fn is_firefox_host_registered() -> bool {
    let Some(manifest) = native_manifest_path() else {
        return false;
    };
    manifest.is_file() && win_pipe::registry_points_to(&manifest)
}

#[cfg(not(windows))]
pub fn is_firefox_host_registered() -> bool {
    false
}

#[cfg(windows)]
pub fn register_firefox_native_host() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法获取当前程序路径：{e}"))?;
    let manifest_path =
        native_manifest_path().ok_or_else(|| "无法读取 USERPROFILE 目录".to_owned())?;
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("无法创建清单目录：{e}"))?;
    }
    // Firefox on Windows requires `path` to point to an executable or `.cmd` wrapper when passing CLI flags.
    // Because Native Messaging manifests do NOT support an `args` field, we generate a zero-window
    // tiny `.cmd` trampoline OR a dedicated `--native-messaging` shim!
    // EVEN BETTER: a tiny compiled/hardlinked `bytewarden-native-host.exe` (hardlink to the same exe,
    // which detects its own filename `bytewarden-native-host` so NO `.cmd` console flash ever happens!).
    let host_exe = create_or_update_native_host_binary(&exe, manifest_path.parent().unwrap())?;
    let manifest = json!({
        "name": NATIVE_HOST_NAME,
        "description": "Bytewarden Desktop Native Messaging Bridge",
        "path": host_exe.to_string_lossy(),
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID]
    });
    let serialized = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(&manifest_path, serialized).map_err(|e| format!("写入原生通信清单失败：{e}"))?;
    win_pipe::write_firefox_registry_key(&manifest_path)?;
    Ok(manifest_path)
}

#[cfg(not(windows))]
pub fn register_firefox_native_host() -> Result<PathBuf, String> {
    Err("当前平台暂不支持注册 Windows Firefox 原生通信清单".into())
}

#[cfg(windows)]
fn create_or_update_native_host_binary(
    current_exe: &std::path::Path,
    target_dir: &std::path::Path,
) -> Result<PathBuf, String> {
    let host_exe = target_dir.join("bytewarden-native-host.exe");
    let needs_copy = match (std::fs::metadata(current_exe), std::fs::metadata(&host_exe)) {
        (Ok(src), Ok(dst)) => src.len() != dst.len() || src.modified().ok() != dst.modified().ok(),
        _ => true,
    };
    if needs_copy {
        let _ = std::fs::remove_file(&host_exe);
        // Try zero-disk-space hard link first, falling back to file copy across drives.
        if std::fs::hard_link(current_exe, &host_exe).is_err() {
            std::fs::copy(current_exe, &host_exe)
                .map_err(|e| format!("创建原生通信宿主程序失败：{e}"))?;
        }
    }
    Ok(host_exe)
}

/// Check at line 1 of `main()` whether this process was invoked by Firefox Native Messaging
/// (either via `--native-messaging` flag OR via the `bytewarden-native-host.exe` binary name).
/// Runs with zero GPUI/DirectX/core initialization (< 2 MB private working set).
pub fn maybe_run_native_messaging_cli() -> bool {
    let explicit_flag = std::env::args_os().any(|arg| arg == "--native-messaging");
    let by_exe_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .is_some_and(|stem| stem.eq_ignore_ascii_case("bytewarden-native-host"));
    if !explicit_flag && !by_exe_name {
        return false;
    }
    #[cfg(windows)]
    {
        let _ = win_pipe::run_stdio_relay(&default_pipe_name());
    }
    true
}

enum InboundBridgeEvent {
    ClientCount(usize),
    Request {
        request: WireRequest,
        responder: oneshot::Sender<Vec<u8>>,
    },
}

impl VaultPreview {
    pub(crate) fn sync_browser_bridge_service(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.browser_bridge.host_registered = is_firefox_host_registered();
        if !self.settings.browser_integration || self.closing {
            self.stop_browser_bridge(cx);
            return;
        }
        #[cfg(windows)]
        if self.browser_bridge.server.is_some() {
            self.broadcast_browser_state();
            return;
        }
        #[cfg(windows)]
        {
            // Keep `bytewarden-native-host.exe` in sync silently if already registered.
            if self.browser_bridge.host_registered {
                let _ = register_firefox_native_host();
            }
            let (tx, mut rx) = mpsc::unbounded::<InboundBridgeEvent>();
            match win_pipe::BridgeServer::start(&default_pipe_name(), tx) {
                Ok(server) => {
                    self.browser_bridge.server = Some(server);
                    self.browser_bridge.task = Some(cx.spawn_in(window, async move |weak, app| {
                        while let Some(event) = rx.next().await {
                            match event {
                                InboundBridgeEvent::ClientCount(count) => {
                                    let _ = weak.update(app, |this, cx| {
                                        if this.browser_bridge.connected_clients != count {
                                            this.browser_bridge.connected_clients = count;
                                            cx.notify();
                                        }
                                    });
                                }
                                InboundBridgeEvent::Request { request, responder } => {
                                    Self::dispatch_browser_request(&weak, app, request, responder)
                                        .await;
                                }
                            }
                        }
                    }));
                    self.broadcast_browser_state();
                }
                Err(err) => {
                    self.client.notice = format!("浏览器桥接服务启动失败：{err}");
                }
            }
        }
    }

    pub(crate) fn stop_browser_bridge(&mut self, cx: &mut Context<Self>) {
        self.cancel_browser_approval(cx);
        self.browser_bridge.task = None;
        self.browser_bridge.connected_clients = 0;
        #[cfg(windows)]
        {
            if let Some(server) = self.browser_bridge.server.take() {
                let locked = Self::serialize_event(
                    "state_changed",
                    json!({
                        "desktopRunning": !self.closing,
                        "enabled": self.settings.browser_integration,
                        "session": "locked",
                        "accountEmail": Value::Null,
                        "helloEnabled": false,
                        "biometricGate": self.settings.browser_biometric_gate,
                        "itemCount": 0,
                    }),
                );
                server.broadcast(&locked);
                server.stop();
            }
        }
    }

    pub(crate) fn broadcast_browser_state(&self) {
        #[cfg(windows)]
        if let Some(server) = &self.browser_bridge.server {
            if server.client_count() == 0 {
                return;
            }
            let payload = self.current_browser_status_json();
            let frame = Self::serialize_event("state_changed", payload);
            server.broadcast(&frame);
        }
    }

    fn current_browser_status_json(&self) -> Value {
        let session = match self.client.state() {
            Session::Unlocked if self.client.overview.is_some() => "unlocked",
            Session::Unlocked => "unlocked",
            Session::Locked => "locked",
            Session::SignedOut => "signedOut",
        };
        let active = self.client.active();
        let email = active.map(|a| a.email.clone());
        let hello_enabled = active.is_some_and(|a| a.hello_enabled);
        let pin_enabled = active.is_some_and(|a| a.pin_enabled);
        let item_count = self.client.overview.as_ref().map_or(0, |ov| {
            ov.items
                .iter()
                .filter(|i| !i.deleted && i.item_type == 1)
                .count()
        });
        json!({
            "desktopRunning": true,
            "enabled": self.settings.browser_integration,
            "session": session,
            "accountEmail": email,
            "helloEnabled": hello_enabled,
            "pinEnabled": pin_enabled,
            "biometricGate": self.settings.browser_biometric_gate,
            "itemCount": item_count,
        })
    }

    fn serialize_ok(id: u64, data: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": id,
            "ok": true,
            "data": data,
        }))
        .unwrap_or_default()
    }

    fn serialize_err(id: u64, code: &str, message: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": id,
            "ok": false,
            "error": code,
            "message": message,
        }))
        .unwrap_or_default()
    }

    fn serialize_event(event: &str, data: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": "event",
            "event": event,
            "data": data,
        }))
        .unwrap_or_default()
    }

    async fn dispatch_browser_request(
        weak: &gpui::WeakEntity<Self>,
        app: &mut gpui::AsyncWindowContext,
        req: WireRequest,
        responder: oneshot::Sender<Vec<u8>>,
    ) {
        let id = req.id;
        match req.kind.as_str() {
            "ping" | "get_status" => {
                let reply = weak
                    .update(app, |this, _| {
                        Self::serialize_ok(id, this.current_browser_status_json())
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "focus_desktop" => {
                let reply = weak
                    .update_in(app, |this, window, cx| {
                        crate::tray::restore_window_to_front(window);
                        window.activate_window();
                        match this.client.state() {
                            Session::Locked => this.open_unlock(window, cx),
                            Session::SignedOut => this.begin_sign_in(window, cx),
                            Session::Unlocked => cx.notify(),
                        }
                        Self::serialize_ok(id, this.current_browser_status_json())
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "lock_vault" => {
                let reply = weak
                    .update(app, |this, cx| {
                        this.request_lock(cx);
                        Self::serialize_ok(id, json!({ "locked": true }))
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "sync_vault" => {
                let reply = weak
                    .update(app, |this, cx| {
                        if this.client.state() != Session::Unlocked {
                            return Self::serialize_err(id, "locked", "请先解锁桌面端保险库");
                        }
                        this.request_sync(cx);
                        Self::serialize_ok(id, json!({ "syncing": true }))
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "unlock_hello" => {
                let prep = weak
                    .update_in(app, |this, window, cx| {
                        if this.client.state() == Session::Unlocked {
                            return Ok(None);
                        }
                        crate::tray::restore_window_to_front(window);
                        window.activate_window();
                        let Some(account) = this
                            .client
                            .active()
                            .filter(|a| a.session() == Session::Locked && a.hello_enabled)
                        else {
                            this.open_unlock(window, cx);
                            return Err((
                                "helloNotEnabled",
                                "当前账户未开启 Windows Hello，请在弹出的桌面端窗口输入主密码解锁",
                            ));
                        };
                        let hwnd = crate::tray::window_hwnd_usize(window).unwrap_or(0);
                        if hwnd == 0 {
                            return Err(("helloUnavailable", "无法获取桌面窗口句柄"));
                        }
                        Ok(Some((
                            account.id.clone(),
                            hwnd,
                            cx.background_executor().clone(),
                        )))
                    })
                    .unwrap_or(Err(("closed", "桌面端已关闭")));

                match prep {
                    Ok(None) => {
                        let status = weak
                            .update(app, |this, _| this.current_browser_status_json())
                            .unwrap_or(Value::Null);
                        let _ = responder.send(Self::serialize_ok(id, status));
                    }
                    Ok(Some((account_id, hwnd, bg))) => {
                        let token = Request::begin();
                        let unlocked = bg
                            .spawn(async move {
                                token.unlock_hello(&account_id, hwnd)?;
                                let accounts = desktop::accounts()?;
                                let overview = desktop::overview().ok();
                                Ok::<_, desktop::Error>((accounts, overview))
                            })
                            .await;
                        let reply = weak
                            .update(app, |this, cx| match unlocked {
                                Ok((accounts, overview)) => {
                                    this.close_form(cx);
                                    this.client.accounts = accounts;
                                    if let Some(ov) = overview {
                                        this.client.set_overview(ov);
                                    }
                                    this.invalidate_query();
                                    this.session_notice();
                                    this.broadcast_browser_state();
                                    cx.notify();
                                    Self::serialize_ok(id, this.current_browser_status_json())
                                }
                                Err(err) => {
                                    let msg = crate::client::status_text(&err);
                                    Self::serialize_err(id, err.code(), &msg)
                                }
                            })
                            .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                        let _ = responder.send(reply);
                    }
                    Err((code, msg)) => {
                        let _ = responder.send(Self::serialize_err(id, code, msg));
                    }
                }
            }
            "query_logins" => {
                let url = req
                    .payload
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let query = req
                    .payload
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                let limit = req
                    .payload
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map_or(30usize, |n| (n as usize).clamp(1, 100));

                let reply = weak
                    .update(app, |this, _| {
                        if !this.settings.browser_integration {
                            return Self::serialize_err(id, "disabled", "浏览器联动已关闭");
                        }
                        if this.client.state() != Session::Unlocked {
                            return Self::serialize_err(id, "locked", "保险库已锁定");
                        }
                        let Some(overview) = &this.client.overview else {
                            return Self::serialize_ok(
                                id,
                                json!({ "tabMatches": [], "results": [] }),
                            );
                        };
                        let tab_host = extract_url_host(&url);
                        let mut tab_matches: Vec<(u32, BridgeItemSummary)> = Vec::new();
                        let mut results: Vec<BridgeItemSummary> = Vec::new();

                        for item in &overview.items {
                            if item.deleted || item.restricted || item.item_type != 1 {
                                continue;
                            }
                            if let Some(th) = tab_host.as_deref()
                                && let Some(ih) = item.website_host.as_deref()
                            {
                                let score = domain_match_score(th, ih, item.favorite);
                                if score > 0 {
                                    tab_matches.push((score, BridgeItemSummary::from_item(item)));
                                }
                            }
                            if !query.is_empty() && results.len() < limit {
                                let matched = contains_ignore_case(&item.name, &query)
                                    || contains_ignore_case(&item.subtitle, &query)
                                    || item
                                        .website_host
                                        .as_deref()
                                        .is_some_and(|h| contains_ignore_case(h, &query));
                                if matched {
                                    results.push(BridgeItemSummary::from_item(item));
                                }
                            } else if query.is_empty()
                                && tab_host.is_none()
                                && results.len() < limit
                            {
                                results.push(BridgeItemSummary::from_item(item));
                            }
                        }
                        tab_matches.sort_by(|a, b| {
                            b.0.cmp(&a.0)
                                .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
                        });
                        let tab_matches: Vec<BridgeItemSummary> = tab_matches
                            .into_iter()
                            .take(limit)
                            .map(|(_, item)| item)
                            .collect();
                        Self::serialize_ok(
                            id,
                            json!({
                                "tabMatches": tab_matches,
                                "results": results,
                            }),
                        )
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "get_credentials" => {
                Self::handle_get_credentials(weak, app, id, &req.payload, responder).await;
            }
            "generate_password" => {
                let is_phrase = req
                    .payload
                    .get("passphrase")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let bg = weak
                    .update(app, |_, cx| cx.background_executor().clone())
                    .ok();
                let Some(bg) = bg else {
                    let _ = responder.send(Self::serialize_err(id, "closed", "桌面端已关闭"));
                    return;
                };
                let generated = if is_phrase {
                    let words = req
                        .payload
                        .get("words")
                        .and_then(Value::as_u64)
                        .unwrap_or(4)
                        .clamp(3, 20) as u32;
                    let sep = req
                        .payload
                        .get("separator")
                        .and_then(Value::as_str)
                        .unwrap_or("-")
                        .to_owned();
                    let cap = req
                        .payload
                        .get("capitalize")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let num = req
                        .payload
                        .get("includeNumber")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    bg.spawn(async move { desktop::passphrase(words, &sep, cap, num) })
                        .await
                } else {
                    let length = req
                        .payload
                        .get("length")
                        .and_then(Value::as_u64)
                        .unwrap_or(20)
                        .clamp(8, 128) as u32;
                    let upper = req
                        .payload
                        .get("uppercase")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let lower = req
                        .payload
                        .get("lowercase")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let digits = req
                        .payload
                        .get("numbers")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let symbols = req
                        .payload
                        .get("symbols")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let mut opts = 0u32;
                    if lower {
                        opts |= 1;
                    }
                    if upper {
                        opts |= 2;
                    }
                    if digits {
                        opts |= 4;
                    }
                    if symbols {
                        opts |= 8;
                    }
                    if opts == 0 {
                        opts = 1 | 2 | 4;
                    }
                    bg.spawn(async move {
                        desktop::password(length, opts, u32::from(digits), u32::from(symbols))
                    })
                    .await
                };
                match generated {
                    Ok(val) => {
                        let out = Self::serialize_ok(id, json!({ "value": val.as_str() }));
                        let _ = responder.send(out);
                    }
                    Err(err) => {
                        let msg = crate::client::status_text(&err);
                        let _ = responder.send(Self::serialize_err(id, err.code(), &msg));
                    }
                }
            }
            "create_login" => {
                Self::handle_create_login(weak, app, id, &req.payload, responder).await;
            }
            "unlock_pin" => {
                Self::handle_unlock_pin(weak, app, id, &req.payload, responder).await;
            }
            "check_save_candidate" => {
                let url = req
                    .payload
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let username = req
                    .payload
                    .get("username")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                let reply = weak
                    .update(app, |this, _| {
                        if !this.settings.browser_integration
                            || this.client.state() != Session::Unlocked
                        {
                            return Self::serialize_ok(id, json!({ "shouldPrompt": false }));
                        }
                        let Some(overview) = &this.client.overview else {
                            return Self::serialize_ok(id, json!({ "shouldPrompt": false }));
                        };
                        if overview.read_only {
                            return Self::serialize_ok(id, json!({ "shouldPrompt": false }));
                        }
                        let Some(tab_host) = extract_url_host(&url) else {
                            return Self::serialize_ok(id, json!({ "shouldPrompt": false }));
                        };
                        let already_saved = overview.items.iter().any(|item| {
                            !item.deleted
                                && !item.restricted
                                && item.item_type == 1
                                && item
                                    .website_host
                                    .as_deref()
                                    .is_some_and(|ih| domain_match_score(&tab_host, ih, false) > 0)
                                && (username.is_empty()
                                    || item.subtitle.trim().eq_ignore_ascii_case(&username))
                        });
                        Self::serialize_ok(
                            id,
                            json!({
                                "shouldPrompt": !already_saved,
                                "suggestedName": tab_host,
                            }),
                        )
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "query_autofill_profiles" => {
                let reply = weak
                    .update(app, |this, _| {
                        if !this.settings.browser_integration {
                            return Self::serialize_err(id, "disabled", "浏览器联动已关闭");
                        }
                        if this.client.state() != Session::Unlocked {
                            return Self::serialize_err(id, "locked", "保险库已锁定");
                        }
                        let Some(overview) = &this.client.overview else {
                            return Self::serialize_ok(
                                id,
                                json!({ "cards": [], "identities": [] }),
                            );
                        };
                        let mut cards = Vec::new();
                        let mut identities = Vec::new();
                        for item in &overview.items {
                            if item.deleted || item.restricted {
                                continue;
                            }
                            if item.item_type == 3 {
                                cards.push(json!({
                                    "id": item.id,
                                    "name": item.name,
                                    "subtitle": item.subtitle,
                                    "reprompt": item.reprompt,
                                }));
                            } else if item.item_type == 4 {
                                identities.push(json!({
                                    "id": item.id,
                                    "name": item.name,
                                    "subtitle": item.subtitle,
                                    "reprompt": item.reprompt,
                                }));
                            }
                        }
                        Self::serialize_ok(
                            id,
                            json!({
                                "cards": cards,
                                "identities": identities,
                            }),
                        )
                    })
                    .unwrap_or_else(|_| Self::serialize_err(id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            "get_autofill_profile" => {
                Self::handle_get_autofill_profile(weak, app, id, &req.payload, responder).await;
            }
            _ => {
                let _ = responder.send(Self::serialize_err(
                    id,
                    "unknownCommand",
                    "未知的浏览器联动指令",
                ));
            }
        }
    }

    async fn handle_get_credentials(
        weak: &gpui::WeakEntity<Self>,
        app: &mut gpui::AsyncWindowContext,
        req_id: u64,
        payload: &Value,
        responder: oneshot::Sender<Vec<u8>>,
    ) {
        let item_id = payload
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let tab_url = payload
            .get("tabUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let action = payload
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("fill")
            .to_owned();
        if item_id.is_empty() {
            let _ = responder.send(Self::serialize_err(req_id, "validation", "缺少条目 ID"));
            return;
        }

        enum AuthPlan {
            Direct {
                bg: gpui::BackgroundExecutor,
            },
            HelloThenDirect {
                account_id: String,
                hwnd: usize,
                bg: gpui::BackgroundExecutor,
            },
            Modal(oneshot::Receiver<Result<SecretPayload, String>>),
        }

        let plan = weak
            .update_in(app, |this, window, cx| {
                if !this.settings.browser_integration {
                    return Err(("disabled", "浏览器联动已关闭"));
                }
                if this.client.state() != Session::Unlocked {
                    return Err(("locked", "请先解锁保险库"));
                }
                let Some(overview) = &this.client.overview else {
                    return Err(("notSynced", "保险库尚未加载"));
                };
                let Some(item) = overview
                    .items
                    .iter()
                    .find(|i| i.id == item_id && !i.deleted && !i.restricted && i.item_type == 1)
                    .cloned()
                else {
                    return Err(("notFound", "找不到对应的登录条目"));
                };

                // Copying non-sensitive username never requires biometric/password reprompt.
                if action == "copy_username" {
                    return Ok((
                        item,
                        AuthPlan::Direct {
                            bg: cx.background_executor().clone(),
                        },
                    ));
                }

                let needs_reprompt = item.reprompt;
                let needs_gate = this.settings.browser_biometric_gate || needs_reprompt;
                if !needs_gate {
                    return Ok((
                        item,
                        AuthPlan::Direct {
                            bg: cx.background_executor().clone(),
                        },
                    ));
                }

                // If biometric gate is active (without master-password reprompt) and Windows Hello is enrolled,
                // verify hardware TPM Windows Hello signature directly!
                if !needs_reprompt
                    && let Some(acc) = this.client.active().filter(|a| a.hello_enabled)
                    && let Some(hwnd) = crate::tray::window_hwnd_usize(window)
                {
                    crate::tray::restore_window_to_front(window);
                    return Ok((
                        item,
                        AuthPlan::HelloThenDirect {
                            account_id: acc.id.clone(),
                            hwnd,
                            bg: cx.background_executor().clone(),
                        },
                    ));
                }

                // Otherwise present the in-app approval / master-password reprompt modal.
                crate::tray::restore_window_to_front(window);
                window.activate_window();
                let (tx, rx) = oneshot::channel();
                let password_input =
                    cx.new(|cx| Input::new("输入主密码以授权浏览器读取", true, cx));
                if needs_reprompt {
                    password_input.read(cx).focus(window);
                }
                let origin_host = extract_url_host(&tab_url).unwrap_or_else(|| "当前网页".into());
                let action_label = match action.as_str() {
                    "copy_password" => "复制密码",
                    "copy_totp" => "复制动态验证码",
                    _ => "自动填充登录凭据",
                };
                this.browser_bridge.pending_approval = Some(PendingBrowserApproval {
                    item_id: item.id.clone(),
                    item_name: item.name.clone(),
                    username: item.subtitle.clone(),
                    origin_host,
                    action_label,
                    needs_password: needs_reprompt,
                    password_input,
                    error: None,
                    busy: false,
                    reply: Some(tx),
                });
                cx.notify();
                Ok((item, AuthPlan::Modal(rx)))
            })
            .unwrap_or(Err(("closed", "桌面端已关闭")));

        let (item, plan) = match plan {
            Ok(v) => v,
            Err((code, msg)) => {
                let _ = responder.send(Self::serialize_err(req_id, code, msg));
                return;
            }
        };

        let secret_res: Result<SecretPayload, String> = match plan {
            AuthPlan::Direct { bg } => {
                if action == "copy_username" {
                    Ok(SecretPayload {
                        username: Zeroizing::new(item.subtitle.clone()),
                        password: Zeroizing::default(),
                        totp: None,
                        totp_remaining: None,
                        profile_fields: None,
                    })
                } else {
                    let id_clone = item.id.clone();
                    bg.spawn(async move { decrypt_secret_payload(&id_clone, "") })
                        .await
                }
            }
            AuthPlan::HelloThenDirect {
                account_id,
                hwnd,
                bg,
            } => {
                let id_clone = item.id.clone();
                bg.spawn(async move {
                    let req = Request::begin();
                    req.verify_hello(&account_id, hwnd)
                        .map_err(|e| crate::client::status_text(&e))?;
                    decrypt_secret_payload(&id_clone, "")
                })
                .await
            }
            AuthPlan::Modal(rx) => match rx.await {
                Ok(res) => res,
                Err(_) => Err("已取消授权".into()),
            },
        };

        match secret_res {
            Ok(secret) => {
                let mut body = serde_json::Map::new();
                body.insert("id".into(), Value::String(item.id));
                body.insert("name".into(), Value::String(item.name));
                body.insert(
                    "username".into(),
                    Value::String(secret.username.as_str().to_owned()),
                );
                if matches!(action.as_str(), "fill" | "copy_password") {
                    body.insert(
                        "password".into(),
                        Value::String(secret.password.as_str().to_owned()),
                    );
                }
                if matches!(action.as_str(), "fill" | "copy_totp")
                    && let Some(code) = &secret.totp
                {
                    body.insert("totp".into(), Value::String(code.as_str().to_owned()));
                    if let Some(rem) = secret.totp_remaining {
                        body.insert("totpRemaining".into(), Value::Number(rem.into()));
                    }
                }
                let mut frame = Self::serialize_ok(req_id, Value::Object(body));
                let _ = responder.send(std::mem::take(&mut frame));
            }
            Err(message) => {
                let _ = responder.send(Self::serialize_err(req_id, "denied", &message));
            }
        }
    }

    async fn handle_create_login(
        weak: &gpui::WeakEntity<Self>,
        app: &mut gpui::AsyncWindowContext,
        req_id: u64,
        payload: &Value,
        responder: oneshot::Sender<Vec<u8>>,
    ) {
        let mut name = payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let username = Zeroizing::new(
            payload
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        );
        let password = Zeroizing::new(
            payload
                .get("password")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
        let uri = payload
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let totp = Zeroizing::new(
            payload
                .get("totp")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        );

        if name.is_empty() {
            name = extract_url_host(&uri).unwrap_or_else(|| "新建登录项".into());
        }

        let prep = weak
            .update(app, |this, cx| {
                if !this.settings.browser_integration {
                    return Err(("disabled", "浏览器联动已关闭"));
                }
                if this.client.state() != Session::Unlocked {
                    return Err(("locked", "请先解锁桌面端保险库"));
                }
                if this.client.overview.as_ref().is_none_or(|ov| ov.read_only) {
                    return Err(("readOnly", "当前离线只读快照不可写入，请先在桌面端同步"));
                }
                Ok(cx.background_executor().clone())
            })
            .unwrap_or(Err(("closed", "桌面端已关闭")));

        let bg = match prep {
            Ok(bg) => bg,
            Err((code, msg)) => {
                let _ = responder.send(Self::serialize_err(req_id, code, msg));
                return;
            }
        };

        let saved = bg
            .spawn(async move {
                let mut draft = Draft::default();
                draft.item_type = 1;
                draft.name = name;
                if !username.is_empty() {
                    draft
                        .values
                        .insert("username".into(), username.as_str().to_owned());
                }
                if !password.is_empty() {
                    draft
                        .values
                        .insert("password".into(), password.as_str().to_owned());
                }
                if !totp.is_empty() {
                    draft.values.insert("totp".into(), totp.as_str().to_owned());
                }
                if !uri.is_empty() {
                    draft.uris.push(DraftUri {
                        uri: Some(uri),
                        r#match: None,
                    });
                }
                Request::begin().save_item(&draft, "")
            })
            .await;

        let reply = weak
            .update(app, |this, cx| match saved {
                Ok(mutation) => {
                    let created_id = mutation.id.clone();
                    this.accept_synced_overview(mutation.overview);
                    this.broadcast_browser_state();
                    this.ui.toast.show(
                        ui::toast::ToastKind::Success,
                        "已保存来自 Firefox 扩展的新登录项",
                        std::time::Instant::now(),
                        this.animate,
                    );
                    cx.notify();
                    Self::serialize_ok(req_id, json!({ "id": created_id }))
                }
                Err(err) => {
                    let msg = crate::client::status_text(&err);
                    Self::serialize_err(req_id, err.code(), &msg)
                }
            })
            .unwrap_or_else(|_| Self::serialize_err(req_id, "closed", "桌面端已关闭"));
        let _ = responder.send(reply);
    }

    pub(crate) fn cancel_browser_approval(&mut self, cx: &mut Context<Self>) {
        if let Some(mut modal) = self.browser_bridge.pending_approval.take() {
            modal
                .password_input
                .update(cx, |input, cx| input.set("", cx));
            if let Some(tx) = modal.reply.take() {
                let _ = tx.send(Err("已在桌面端拒绝填充授权".into()));
            }
            cx.notify();
        }
    }

    pub(crate) fn approve_browser_request(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.browser_bridge.pending_approval.as_mut() else {
            return;
        };
        if modal.busy {
            return;
        }
        let password = if modal.needs_password {
            let val = Zeroizing::new(modal.password_input.read(cx).value().to_owned());
            if val.is_empty() {
                modal.error = Some("请输入主密码以继续验证".into());
                cx.notify();
                return;
            }
            val
        } else {
            Zeroizing::default()
        };
        modal.busy = true;
        modal.error = None;
        let item_id = modal.item_id.clone();
        let bg = cx.background_executor().clone();
        cx.notify();

        cx.spawn(async move |weak, app| {
            let res = bg
                .spawn(async move { decrypt_secret_payload(&item_id, &password) })
                .await;
            let _ = weak.update(app, |this, cx| {
                let Some(modal) = this.browser_bridge.pending_approval.as_mut() else {
                    return;
                };
                modal.busy = false;
                match res {
                    Ok(secret) => {
                        if let Some(mut finished) = this.browser_bridge.pending_approval.take() {
                            finished
                                .password_input
                                .update(cx, |input, cx| input.set("", cx));
                            if let Some(tx) = finished.reply.take() {
                                let _ = tx.send(Ok(secret));
                            }
                        }
                    }
                    Err(err) => {
                        modal.error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_browser_approval_modal(&self, cx: &mut Context<Self>) -> Div {
        let pal = theme::palette();
        let Some(modal) = &self.browser_bridge.pending_approval else {
            return div();
        };
        div()
            .w(px(410.))
            .max_w(px(410.))
            .p(px(22.))
            .rounded(px(ui::tokens::radius::OVERLAY))
            .bg(theme::hex(pal.card))
            .border_1()
            .border_color(theme::hex(pal.border))
            .shadow(ui::components::overlay_shadow())
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(36.))
                            .h(px(36.))
                            .rounded(px(10.))
                            .bg(pal.tint(pal.accent_fill))
                            .border_1()
                            .border_color(pal.tint_border(pal.accent_fill))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::hex(pal.accent_text))
                            .child(ui::components::glyph_icon(ui::tokens::glyph::SHIELD, 17.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(ui::tokens::font::SUBTITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::hex(pal.text))
                                    .child(format!("Firefox 请求{}", modal.action_label)),
                            )
                            .child(
                                div()
                                    .text_size(px(ui::tokens::font::CAPTION))
                                    .text_color(theme::hex(pal.text_muted))
                                    .child(format!("目标站点：{}", modal.origin_host)),
                            ),
                    ),
            )
            .child(
                div()
                    .p(px(12.))
                    .rounded(px(ui::tokens::radius::CONTROL))
                    .bg(theme::hex(pal.layer))
                    .border_1()
                    .border_color(theme::hex(pal.border_subtle))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(ui::tokens::font::BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::hex(pal.text))
                            .child(modal.item_name.clone()),
                    )
                    .when(!modal.username.is_empty(), |card| {
                        card.child(
                            div()
                                .text_size(px(ui::tokens::font::CAPTION))
                                .text_color(theme::hex(pal.text_secondary))
                                .child(modal.username.clone()),
                        )
                    }),
            )
            .when(modal.needs_password, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(ui::tokens::font::CAPTION))
                                .text_color(theme::hex(pal.text_secondary))
                                .child("此条目启用了主密码二次确认："),
                        )
                        .child(modal.password_input.clone()),
                )
            })
            .when_some(modal.error.as_ref(), |el, err| {
                el.child(ui::components::info_bar(
                    ui::toast::ToastKind::Error,
                    err.clone(),
                ))
            })
            .child(
                div()
                    .pt(px(2.))
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        ui::components::btn(
                            "browser-approve-cancel",
                            ui::components::BtnVariant::Secondary,
                        )
                        .h(px(34.))
                        .px(px(14.))
                        .child("拒绝")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_browser_approval(cx);
                        })),
                    )
                    .child(
                        ui::components::btn(
                            "browser-approve-ok",
                            ui::components::BtnVariant::Primary,
                        )
                        .h(px(34.))
                        .px(px(16.))
                        .when(modal.busy, |b| b.opacity(0.5))
                        .child(if modal.busy {
                            "验证中…"
                        } else {
                            "允许并发送"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.approve_browser_request(cx);
                        })),
                    ),
            )
    }
}

fn decrypt_secret_payload(item_id: &str, master_password: &str) -> Result<SecretPayload, String> {
    let req = Request::begin();
    let mut detail = req
        .detail(item_id, master_password)
        .map_err(|e| crate::client::status_text(&e))?;
    let mut username = Zeroizing::new(String::new());
    let mut password = Zeroizing::new(String::new());
    let mut profile_map = serde_json::Map::new();
    for field in &mut detail.fields {
        if detail.item_type == 1 {
            if field.label == "用户名" && username.is_empty() {
                *username = std::mem::take(&mut field.value);
            } else if field.label == "密码" && password.is_empty() {
                *password = std::mem::take(&mut field.value);
            } else {
                field.value.zeroize();
            }
        } else {
            let key = match (detail.item_type, field.label.as_str()) {
                (3, "持卡人") => Some("cardholderName"),
                (3, "卡组织") => Some("brand"),
                (3, "卡号") => Some("number"),
                (3, "到期月份") => Some("expMonth"),
                (3, "到期年份") => Some("expYear"),
                (3, "安全码") => Some("code"),
                (4, "称谓") => Some("title"),
                (4, "名") => Some("firstName"),
                (4, "中间名") => Some("middleName"),
                (4, "姓") => Some("lastName"),
                (4, "用户名") => Some("username"),
                (4, "公司") => Some("company"),
                (4, "邮箱") => Some("email"),
                (4, "电话") => Some("phone"),
                (4, "地址 1") => Some("address1"),
                (4, "地址 2") => Some("address2"),
                (4, "城市") => Some("city"),
                (4, "省 / 州") => Some("state"),
                (4, "邮编") => Some("postalCode"),
                (4, "国家") => Some("country"),
                _ => None,
            };
            if let Some(k) = key {
                profile_map.insert(k.into(), Value::String(field.value.clone()));
            }
            field.value.zeroize();
        }
    }
    let now_unix = chrono::Utc::now().timestamp();
    let (totp, totp_remaining) = match &detail.totp {
        Some(Totp::Ready { value }) => (
            Some(Zeroizing::new(value.code.clone())),
            Some((value.valid_until - now_unix).max(0)),
        ),
        _ => (None, None),
    };
    let item_type = detail.item_type;
    detail.revoke();
    Ok(SecretPayload {
        username,
        password,
        totp,
        totp_remaining,
        profile_fields: (item_type != 1).then_some(profile_map),
    })
}

impl VaultPreview {
    async fn handle_unlock_pin(
        weak: &gpui::WeakEntity<Self>,
        app: &mut gpui::AsyncWindowContext,
        req_id: u64,
        payload: &Value,
        responder: oneshot::Sender<Vec<u8>>,
    ) {
        let pin = Zeroizing::new(
            payload
                .get("pin")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        );
        if pin.is_empty() {
            let _ = responder.send(Self::serialize_err(req_id, "wrongPin", "请输入 PIN 码"));
            return;
        }
        let prep = weak
            .update(app, |this, cx| {
                if this.client.state() == Session::Unlocked {
                    return Ok(None);
                }
                let Some(acc) = this
                    .client
                    .active()
                    .filter(|a| a.session() == Session::Locked && a.pin_enabled)
                else {
                    return Err(("pinNotEnabled", "当前账户未开启 PIN 码解锁"));
                };
                Ok(Some((acc.id.clone(), cx.background_executor().clone())))
            })
            .unwrap_or(Err(("closed", "桌面端已关闭")));

        match prep {
            Ok(None) => {
                let status = weak
                    .update(app, |this, _| this.current_browser_status_json())
                    .unwrap_or(Value::Null);
                let _ = responder.send(Self::serialize_ok(req_id, status));
            }
            Ok(Some((account_id, bg))) => {
                let token = Request::begin();
                let unlocked = bg
                    .spawn(async move {
                        match token.unlock_pin(&account_id, &pin) {
                            Ok(()) => {
                                let accounts = desktop::accounts().map_err(|e| (e, None))?;
                                let overview = desktop::overview().ok();
                                Ok((accounts, overview))
                            }
                            Err(err) => {
                                let refreshed = (err.code() == "pinRevoked")
                                    .then(|| desktop::accounts().ok())
                                    .flatten();
                                Err((err, refreshed))
                            }
                        }
                    })
                    .await;
                let reply = weak
                    .update(app, |this, cx| match unlocked {
                        Ok((accounts, overview)) => {
                            this.close_form(cx);
                            this.client.accounts = accounts;
                            if let Some(ov) = overview {
                                this.client.set_overview(ov);
                            }
                            this.invalidate_query();
                            this.session_notice();
                            this.broadcast_browser_state();
                            cx.notify();
                            Self::serialize_ok(req_id, this.current_browser_status_json())
                        }
                        Err((err, refreshed)) => {
                            if let Some(accounts) = refreshed {
                                this.client.accounts = accounts;
                                this.client.unlock_with_pin = false;
                                this.broadcast_browser_state();
                                cx.notify();
                            }
                            let msg = crate::client::status_text(&err);
                            Self::serialize_err(req_id, err.code(), &msg)
                        }
                    })
                    .unwrap_or_else(|_| Self::serialize_err(req_id, "closed", "桌面端已关闭"));
                let _ = responder.send(reply);
            }
            Err((code, msg)) => {
                let _ = responder.send(Self::serialize_err(req_id, code, msg));
            }
        }
    }

    async fn handle_get_autofill_profile(
        weak: &gpui::WeakEntity<Self>,
        app: &mut gpui::AsyncWindowContext,
        req_id: u64,
        payload: &Value,
        responder: oneshot::Sender<Vec<u8>>,
    ) {
        let item_id = payload
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let tab_url = payload
            .get("tabUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if item_id.is_empty() {
            let _ = responder.send(Self::serialize_err(req_id, "validation", "缺少条目 ID"));
            return;
        }

        enum ProfilePlan {
            Direct(gpui::BackgroundExecutor),
            Hello {
                account_id: String,
                hwnd: usize,
                bg: gpui::BackgroundExecutor,
            },
            Modal(oneshot::Receiver<Result<SecretPayload, String>>),
        }

        let plan = weak
            .update_in(app, |this, window, cx| {
                if !this.settings.browser_integration {
                    return Err(("disabled", "浏览器联动已关闭"));
                }
                if this.client.state() != Session::Unlocked {
                    return Err(("locked", "保险库已锁定"));
                }
                let Some(overview) = &this.client.overview else {
                    return Err(("notSynced", "保险库尚未加载"));
                };
                let Some(item) = overview
                    .items
                    .iter()
                    .find(|i| {
                        i.id == item_id
                            && !i.deleted
                            && !i.restricted
                            && matches!(i.item_type, 3 | 4)
                    })
                    .cloned()
                else {
                    return Err(("notFound", "找不到对应的填表条目"));
                };

                let needs_reprompt = item.reprompt;
                let needs_gate = this.settings.browser_biometric_gate || needs_reprompt;
                if !needs_gate {
                    return Ok((item, ProfilePlan::Direct(cx.background_executor().clone())));
                }
                if !needs_reprompt
                    && let Some(acc) = this.client.active().filter(|a| a.hello_enabled)
                    && let Some(hwnd) = crate::tray::window_hwnd_usize(window)
                {
                    crate::tray::restore_window_to_front(window);
                    return Ok((
                        item,
                        ProfilePlan::Hello {
                            account_id: acc.id.clone(),
                            hwnd,
                            bg: cx.background_executor().clone(),
                        },
                    ));
                }

                crate::tray::restore_window_to_front(window);
                window.activate_window();
                let (tx, rx) = oneshot::channel();
                let password_input = cx.new(|cx| Input::new("输入主密码以授权填表", true, cx));
                if needs_reprompt {
                    password_input.read(cx).focus(window);
                }
                let origin_host = extract_url_host(&tab_url).unwrap_or_else(|| "当前网页".into());
                this.browser_bridge.pending_approval = Some(PendingBrowserApproval {
                    item_id: item.id.clone(),
                    item_name: item.name.clone(),
                    username: item.subtitle.clone(),
                    origin_host,
                    action_label: if item.item_type == 3 {
                        "填充银行卡信息"
                    } else {
                        "填充身份信息"
                    },
                    needs_password: needs_reprompt,
                    password_input,
                    error: None,
                    busy: false,
                    reply: Some(tx),
                });
                cx.notify();
                Ok((item, ProfilePlan::Modal(rx)))
            })
            .unwrap_or(Err(("closed", "桌面端已关闭")));

        let (item, plan) = match plan {
            Ok(v) => v,
            Err((code, msg)) => {
                let _ = responder.send(Self::serialize_err(req_id, code, msg));
                return;
            }
        };

        let res = match plan {
            ProfilePlan::Direct(bg) => {
                let id_clone = item.id.clone();
                bg.spawn(async move { decrypt_secret_payload(&id_clone, "") })
                    .await
            }
            ProfilePlan::Hello {
                account_id,
                hwnd,
                bg,
            } => {
                let id_clone = item.id.clone();
                bg.spawn(async move {
                    Request::begin()
                        .verify_hello(&account_id, hwnd)
                        .map_err(|e| crate::client::status_text(&e))?;
                    decrypt_secret_payload(&id_clone, "")
                })
                .await
            }
            ProfilePlan::Modal(rx) => match rx.await {
                Ok(r) => r,
                Err(_) => Err("已取消授权".into()),
            },
        };

        match res {
            Ok(secret) => {
                let kind = if item.item_type == 3 {
                    "card"
                } else {
                    "identity"
                };
                let fields = Value::Object(secret.profile_fields.unwrap_or_default());
                let out = Self::serialize_ok(
                    req_id,
                    json!({
                        "id": item.id,
                        "kind": kind,
                        "fields": fields,
                    }),
                );
                let _ = responder.send(out);
            }
            Err(msg) => {
                let _ = responder.send(Self::serialize_err(req_id, "denied", &msg));
            }
        }
    }
}

#[cfg(windows)]
mod win_pipe {
    use super::*;
    use std::{
        ffi::c_void,
        ptr::{null, null_mut},
        sync::atomic::AtomicUsize,
    };

    type Handle = *mut c_void;
    const INVALID_HANDLE_VALUE: Handle = (-1isize) as Handle;
    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_PIPE_BUSY: u32 = 231;
    const WAIT_OBJECT_0: u32 = 0;

    const HKEY_CURRENT_USER: isize = -2147483647i32 as isize;
    const KEY_READ: u32 = 0x20019;
    const KEY_WRITE: u32 = 0x20006;
    const REG_SZ: u32 = 1;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: Handle,
    }

    impl Overlapped {
        fn new(event: Handle) -> Self {
            Self {
                internal: 0,
                internal_high: 0,
                offset: 0,
                offset_high: 0,
                event,
            }
        }
    }

    struct OwnedHandle(Handle);
    unsafe impl Send for OwnedHandle {}
    unsafe impl Sync for OwnedHandle {}
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    fn wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn overlapped_read_exact(
        pipe: Handle,
        event: Handle,
        mut buf: &mut [u8],
    ) -> io::Result<()> {
        while !buf.is_empty() {
            unsafe {
                let _ = ResetEvent(event);
            }
            let mut ov = Overlapped::new(event);
            let mut got = 0u32;
            let ok = unsafe {
                ReadFile(
                    pipe,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len() as u32,
                    &mut got,
                    &mut ov,
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                if err != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(err as i32));
                }
                if unsafe { GetOverlappedResult(pipe, &mut ov, &mut got, 1) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if got == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed"));
            }
            buf = &mut buf[got as usize..];
        }
        Ok(())
    }

    unsafe fn overlapped_write_all(pipe: Handle, event: Handle, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            unsafe {
                let _ = ResetEvent(event);
            }
            let mut ov = Overlapped::new(event);
            let mut sent = 0u32;
            let ok = unsafe {
                WriteFile(
                    pipe,
                    buf.as_ptr() as *const c_void,
                    buf.len() as u32,
                    &mut sent,
                    &mut ov,
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                if err != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(err as i32));
                }
                if unsafe { GetOverlappedResult(pipe, &mut ov, &mut sent, 1) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if sent == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "pipe write zero"));
            }
            buf = &buf[sent as usize..];
        }
        Ok(())
    }

    unsafe fn read_pipe_frame(pipe: Handle, event: Handle) -> io::Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        unsafe { overlapped_read_exact(pipe, event, &mut len_buf)? };
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad pipe frame"));
        }
        let mut body = vec![0u8; len];
        unsafe { overlapped_read_exact(pipe, event, &mut body)? };
        Ok(body)
    }

    unsafe fn write_pipe_frame(pipe: Handle, event: Handle, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bad pipe frame",
            ));
        }
        let len = (payload.len() as u32).to_le_bytes();
        unsafe {
            overlapped_write_all(pipe, event, &len)?;
            overlapped_write_all(pipe, event, payload)
        }
    }

    struct PipeClientWriter {
        pipe: Arc<OwnedHandle>,
        write_lock: Mutex<OwnedHandle>,
    }

    impl PipeClientWriter {
        fn send(&self, payload: &[u8]) -> bool {
            let Ok(event) = self.write_lock.lock() else {
                return false;
            };
            unsafe { write_pipe_frame(self.pipe.0, event.0, payload).is_ok() }
        }
    }

    pub struct BridgeServer {
        stop_flag: Arc<AtomicBool>,
        stop_event: Arc<OwnedHandle>,
        clients: Arc<Mutex<Vec<Arc<PipeClientWriter>>>>,
        client_count: Arc<AtomicUsize>,
    }

    impl BridgeServer {
        pub fn start(
            pipe_name: &str,
            inbound: mpsc::UnboundedSender<InboundBridgeEvent>,
        ) -> Result<Self, String> {
            let stop_event_raw = unsafe { CreateEventW(null(), 1, 0, null()) };
            if stop_event_raw.is_null() {
                return Err("CreateEventW failed".into());
            }
            let stop_event = Arc::new(OwnedHandle(stop_event_raw));
            let stop_flag = Arc::new(AtomicBool::new(false));
            let clients: Arc<Mutex<Vec<Arc<PipeClientWriter>>>> = Arc::new(Mutex::new(Vec::new()));
            let client_count = Arc::new(AtomicUsize::new(0));

            let pipe_wide = wide_null(pipe_name);
            let stop_flag_bg = stop_flag.clone();
            let stop_event_bg = stop_event.clone();
            let clients_bg = clients.clone();
            let count_bg = client_count.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<()>(1);

            std::thread::Builder::new()
                .name("bw-browser-bridge".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let mut ready_signal = Some(ready_tx);
                    let mut own_session = 0u32;
                    unsafe {
                        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session);
                    }
                    while !stop_flag_bg.load(Ordering::Acquire) {
                        let pipe_raw = unsafe {
                            CreateNamedPipeW(
                                pipe_wide.as_ptr(),
                                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                                PIPE_TYPE_BYTE
                                    | PIPE_READMODE_BYTE
                                    | PIPE_WAIT
                                    | PIPE_REJECT_REMOTE_CLIENTS,
                                PIPE_UNLIMITED_INSTANCES,
                                64 * 1024,
                                64 * 1024,
                                0,
                                null(),
                            )
                        };
                        if pipe_raw.is_null() || pipe_raw == INVALID_HANDLE_VALUE {
                            if let Some(tx) = ready_signal.take() {
                                let _ = tx.send(());
                            }
                            std::thread::sleep(Duration::from_millis(200));
                            continue;
                        }
                        if let Some(tx) = ready_signal.take() {
                            let _ = tx.send(());
                        }
                        let pipe = Arc::new(OwnedHandle(pipe_raw));
                        let connect_ev = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
                        if connect_ev.0.is_null() {
                            break;
                        }
                        let mut ov = Overlapped::new(connect_ev.0);
                        let connected = unsafe { ConnectNamedPipe(pipe.0, &mut ov) };
                        if connected == 0 {
                            let err = unsafe { GetLastError() };
                            if err == ERROR_IO_PENDING {
                                let handles = [connect_ev.0, stop_event_bg.0];
                                let waited = unsafe {
                                    WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX)
                                };
                                if waited != WAIT_OBJECT_0 || stop_flag_bg.load(Ordering::Acquire) {
                                    unsafe {
                                        let _ = CancelIoEx(pipe.0, &ov);
                                    }
                                    break;
                                }
                                let mut dummy = 0u32;
                                if unsafe { GetOverlappedResult(pipe.0, &mut ov, &mut dummy, 0) }
                                    == 0
                                {
                                    continue;
                                }
                            } else if err != ERROR_PIPE_CONNECTED {
                                continue;
                            }
                        }

                        // Verify client belongs to the exact same desktop Windows session.
                        let mut client_pid = 0u32;
                        let mut client_session = u32::MAX;
                        let same_session = unsafe {
                            GetNamedPipeClientProcessId(pipe.0, &mut client_pid) != 0
                                && ProcessIdToSessionId(client_pid, &mut client_session) != 0
                                && client_session == own_session
                        };
                        if !same_session {
                            unsafe {
                                let _ = DisconnectNamedPipe(pipe.0);
                            }
                            continue;
                        }

                        let write_ev = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
                        if write_ev.0.is_null() {
                            continue;
                        }
                        let writer = Arc::new(PipeClientWriter {
                            pipe: pipe.clone(),
                            write_lock: Mutex::new(write_ev),
                        });
                        if let Ok(mut list) = clients_bg.lock() {
                            list.push(writer.clone());
                            let len = list.len();
                            count_bg.store(len, Ordering::Release);
                            let _ = inbound.unbounded_send(InboundBridgeEvent::ClientCount(len));
                        }

                        let inbound_client = inbound.clone();
                        let clients_cleanup = clients_bg.clone();
                        let count_cleanup = count_bg.clone();
                        let stop_client = stop_flag_bg.clone();
                        std::thread::Builder::new()
                            .name("bw-browser-client".into())
                            .stack_size(256 * 1024)
                            .spawn(move || {
                                let read_ev =
                                    OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
                                if !read_ev.0.is_null() {
                                    while !stop_client.load(Ordering::Acquire) {
                                        let Ok(frame) =
                                            (unsafe { read_pipe_frame(pipe.0, read_ev.0) })
                                        else {
                                            break;
                                        };
                                        let Ok(req) = serde_json::from_slice::<WireRequest>(&frame)
                                        else {
                                            continue;
                                        };
                                        let (tx, rx) = oneshot::channel::<Vec<u8>>();
                                        if inbound_client
                                            .unbounded_send(InboundBridgeEvent::Request {
                                                request: req,
                                                responder: tx,
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                        if let Ok(reply) = futures::executor::block_on(rx)
                                            && !writer.send(&reply)
                                        {
                                            break;
                                        }
                                    }
                                }
                                unsafe {
                                    let _ = DisconnectNamedPipe(pipe.0);
                                }
                                if let Ok(mut list) = clients_cleanup.lock() {
                                    list.retain(|item| !Arc::ptr_eq(item, &writer));
                                    let len = list.len();
                                    count_cleanup.store(len, Ordering::Release);
                                    let _ = inbound_client
                                        .unbounded_send(InboundBridgeEvent::ClientCount(len));
                                }
                            })
                            .ok();
                    }
                })
                .map_err(|e| e.to_string())?;

            let _ = ready_rx.recv_timeout(Duration::from_secs(2));
            Ok(Self {
                stop_flag,
                stop_event,
                clients,
                client_count,
            })
        }

        pub fn client_count(&self) -> usize {
            self.client_count.load(Ordering::Acquire)
        }

        pub fn broadcast(&self, payload: &[u8]) {
            let snapshot: Vec<Arc<PipeClientWriter>> = self
                .clients
                .lock()
                .map(|list| list.clone())
                .unwrap_or_default();
            for client in snapshot {
                let _ = client.send(payload);
            }
        }

        pub fn stop(&self) {
            self.stop_flag.store(true, Ordering::Release);
            unsafe {
                let _ = SetEvent(self.stop_event.0);
            }
            if let Ok(mut list) = self.clients.lock() {
                for client in list.drain(..) {
                    unsafe {
                        let _ = CancelIoEx(client.pipe.0, null());
                    }
                }
            }
            self.client_count.store(0, Ordering::Release);
        }
    }

    impl Drop for BridgeServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn connect_pipe_once(pipe_wide: &[u16]) -> Option<Arc<OwnedHandle>> {
        for _ in 0..3 {
            let h = unsafe {
                CreateFileW(
                    pipe_wide.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    null_mut(),
                )
            };
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                return Some(Arc::new(OwnedHandle(h)));
            }
            if unsafe { GetLastError() } == ERROR_PIPE_BUSY {
                unsafe {
                    let _ = WaitNamedPipeW(pipe_wide.as_ptr(), 500);
                }
            } else {
                break;
            }
        }
        None
    }

    fn spawn_desktop_app() {
        // Launch MSIX packaged Bytewarden if registered; fallback to sibling Bytewarden-GPUI-Preview.exe.
        let launched_msix = std::process::Command::new("explorer.exe")
            .arg("shell:AppsFolder\\ZHT0711.BytewardenGpui_d8kzwpfsmvz3y!App")
            .spawn()
            .is_ok();
        if !launched_msix
            && let Ok(cur) = std::env::current_exe()
            && let Some(dir) = cur.parent()
        {
            let sibling = dir.join("Bytewarden-GPUI-Preview.exe");
            if sibling.is_file() {
                let _ = std::process::Command::new(sibling).spawn();
            }
        }
    }

    /// Ultra-lightweight stdio <-> Named Pipe relay for Firefox Native Messaging.
    pub fn run_stdio_relay(pipe_name: &str) -> io::Result<()> {
        let pipe_wide = wide_null(pipe_name);
        let stdout_mutex = Arc::new(Mutex::new(io::stdout()));

        let try_attach =
            |stdout_mutex: &Arc<Mutex<io::Stdout>>| -> Option<(Arc<OwnedHandle>, OwnedHandle)> {
                let pipe = connect_pipe_once(&pipe_wide)?;
                let write_ev = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
                if write_ev.0.is_null() {
                    return None;
                }
                let pipe_reader = pipe.clone();
                let stdout_reader = stdout_mutex.clone();
                std::thread::Builder::new()
                    .name("bw-relay-reader".into())
                    .stack_size(192 * 1024)
                    .spawn(move || {
                        let read_ev = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
                        if read_ev.0.is_null() {
                            return;
                        }
                        while let Ok(frame) = unsafe { read_pipe_frame(pipe_reader.0, read_ev.0) } {
                            let Ok(mut out) = stdout_reader.lock() else {
                                break;
                            };
                            if write_native_frame(&mut *out, &frame).is_err() {
                                break;
                            }
                        }
                        // Notify browser immediately if Desktop exited while Firefox stayed open.
                        if let Ok(mut out) = stdout_reader.lock() {
                            let offline = serde_json::to_vec(&json!({
                                "type": "event",
                                "event": "state_changed",
                                "data": {
                                    "desktopRunning": false,
                                    "enabled": true,
                                    "session": "offline",
                                    "accountEmail": Value::Null,
                                    "helloEnabled": false,
                                    "biometricGate": false,
                                    "itemCount": 0,
                                }
                            }))
                            .unwrap_or_default();
                            let _ = write_native_frame(&mut *out, &offline);
                        }
                    })
                    .ok()?;
                Some((pipe, write_ev))
            };

        let mut active_pipe = try_attach(&stdout_mutex);

        let mut stdin = io::stdin().lock();
        while let Some(frame) = read_native_frame(&mut stdin)? {
            if let Some((pipe, write_ev)) = &active_pipe
                && unsafe { write_pipe_frame(pipe.0, write_ev.0, &frame) }.is_ok()
            {
                continue;
            }
            // Try immediate reconnect in case Desktop was started separately.
            active_pipe = try_attach(&stdout_mutex);
            if let Some((pipe, write_ev)) = &active_pipe
                && unsafe { write_pipe_frame(pipe.0, write_ev.0, &frame) }.is_ok()
            {
                continue;
            }

            // Desktop is offline: answer `get_status` / `launch_desktop` directly from the relay!
            let parsed = serde_json::from_slice::<WireRequest>(&frame).ok();
            let req_id = parsed.as_ref().map_or(0, |r| r.id);
            let kind = parsed.as_ref().map_or("", |r| r.kind.as_str());
            if matches!(kind, "launch_desktop" | "focus_desktop") {
                spawn_desktop_app();
                for _ in 0..25 {
                    std::thread::sleep(Duration::from_millis(200));
                    active_pipe = try_attach(&stdout_mutex);
                    if active_pipe.is_some() {
                        break;
                    }
                }
                if let Some((pipe, write_ev)) = &active_pipe {
                    let focus_req = serde_json::to_vec(&json!({
                        "id": req_id,
                        "type": "focus_desktop",
                        "payload": {}
                    }))
                    .unwrap_or_default();
                    if unsafe { write_pipe_frame(pipe.0, write_ev.0, &focus_req) }.is_ok() {
                        continue;
                    }
                }
            }

            let offline_reply = if matches!(
                kind,
                "ping" | "get_status" | "launch_desktop" | "focus_desktop"
            ) {
                serde_json::to_vec(&json!({
                    "id": req_id,
                    "ok": true,
                    "data": {
                        "desktopRunning": false,
                        "enabled": true,
                        "session": "offline",
                        "accountEmail": Value::Null,
                        "helloEnabled": false,
                        "biometricGate": false,
                        "itemCount": 0,
                    }
                }))
                .unwrap_or_default()
            } else {
                serde_json::to_vec(&json!({
                    "id": req_id,
                    "ok": false,
                    "error": "desktopOffline",
                    "message": "Bytewarden 桌面端未运行，请点击启动桌面端"
                }))
                .unwrap_or_default()
            };
            if let Ok(mut out) = stdout_mutex.lock() {
                write_native_frame(&mut *out, &offline_reply)?;
            }
        }
        Ok(())
    }

    pub fn write_firefox_registry_key(manifest_path: &std::path::Path) -> Result<(), String> {
        let subkey = wide_null(&format!(
            "Software\\Mozilla\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"
        ));
        let value = wide_null(&manifest_path.to_string_lossy());
        let mut hkey: isize = 0;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                null(),
                0,
                KEY_WRITE,
                null(),
                &mut hkey,
                null_mut(),
            )
        };
        if status != 0 {
            return Err(format!("RegCreateKeyExW 失败 (code {status})"));
        }
        let set_status = unsafe {
            RegSetValueExW(
                hkey,
                null(),
                0,
                REG_SZ,
                value.as_ptr() as *const u8,
                (value.len() * 2) as u32,
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if set_status != 0 {
            return Err(format!("RegSetValueExW 失败 (code {set_status})"));
        }
        Ok(())
    }

    pub fn registry_points_to(expected: &std::path::Path) -> bool {
        let subkey = wide_null(&format!(
            "Software\\Mozilla\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"
        ));
        let mut hkey: isize = 0;
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) } != 0
        {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        let ok = unsafe {
            RegQueryValueExW(
                hkey,
                null(),
                null_mut(),
                &mut kind,
                buf.as_mut_ptr() as *mut u8,
                &mut bytes,
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if ok != 0 || kind != REG_SZ || bytes < 2 {
            return false;
        }
        let len = (bytes as usize / 2).saturating_sub(1);
        let actual = String::from_utf16_lossy(&buf[..len]);
        actual
            .trim_end_matches('\0')
            .eq_ignore_ascii_case(&expected.to_string_lossy())
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buf: u32,
            in_buf: u32,
            default_timeout: u32,
            security: *const c_void,
        ) -> Handle;
        fn ConnectNamedPipe(pipe: Handle, overlapped: *mut Overlapped) -> i32;
        fn DisconnectNamedPipe(pipe: Handle) -> i32;
        fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
        fn GetNamedPipeClientProcessId(pipe: Handle, client_pid: *mut u32) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn ReadFile(
            file: Handle,
            buf: *mut c_void,
            to_read: u32,
            read: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn WriteFile(
            file: Handle,
            buf: *const c_void,
            to_write: u32,
            written: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn GetOverlappedResult(
            file: Handle,
            overlapped: *mut Overlapped,
            transferred: *mut u32,
            wait: i32,
        ) -> i32;
        fn CancelIoEx(file: Handle, overlapped: *const Overlapped) -> i32;
        fn CreateEventW(
            security: *const c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> Handle;
        fn SetEvent(event: Handle) -> i32;
        fn ResetEvent(event: Handle) -> i32;
        fn WaitForMultipleObjects(
            count: u32,
            handles: *const Handle,
            wait_all: i32,
            millis: u32,
        ) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
        fn GetCurrentProcessId() -> u32;
        fn ProcessIdToSessionId(pid: u32, session_id: *mut u32) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegCreateKeyExW(
            key: isize,
            subkey: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam: u32,
            security: *const c_void,
            result: *mut isize,
            disposition: *mut u32,
        ) -> i32;
        fn RegOpenKeyExW(
            key: isize,
            subkey: *const u16,
            options: u32,
            sam: u32,
            result: *mut isize,
        ) -> i32;
        fn RegSetValueExW(
            key: isize,
            value_name: *const u16,
            reserved: u32,
            kind: u32,
            data: *const u8,
            cb_data: u32,
        ) -> i32;
        fn RegQueryValueExW(
            key: isize,
            value_name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            data: *mut u8,
            cb_data: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }

    #[cfg(test)]
    pub(super) mod pipe_tests {
        use super::*;

        #[test]
        fn named_pipe_bridge_supports_rpc_and_realtime_broadcast() {
            let tag = format!("test-{}", uuid::Uuid::new_v4().simple());
            let pipe_name = pipe_name_for_scope(&tag);
            let (tx, mut rx) = mpsc::unbounded();
            let server = BridgeServer::start(&pipe_name, tx).expect("bridge server starts");

            // Spawn a responder thread that echoes ping requests.
            let responder_handle = std::thread::spawn(move || {
                while let Some(ev) = futures::executor::block_on(rx.next()) {
                    if let InboundBridgeEvent::Request { request, responder } = ev {
                        let out = serde_json::to_vec(&json!({
                            "id": request.id,
                            "ok": true,
                            "echo": request.kind,
                        }))
                        .unwrap();
                        let _ = responder.send(out);
                        break;
                    }
                }
            });

            let wide = wide_null(&pipe_name);
            let client = connect_pipe_once(&wide).expect("client connects to named pipe");
            let write_ev = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
            let read_ev = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });

            let req_bytes = serde_json::to_vec(&json!({
                "id": 7,
                "type": "ping",
                "payload": {}
            }))
            .unwrap();
            unsafe {
                write_pipe_frame(client.0, write_ev.0, &req_bytes).expect("client writes request");
            }
            let reply_bytes =
                unsafe { read_pipe_frame(client.0, read_ev.0).expect("client reads reply") };
            let reply: Value = serde_json::from_slice(&reply_bytes).unwrap();
            assert_eq!(reply["id"], 7);
            assert_eq!(reply["echo"], "ping");

            // Verify server-initiated push broadcast (`state_changed`) arrives immediately.
            let push_bytes = serde_json::to_vec(&json!({
                "type": "event",
                "event": "state_changed",
                "data": { "session": "locked" }
            }))
            .unwrap();
            server.broadcast(&push_bytes);
            let pushed =
                unsafe { read_pipe_frame(client.0, read_ev.0).expect("client reads broadcast") };
            let pushed_json: Value = serde_json::from_slice(&pushed).unwrap();
            assert_eq!(pushed_json["event"], "state_changed");
            assert_eq!(pushed_json["data"]["session"], "locked");

            responder_handle.join().unwrap();
            server.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_frame_roundtrip_and_bounds_check() {
        let mut buf = Vec::new();
        let msg = br#"{"id":1,"type":"get_status"}"#;
        write_native_frame(&mut buf, msg).unwrap();
        assert_eq!(buf.len(), 4 + msg.len());

        let mut cursor = io::Cursor::new(buf);
        let decoded = read_native_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(decoded, msg);
        assert!(read_native_frame(&mut cursor).unwrap().is_none());

        // Reject oversized frames (> 256 KB).
        let huge = ((MAX_FRAME_BYTES as u32) + 1).to_le_bytes();
        let mut bad = io::Cursor::new(huge.to_vec());
        assert!(read_native_frame(&mut bad).is_err());
    }

    #[test]
    fn domain_matching_handles_subdomains_and_multipart_tlds_safely() {
        assert_eq!(
            extract_url_host("https://Login.GitHub.com:443/session?x=1").as_deref(),
            Some("login.github.com")
        );
        assert_eq!(extract_url_host("about:debugging"), None);
        assert_eq!(extract_url_host("moz-extension://abc/popup.html"), None);

        // Exact host > subdomain match > unrelated domain.
        assert_eq!(domain_match_score("github.com", "github.com", false), 100);
        assert_eq!(
            domain_match_score("gist.github.com", "login.github.com", true),
            85
        );
        assert_eq!(domain_match_score("evilgithub.com", "github.com", false), 0);

        // Multi-part public suffixes (.co.uk, .com.cn) must NEVER match sibling tenants!
        assert_eq!(domain_match_score("a.co.uk", "b.co.uk", false), 0);
        assert_eq!(
            domain_match_score("login.bbc.co.uk", "bbc.co.uk", false),
            80
        );
        assert_eq!(domain_match_score("a.com.cn", "b.com.cn", false), 0);
        assert_eq!(
            domain_match_score("passport.jd.com.cn", "jd.com.cn", false),
            80
        );
    }
}
