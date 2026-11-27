const $ = (id) => document.getElementById(id);

let activeTab = null;
let currentState = null;
let searchDebounce = null;
let toastTimer = null;

function showToast(text) {
  const el = $("toast");
  el.textContent = text;
  el.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add("hidden"), 2200);
}

async function nativeCall(command, payload = {}) {
  return browser.runtime.sendMessage({
    type: "bw:native",
    command,
    payload,
  });
}

function renderState(state, rawError = null) {
  currentState = state;
  const viewOffline = $("view-offline");
  const viewLocked = $("view-locked");
  const viewUnlocked = $("view-unlocked");
  const unlockedActions = $("unlocked-actions");
  const sub = $("status-sub");

  viewOffline.classList.add("hidden");
  viewLocked.classList.add("hidden");
  viewUnlocked.classList.add("hidden");
  unlockedActions.classList.add("hidden");

  if (!state || !state.desktopRunning || state.session === "offline") {
    sub.textContent = "桌面端未连接";
    if (rawError && rawError.includes("未检测到")) {
      $("offline-title").textContent = "尚未注册 Firefox 原生通信";
      $("offline-desc").textContent = rawError;
    } else {
      $("offline-title").textContent = "桌面端尚未运行";
      $("offline-desc").textContent =
        "浏览器扩展不驻留任何密码库副本，点击下方按钮即可拉起轻量级 Bytewarden 桌面端。";
    }
    viewOffline.classList.remove("hidden");
    return;
  }

  if (!state.enabled) {
    sub.textContent = "桌面端已关闭浏览器联动";
    $("offline-title").textContent = "浏览器联动已关闭";
    $("offline-desc").textContent =
      "请在 Bytewarden 桌面端「设置 → 安全与超时 → Firefox 浏览器扩展联动」中开启开关。";
    viewOffline.classList.remove("hidden");
    return;
  }

  if (state.session !== "unlocked") {
    sub.textContent = state.accountEmail || "等待登录或解锁";
    $("locked-title").textContent = state.session === "signedOut" ? "桌面端尚未登录" : "保险库已锁定";
    $("locked-desc").textContent =
      state.session === "signedOut"
        ? "请先在 Bytewarden 桌面端登录账户。"
        : `账户 ${state.accountEmail || ""} 已锁定，解锁后实时同步就绪。`;
    $("btn-hello-unlock").classList.toggle("hidden", !state.helloEnabled);
    viewLocked.classList.remove("hidden");
    return;
  }

  sub.textContent = `${state.accountEmail || "已解锁"} · ${state.itemCount || 0} 项登录`;
  unlockedActions.classList.remove("hidden");
  viewUnlocked.classList.remove("hidden");
  refreshVaultList();
}

function createVaultRow(item) {
  const row = document.createElement("div");
  row.className = "vault-row";

  const main = document.createElement("div");
  main.className = "row-main";
  main.title = "点击自动填充到当前网页";
  const title = document.createElement("div");
  title.className = "row-title";
  title.textContent = item.name || "未命名条目";
  const sub = document.createElement("div");
  sub.className = "row-sub";
  sub.textContent = item.username || item.host || "无用户名";
  main.appendChild(title);
  main.appendChild(sub);

  const doFill = async () => {
    if (!activeTab || activeTab.id == null) return;
    const res = await browser.runtime.sendMessage({
      type: "bw:fill_item",
      tabId: activeTab.id,
      itemId: item.id,
      tabUrl: activeTab.url || "",
    });
    if (res && res.ok) {
      showToast(res.data && res.data.totpCopied ? "已填充，验证码已复制" : "已自动填充");
      setTimeout(() => window.close(), 350);
    } else {
      showToast((res && res.message) || "填充已取消或失败");
    }
  };
  main.addEventListener("click", doFill);

  const btns = document.createElement("div");
  btns.className = "row-btns";

  const fillBtn = document.createElement("button");
  fillBtn.className = "fill-pill";
  fillBtn.textContent = "填充";
  fillBtn.addEventListener("click", doFill);

  const copyUserBtn = document.createElement("button");
  copyUserBtn.className = "mini-btn";
  copyUserBtn.title = "复制用户名";
  copyUserBtn.textContent = "账";
  copyUserBtn.addEventListener("click", async () => {
    const res = await nativeCall("get_credentials", {
      id: item.id,
      tabUrl: (activeTab && activeTab.url) || "",
      action: "copy_username",
    });
    if (res && res.ok && res.data && res.data.username) {
      await navigator.clipboard.writeText(res.data.username);
      showToast("已复制用户名");
    } else {
      showToast("该条目没有用户名");
    }
  });

  const copyPassBtn = document.createElement("button");
  copyPassBtn.className = "mini-btn";
  copyPassBtn.title = "复制密码";
  copyPassBtn.textContent = "密";
  copyPassBtn.addEventListener("click", async () => {
    const res = await nativeCall("get_credentials", {
      id: item.id,
      tabUrl: (activeTab && activeTab.url) || "",
      action: "copy_password",
    });
    if (res && res.ok && res.data && res.data.password) {
      await navigator.clipboard.writeText(res.data.password);
      showToast("已复制密码");
    } else {
      showToast((res && res.message) || "无可用密码");
    }
  });

  const copyTotpBtn = document.createElement("button");
  copyTotpBtn.className = "mini-btn";
  copyTotpBtn.title = "复制 TOTP 动态验证码";
  copyTotpBtn.textContent = "码";
  copyTotpBtn.addEventListener("click", async () => {
    const res = await nativeCall("get_credentials", {
      id: item.id,
      tabUrl: (activeTab && activeTab.url) || "",
      action: "copy_totp",
    });
    if (res && res.ok && res.data && res.data.totp) {
      await navigator.clipboard.writeText(res.data.totp);
      showToast(`已复制验证码 (${res.data.totp})`);
    } else {
      showToast((res && res.message) || "该条目未配置 TOTP 验证码");
    }
  });

  btns.append(fillBtn, copyUserBtn, copyPassBtn, copyTotpBtn);
  row.append(main, btns);
  return row;
}

