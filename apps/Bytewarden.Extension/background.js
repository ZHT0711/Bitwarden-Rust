// Bytewarden Firefox Extension — Zero-Vault-Storage Native Messaging Coordinator.
// Holds zero vault ciphertext or encryption keys in the browser process; all crypto
// and secret storage stay inside the native Rust Bytewarden Desktop process.

const HOST_NAME = "com.bytewarden.desktop";

let nativePort = null;
let reconnectTimer = null;
let nextRequestId = 1;
const pendingRequests = new Map();

let bridgeState = {
  desktopRunning: false,
  enabled: true,
  session: "offline", // "offline" | "signedOut" | "locked" | "unlocked"
  accountEmail: null,
  helloEnabled: false,
  biometricGate: false,
  itemCount: 0,
};

function ensurePort() {
  if (nativePort) return nativePort;
  try {
    const port = browser.runtime.connectNative(HOST_NAME);
    nativePort = port;

    port.onMessage.addListener((msg) => {
      if (!msg || typeof msg !== "object") return;
      if (msg.type === "event" && msg.event === "state_changed" && msg.data) {
        applyBridgeState(msg.data);
        return;
      }
      if (typeof msg.id === "number" && pendingRequests.has(msg.id)) {
        const entry = pendingRequests.get(msg.id);
        pendingRequests.delete(msg.id);
        clearTimeout(entry.timer);
        entry.resolve(msg);
      }
    });

    port.onDisconnect.addListener(() => {
      nativePort = null;
      for (const [, entry] of pendingRequests) {
        clearTimeout(entry.timer);
        entry.resolve({
          ok: false,
          error: "disconnected",
          message: port.error ? String(port.error.message || port.error) : "原生通信连接已断开",
        });
      }
      pendingRequests.clear();
      applyBridgeState({
        desktopRunning: false,
        enabled: true,
        session: "offline",
        accountEmail: null,
        helloEnabled: false,
        biometricGate: false,
        itemCount: 0,
      });
      scheduleReconnect();
    });

    sendNative("get_status", {}, 3000).then((res) => {
      if (res && res.ok && res.data) {
        applyBridgeState(res.data);
      }
    });
  } catch (_err) {
    nativePort = null;
    scheduleReconnect();
  }
  return nativePort;
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    ensurePort();
  }, 3500);
}

function sendNative(type, payload = {}, timeoutMs = 45000) {
  const port = ensurePort();
  if (!port) {
    return Promise.resolve({
      ok: false,
      error: "hostUnavailable",
      message: "未检测到 Bytewarden 原生通信清单，请在桌面端「设置 → 安全与超时」中点击注册到 Firefox",
    });
  }
  const id = nextRequestId++;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      pendingRequests.delete(id);
      resolve({ ok: false, error: "timeout", message: "等待桌面端响应超时" });
    }, timeoutMs);
    pendingRequests.set(id, { resolve, timer });
    try {
      port.postMessage({ id, type, payload });
    } catch (err) {
      clearTimeout(timer);
      pendingRequests.delete(id);
      resolve({ ok: false, error: "sendFailed", message: String(err) });
    }
  });
}

function applyBridgeState(next) {
  const prevSession = bridgeState.session;
  bridgeState = { ...bridgeState, ...next };
  updateBadgeForActiveTab();

  browser.runtime
    .sendMessage({ type: "bw:state_changed", state: bridgeState })
    .catch(() => {});

  if (prevSession === "unlocked" && bridgeState.session !== "unlocked") {
    broadcastToTabs({ type: "bw:vault_locked" });
  }
}

async function broadcastToTabs(message) {
  try {
    const tabs = await browser.tabs.query({});
    for (const tab of tabs) {
      if (tab.id != null) {
        browser.tabs.sendMessage(tab.id, message).catch(() => {});
      }
    }
  } catch (_e) {}
}

async function updateBadgeForActiveTab() {
  try {
    if (!bridgeState.desktopRunning || bridgeState.session === "offline") {
      await browser.action.setBadgeText({ text: "" });
      return;
    }
    if (bridgeState.session !== "unlocked") {
      await browser.action.setBadgeBackgroundColor({ color: "#6B7280" });
      await browser.action.setBadgeTextColor({ color: "#FFFFFF" });
      await browser.action.setBadgeText({ text: "锁" });
      return;
    }
    const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
    const url = tab && tab.url ? tab.url : "";
    if (!url.startsWith("http://") && !url.startsWith("https://")) {
      await browser.action.setBadgeText({ text: "" });
      return;
    }
    const res = await sendNative("query_logins", { url, query: "", limit: 20 }, 2500);
    const count = res && res.ok && res.data && Array.isArray(res.data.tabMatches)
      ? res.data.tabMatches.length
      : 0;
    if (count > 0) {
      await browser.action.setBadgeBackgroundColor({ color: "#7C3AED" });
      await browser.action.setBadgeTextColor({ color: "#FFFFFF" });
      await browser.action.setBadgeText({ text: count > 9 ? "9+" : String(count) });
    } else {
      await browser.action.setBadgeText({ text: "" });
    }
  } catch (_e) {}
}