async function refreshVaultList() {
  if (!currentState || currentState.session !== "unlocked") return;
  const query = $("search-input").value.trim();
  const url = activeTab && activeTab.url ? activeTab.url : "";
  const res = await nativeCall("query_logins", { url, query, limit: 35 });
  if (!res || !res.ok || !res.data) return;

  const tabMatches = res.data.tabMatches || [];
  const results = res.data.results || [];

  const secMatch = $("section-tab-matches");
  const listMatch = $("list-tab-matches");
  listMatch.replaceChildren();
  if (tabMatches.length > 0 && !query) {
    for (const item of tabMatches) {
      listMatch.appendChild(createVaultRow(item));
    }
    secMatch.classList.remove("hidden");
  } else {
    secMatch.classList.add("hidden");
  }

  const secRes = $("section-results");
  const listRes = $("list-results");
  listRes.replaceChildren();
  const shownList = query ? results : results;
  if (shownList.length > 0) {
    $("results-label").textContent = query ? `搜索结果 (${shownList.length})` : "全部登录项";
    for (const item of shownList) {
      listRes.appendChild(createVaultRow(item));
    }
    secRes.classList.remove("hidden");
  } else {
    secRes.classList.add("hidden");
  }

  const empty = tabMatches.length === 0 && shownList.length === 0;
  $("vault-empty").classList.toggle("hidden", !empty);
}

async function refreshGeneratedPassword() {
  const passphrase = $("gen-passphrase").checked;
  const res = await nativeCall("generate_password", {
    passphrase,
    length: Number($("gen-length").value),
    uppercase: $("gen-upper").checked,
    lowercase: $("gen-lower").checked,
    numbers: $("gen-num").checked,
    symbols: $("gen-sym").checked,
    words: Number($("gen-words").value),
    separator: "-",
    capitalize: true,
    includeNumber: true,
  });
  if (res && res.ok && res.data && res.data.value) {
    $("gen-output").textContent = res.data.value;
    return res.data.value;
  }
  return "";
}

async function prefillCreateTab() {
  if (!activeTab) return;
  if (!$("new-uri").value && activeTab.url && activeTab.url.startsWith("http")) {
    $("new-uri").value = activeTab.url;
  }
  if (!$("new-name").value && activeTab.title) {
    $("new-name").value = activeTab.title.split(" - ")[0].trim();
  }
}

document.addEventListener("DOMContentLoaded", async () => {
  const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
  activeTab = tab || null;

  // Segmented tabs
  for (const btn of document.querySelectorAll(".seg-btn")) {
    btn.addEventListener("click", () => {
      for (const b of document.querySelectorAll(".seg-btn")) {
        b.classList.toggle("active", b === btn);
      }
      const target = btn.dataset.tab;
      $("tab-vault").classList.toggle("hidden", target !== "vault");
      $("tab-generator").classList.toggle("hidden", target !== "generator");
      $("tab-create").classList.toggle("hidden", target !== "create");
      if (target === "generator") refreshGeneratedPassword();
      if (target === "create") prefillCreateTab();
    });
  }

  // Search box (1-click instant clear)
  const searchInput = $("search-input");
  const clearBtn = $("search-clear");
  searchInput.addEventListener("input", () => {
    clearBtn.classList.toggle("hidden", searchInput.value.length === 0);
    clearTimeout(searchDebounce);
    searchDebounce = setTimeout(refreshVaultList, 90);
  });
  clearBtn.addEventListener("mousedown", (e) => {
    e.preventDefault();
    searchInput.value = "";
    clearBtn.classList.add("hidden");
    searchInput.focus();
    refreshVaultList();
  });

  // Topbar & Auth buttons
  $("btn-launch-desktop").addEventListener("click", async () => {
    $("btn-launch-desktop").textContent = "正在启动…";
    const res = await nativeCall("launch_desktop");
    $("btn-launch-desktop").textContent = "启动 Bytewarden 桌面端";
    if (res && res.ok && res.data) renderState(res.data);
  });

  $("btn-hello-unlock").addEventListener("click", async () => {
    $("btn-hello-unlock").textContent = "请在 Windows Hello 弹窗验证…";
    const res = await nativeCall("unlock_hello");
    $("btn-hello-unlock").textContent = "Windows Hello 解锁";
    if (res && res.ok && res.data) {
      renderState(res.data);
    } else if (res && res.message) {
      showToast(res.message);
    }
  });

  $("btn-open-desktop-unlock").addEventListener("click", async () => {
    await nativeCall("focus_desktop");
    window.close();
  });

  $("btn-lock").addEventListener("click", async () => {
    await nativeCall("lock_vault");
  });

  $("btn-sync").addEventListener("click", async () => {
    const res = await nativeCall("sync_vault");
    if (res && res.ok) showToast("已在桌面端触发同步");
  });

  // Generator controls
  $("gen-passphrase").addEventListener("change", () => {
    const phrase = $("gen-passphrase").checked;
    $("gen-pw-controls").classList.toggle("hidden", phrase);
    $("gen-phrase-controls").classList.toggle("hidden", !phrase);
    refreshGeneratedPassword();
  });
  $("gen-length").addEventListener("input", () => {
    $("gen-len-val").textContent = $("gen-length").value;
    refreshGeneratedPassword();
  });
  $("gen-words").addEventListener("input", () => {
    $("gen-words-val").textContent = $("gen-words").value;
    refreshGeneratedPassword();
  });
  for (const id of ["gen-upper", "gen-lower", "gen-num", "gen-sym"]) {
    $(id).addEventListener("change", refreshGeneratedPassword);
  }
  $("btn-gen-refresh").addEventListener("click", refreshGeneratedPassword);
  $("btn-gen-copy").addEventListener("click", async () => {
    const text = $("gen-output").textContent;
    if (text) {
      await navigator.clipboard.writeText(text);
      showToast("密码已复制");
    }
  });
  $("btn-gen-fill").addEventListener("click", async () => {
    const text = $("gen-output").textContent;
    if (activeTab && activeTab.id != null && text) {
      await browser.tabs
        .sendMessage(activeTab.id, { type: "bw:fill_generated_password", password: text })
        .catch(() => {});
      await navigator.clipboard.writeText(text).catch(() => {});
      showToast("已填入当前输入框并复制");
    }
  });

  // Create new login controls
  $("btn-new-gen").addEventListener("click", async () => {
    const pw = await refreshGeneratedPassword();
    if (pw) {
      $("new-password").type = "text";
      $("new-password").value = pw;
      showToast("已生成强密码");
    }
  });
  $("btn-read-form").addEventListener("click", async () => {
    if (!activeTab || activeTab.id == null) return;
    const snap = await browser.tabs
      .sendMessage(activeTab.id, { type: "bw:inspect_page_form" })
      .catch(() => null);
    if (snap) {
      if (snap.username) $("new-username").value = snap.username;
      if (snap.password) $("new-password").value = snap.password;
      if (snap.url) $("new-uri").value = snap.url;
      if (snap.title && !$("new-name").value) $("new-name").value = snap.title.split(" - ")[0];
      showToast("已读取网页表单内容");
    } else {
      showToast("当前页面无可读取表单");
    }
  });
  $("btn-save-login").addEventListener("click", async () => {
    const payload = {
      name: $("new-name").value.trim(),
      username: $("new-username").value.trim(),
      password: $("new-password").value,
      uri: $("new-uri").value.trim(),
    };
    const res = await nativeCall("create_login", payload);
    if (res && res.ok) {
      $("new-username").value = "";
      $("new-password").value = "";
      showToast("已保存到桌面端保险库");
      document.querySelector('.seg-btn[data-tab="vault"]').click();
      refreshVaultList();
    } else {
      showToast((res && res.message) || "保存失败");
    }
  });

  // Live state updates from background
  browser.runtime.onMessage.addListener((msg) => {
    if (msg && msg.type === "bw:state_changed" && msg.state) {
      renderState(msg.state);
    }
  });

  const init = await browser.runtime.sendMessage({ type: "bw:get_state" }).catch(() => null);
  if (init && init.state) {
    renderState(init.state, init.rawError);
  } else {
    renderState({ desktopRunning: false, session: "offline" });
  }
});