async function fillCredentialsInTab(tabId, frameId, itemId, tabUrl) {
  const res = await sendNative("get_credentials", {
    id: itemId,
    tabUrl: tabUrl || "",
    action: "fill",
  });
  if (!res || !res.ok || !res.data) {
    return res || { ok: false, message: "获取凭据失败" };
  }
  const { username, password, totp } = res.data;
  const opts = typeof frameId === "number" ? { frameId } : {};
  await browser.tabs
    .sendMessage(
      tabId,
      {
        type: "bw:perform_fill",
        username: username || "",
        password: password || "",
        totp: totp || "",
      },
      opts
    )
    .catch(() => {});

  if (totp) {
    try {
      await navigator.clipboard.writeText(totp);
    } catch (_e) {}
  }
  return { ok: true, data: { totpCopied: Boolean(totp) } };
}

function setupContextMenus() {
  browser.contextMenus.removeAll().then(() => {
    browser.contextMenus.create({
      id: "bw-fill-best",
      title: "使用 Bytewarden 自动填充登录信息",
      contexts: ["editable", "page"],
    });
    browser.contextMenus.create({
      id: "bw-generate-fill",
      title: "生成强密码并填充到当前输入框",
      contexts: ["editable"],
    });
    browser.contextMenus.create({
      id: "bw-lock-now",
      title: "立即锁定 Bytewarden 保险库",
      contexts: ["action", "page"],
    });
  }).catch(() => {});
}

browser.contextMenus.onClicked.addListener(async (info, tab) => {
  if (!tab || tab.id == null) return;
  if (info.menuItemId === "bw-lock-now") {
    await sendNative("lock_vault", {});
    return;
  }
  if (info.menuItemId === "bw-generate-fill") {
    const gen = await sendNative("generate_password", {
      length: 20,
      uppercase: true,
      lowercase: true,
      numbers: true,
      symbols: true,
    });
    if (gen && gen.ok && gen.data && gen.data.value) {
      try {
        await navigator.clipboard.writeText(gen.data.value);
      } catch (_e) {}
      await browser.tabs
        .sendMessage(
          tab.id,
          { type: "bw:fill_generated_password", password: gen.data.value },
          typeof info.frameId === "number" ? { frameId: info.frameId } : {}
        )
        .catch(() => {});
    }
    return;
  }
  if (info.menuItemId === "bw-fill-best") {
    const url = info.frameUrl || tab.url || "";
    const list = await sendNative("query_logins", { url, query: "", limit: 5 });
    if (list && list.ok && list.data && list.data.tabMatches && list.data.tabMatches.length > 0) {
      await fillCredentialsInTab(tab.id, info.frameId, list.data.tabMatches[0].id, url);
    } else {
      await sendNative("focus_desktop", {});
    }
  }
});

browser.commands.onCommand.addListener(async (command) => {
  if (command !== "autofill-active") return;
  const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
  if (!tab || tab.id == null || !tab.url) return;
  if (bridgeState.session !== "unlocked") {
    await sendNative("focus_desktop", {});
    return;
  }
  const list = await sendNative("query_logins", { url: tab.url, query: "", limit: 5 });
  if (list && list.ok && list.data && list.data.tabMatches && list.data.tabMatches.length > 0) {
    await fillCredentialsInTab(tab.id, undefined, list.data.tabMatches[0].id, tab.url);
  }
});

browser.tabs.onActivated.addListener(() => {
  updateBadgeForActiveTab();
});

browser.tabs.onUpdated.addListener((_tabId, changeInfo, tab) => {
  if (changeInfo.status === "complete" && tab && tab.active) {
    updateBadgeForActiveTab();
  }
});

browser.runtime.onMessage.addListener((msg, sender) => {
  if (!msg || typeof msg !== "object") return undefined;

  switch (msg.type) {
    case "bw:get_state":
      return sendNative("get_status", {}, 2500).then((res) => {
        if (res && res.ok && res.data) {
          applyBridgeState(res.data);
        }
        return { ok: true, state: bridgeState, rawError: res && !res.ok ? res.message : null };
      });

    case "bw:native":
      return sendNative(msg.command, msg.payload || {}, msg.timeoutMs || 45000).then((res) => {
        if (res && res.ok && res.data && typeof res.data.session === "string") {
          applyBridgeState(res.data);
        }
        return res;
      });

    case "bw:fill_item": {
      const targetTabId = msg.tabId != null ? msg.tabId : sender.tab && sender.tab.id;
      const targetFrameId = msg.frameId != null ? msg.frameId : sender.frameId;
      if (targetTabId == null) {
        return Promise.resolve({ ok: false, message: "找不到活动标签页" });
      }
      return fillCredentialsInTab(targetTabId, targetFrameId, msg.itemId, msg.tabUrl || "");
    }

    case "bw:inline_query": {
      const url = msg.url || (sender.tab && sender.tab.url) || "";
      if (!bridgeState.desktopRunning || bridgeState.session === "offline") {
        return Promise.resolve({ ok: true, state: bridgeState, matches: [] });
      }
      if (bridgeState.session !== "unlocked") {
        return Promise.resolve({ ok: true, state: bridgeState, matches: [] });
      }
      return sendNative("query_logins", { url, query: "", limit: 6 }, 3000).then((res) => ({
        ok: true,
        state: bridgeState,
        matches: (res && res.ok && res.data && res.data.tabMatches) || [],
      }));
    }

    default:
      return undefined;
  }
});

setupContextMenus();
ensurePort();
