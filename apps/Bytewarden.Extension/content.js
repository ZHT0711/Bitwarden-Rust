// Bytewarden Firefox Extension — Lightweight Focus-Driven Content Script.
// Zero MutationObserver overhead: activates ONLY when the user focuses a login field
// or triggers autofill from the popup / keyboard shortcut / context menu.

(() => {
  if (window.__bytewardenInjected) return;
  window.__bytewardenInjected = true;

  let activeInput = null;
  let hostEl = null;
  let shadow = null;
  let menuOpen = false;
  let repositionRaf = 0;

  const USERNAME_HINTS = /user|email|login|account|acct|phone|mobile|id|账号|邮箱|手机|用户名/i;
  const SEARCH_OR_NON_LOGIN = /search|query|coupon|promo|captcha|otp|code|搜索|验证码/i;

  function isVisibleControl(el) {
    if (!el || !(el instanceof HTMLInputElement)) return false;
    if (el.disabled || el.readOnly || el.type === "hidden") return false;
    const rect = el.getBoundingClientRect();
    return rect.width >= 60 && rect.height >= 18;
  }

  function isPasswordField(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    if (el.type === "password") return true;
    const ac = (el.getAttribute("autocomplete") || "").toLowerCase();
    return ac.includes("current-password") || ac.includes("new-password");
  }

  function isLikelyUsernameField(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    const type = (el.type || "text").toLowerCase();
    if (!["text", "email", "tel", ""].includes(type)) return false;
    const ac = (el.getAttribute("autocomplete") || "").toLowerCase();
    if (ac.includes("username") || ac === "email") return true;

    const meta = `${el.name || ""} ${el.id || ""} ${el.placeholder || ""} ${el.getAttribute("aria-label") || ""}`;
    if (SEARCH_OR_NON_LOGIN.test(meta)) return false;

    const scope = el.form || el.closest("form, [role='dialog'], main, body") || document.body;
    const hasPwNearby = Boolean(scope.querySelector('input[type="password"]'));
    return hasPwNearby && (USERNAME_HINTS.test(meta) || type === "email" || meta.trim() === "");
  }

  function isLoginEligibleField(el) {
    return isVisibleControl(el) && (isPasswordField(el) || isLikelyUsernameField(el));
  }

  function setNativeInputValue(input, value) {
    if (!input || value == null) return;
    input.focus({ preventScroll: true });
    const proto = Object.getPrototypeOf(input);
    const descriptor =
      Object.getOwnPropertyDescriptor(proto, "value") ||
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
    if (descriptor && descriptor.set) {
      descriptor.set.call(input, value);
    } else {
      input.value = value;
    }
    input.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, data: value }));
    input.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
  }

  function findFormPair(seedInput) {
    const scope =
      (seedInput && (seedInput.form || seedInput.closest("form, [role='dialog'], section, div"))) ||
      document;
    const allInputs = Array.from(document.querySelectorAll("input")).filter(isVisibleControl);
    const scopedInputs = Array.from(scope.querySelectorAll("input")).filter(isVisibleControl);
    const pool = scopedInputs.length > 0 ? scopedInputs : allInputs;

    let passwordEl =
      (seedInput && isPasswordField(seedInput) ? seedInput : null) ||
      pool.find(isPasswordField) ||
      allInputs.find(isPasswordField) ||
      null;

    let usernameEl = null;
    if (seedInput && isLikelyUsernameField(seedInput) && seedInput !== passwordEl) {
      usernameEl = seedInput;
    } else if (passwordEl) {
      const idx = allInputs.indexOf(passwordEl);
      for (let i = idx - 1; i >= 0; i--) {
        const candidate = allInputs[i];
        const t = (candidate.type || "text").toLowerCase();
        if (["text", "email", "tel", ""].includes(t) && !SEARCH_OR_NON_LOGIN.test(candidate.name || "")) {
          if (!passwordEl.form || candidate.form === passwordEl.form) {
            usernameEl = candidate;
            break;
          }
        }
      }
    }
    return { usernameEl, passwordEl };
  }

  function ensureOverlay() {
    if (hostEl && shadow) return;
    hostEl = document.createElement("div");
    hostEl.setAttribute("data-bytewarden-ui", "1");
    hostEl.style.cssText =
      "position:fixed;top:0;left:0;width:0;height:0;z-index:2147483646;pointer-events:none;";
    shadow = hostEl.attachShadow({ mode: "closed" });
    const style = document.createElement("style");
    style.textContent = `
      :host { all: initial; }
      .bw-badge {
        position: fixed;
        width: 22px;
        height: 22px;
        border-radius: 6px;
        border: 1px solid rgba(139, 92, 246, 0.35);
        background: rgba(124, 58, 237, 0.12);
        backdrop-filter: blur(8px);
        display: flex;
        align-items: center;
        justify-content: center;
        cursor: pointer;
        pointer-events: auto;
        padding: 0;
        transition: transform 120ms ease, background 120ms ease, border-color 120ms ease;
      }
      .bw-badge:hover {
        background: rgba(124, 58, 237, 0.22);
        border-color: rgba(139, 92, 246, 0.65);
        transform: scale(1.05);
      }
      .bw-badge svg { width: 14px; height: 14px; }
      .bw-popover {
        position: fixed;
        min-width: 250px;
        max-width: 320px;
        border-radius: 10px;
        padding: 6px;
        font-family: "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
        font-size: 12.5px;
        color: #F3F4F6;
        background: rgba(26, 26, 32, 0.96);
        border: 1px solid rgba(255, 255, 255, 0.12);
        box-shadow: 0 12px 32px rgba(0, 0, 0, 0.38);
        backdrop-filter: blur(16px);
        pointer-events: auto;
      }
      @media (prefers-color-scheme: light) {
        .bw-popover {
          color: #18181B;
          background: rgba(255, 255, 255, 0.97);
          border-color: rgba(0, 0, 0, 0.1);
          box-shadow: 0 10px 28px rgba(0, 0, 0, 0.14);
        }
      }
      .bw-head {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 4px 8px 6px;
        font-size: 11px;
        opacity: 0.68;
      }
      .bw-item {
        width: 100%;
        border: none;
        background: transparent;
        color: inherit;
        text-align: left;
        padding: 7px 8px;
        border-radius: 7px;
        cursor: pointer;
        display: flex;
        flex-direction: column;
        gap: 2px;
      }
      .bw-item:hover {
        background: rgba(139, 92, 246, 0.16);
      }
      .bw-item-title {
        font-weight: 600;
        font-size: 12.5px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .bw-item-sub {
        font-size: 11.5px;
        opacity: 0.72;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .bw-empty {
        padding: 8px;
        font-size: 12px;
        opacity: 0.8;
        display: flex;
        flex-direction: column;
        gap: 6px;
      }
      .bw-action-btn {
        border: none;
        border-radius: 6px;
        padding: 6px 10px;
        font-size: 12px;
        font-weight: 600;
        color: #FFFFFF;
        background: #7C3AED;
        cursor: pointer;
      }
      .bw-action-btn:hover { background: #6D28D9; }
    `;
    shadow.appendChild(style);
    const root = document.createElement("div");
    root.id = "bw-root";
    shadow.appendChild(root);
    (document.documentElement || document.body).appendChild(hostEl);
  }

  function hideOverlay() {
    menuOpen = false;
    if (shadow) {
      const root = shadow.getElementById("bw-root");
      if (root) root.replaceChildren();
    }
  }

  function positionBadgeAndMenu() {
    if (!activeInput || !shadow || !document.contains(activeInput) || !isVisibleControl(activeInput)) {
      hideOverlay();
      return;
    }
    const root = shadow.getElementById("bw-root");
    if (!root) return;
    const badge = root.querySelector(".bw-badge");
    if (!badge) return;
    const rect = activeInput.getBoundingClientRect();
    const top = Math.round(rect.top + (rect.height - 22) / 2);
    const left = Math.round(rect.right - 28);
    badge.style.top = `${top}px`;
    badge.style.left = `${Math.max(rect.left + 4, left)}px`;

    const pop = root.querySelector(".bw-popover");
    if (pop) {
      const popTop = Math.min(window.innerHeight - 190, Math.round(rect.bottom + 6));
      const popLeft = Math.max(8, Math.min(window.innerWidth - 280, Math.round(rect.left)));
      pop.style.top = `${popTop}px`;
      pop.style.left = `${popLeft}px`;
    }
  }

  function renderBadgeForInput(input) {
    activeInput = input;
    ensureOverlay();
    const root = shadow.getElementById("bw-root");
    if (!root) return;
    root.replaceChildren();

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "bw-badge";
    btn.title = "使用 Bytewarden 自动填充 (Alt+Shift+L)";
    btn.innerHTML = `<svg viewBox="0 0 64 64" fill="none"><path d="M32 10L50 18V32C50 44.4 42.3 55.2 32 58.6C21.7 55.2 14 44.4 14 32V18L32 10Z" stroke="#8B5CF6" stroke-width="5" stroke-linejoin="round"/><path d="M32 17V51.8C39.2 48.8 44 41 44 32V22L32 17Z" fill="#8B5CF6"/></svg>`;
    btn.addEventListener("mousedown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      toggleInlineMenu();
    });
    root.appendChild(btn);
    positionBadgeAndMenu();
  }

  async function toggleInlineMenu() {
    if (!shadow || !activeInput) return;
    const root = shadow.getElementById("bw-root");
    if (!root) return;
    const existing = root.querySelector(".bw-popover");
    if (existing) {
      existing.remove();
      menuOpen = false;
      return;
    }
    menuOpen = true;
    const pop = document.createElement("div");
    pop.className = "bw-popover";
    pop.addEventListener("mousedown", (e) => e.stopPropagation());

    const head = document.createElement("div");
    head.className = "bw-head";
    head.textContent = "Bytewarden 快速填充";
    pop.appendChild(head);

    const res = await browser.runtime
      .sendMessage({ type: "bw:inline_query", url: location.href })
      .catch(() => null);

    const state = res && res.state ? res.state : { session: "offline" };
    if (!state.desktopRunning || state.session === "offline") {
      const box = document.createElement("div");
      box.className = "bw-empty";
      box.textContent = "Bytewarden 桌面端尚未运行";
      const launchBtn = document.createElement("button");
      launchBtn.className = "bw-action-btn";
      launchBtn.textContent = "启动 Bytewarden 桌面端";
      launchBtn.addEventListener("click", async () => {
        await browser.runtime.sendMessage({ type: "bw:native", command: "launch_desktop" });
        hideOverlay();
      });
      box.appendChild(launchBtn);
      pop.appendChild(box);
    } else if (state.session !== "unlocked") {
      const box = document.createElement("div");
      box.className = "bw-empty";
      box.textContent = "保险库已锁定";
      const unlockBtn = document.createElement("button");
      unlockBtn.className = "bw-action-btn";
      unlockBtn.textContent = state.helloEnabled ? "Windows Hello 解锁" : "打开桌面端解锁";
      unlockBtn.addEventListener("click", async () => {
        const cmd = state.helloEnabled ? "unlock_hello" : "focus_desktop";
        const r = await browser.runtime.sendMessage({ type: "bw:native", command: cmd });
        if (r && r.ok && cmd === "unlock_hello") {
          existing && existing.remove();
          menuOpen = false;
          toggleInlineMenu();
        } else {
          hideOverlay();
        }
      });
      box.appendChild(unlockBtn);
      pop.appendChild(box);
    } else {
      const matches = (res && res.matches) || [];
      if (matches.length === 0) {
        const empty = document.createElement("div");
        empty.className = "bw-empty";
        empty.textContent = "当前网站暂无匹配登录项（可在右上角扩展图标搜索全库）";
        pop.appendChild(empty);
      } else {
        for (const item of matches) {
          const row = document.createElement("button");
          row.type = "button";
          row.className = "bw-item";
          const title = document.createElement("div");
          title.className = "bw-item-title";
          title.textContent = item.name || "未命名登录";
          const sub = document.createElement("div");
          sub.className = "bw-item-sub";
          sub.textContent = item.username || item.host || "";
          row.appendChild(title);
          row.appendChild(sub);
          row.addEventListener("click", async () => {
            hideOverlay();
            await browser.runtime.sendMessage({
              type: "bw:fill_item",
              itemId: item.id,
              tabUrl: location.href,
            });
          });
          pop.appendChild(row);
        }
      }
    }

    root.appendChild(pop);
    positionBadgeAndMenu();
  }

  document.addEventListener(
    "focusin",
    (e) => {
      const target = e.target;
      if (isLoginEligibleField(target)) {
        renderBadgeForInput(target);
      }
    },
    true
  );

  document.addEventListener(
    "mousedown",
    (e) => {
      if (hostEl && e.target === hostEl) return;
      if (menuOpen) {
        hideOverlay();
      } else if (activeInput && e.target !== activeInput) {
        hideOverlay();
        activeInput = null;
      }
    },
    true
  );

  const scheduleReposition = () => {
    if (!activeInput || repositionRaf) return;
    repositionRaf = requestAnimationFrame(() => {
      repositionRaf = 0;
      positionBadgeAndMenu();
    });
  };
  window.addEventListener("scroll", scheduleReposition, true);
  window.addEventListener("resize", scheduleReposition, { passive: true });

  browser.runtime.onMessage.addListener((msg) => {
    if (!msg || typeof msg !== "object") return undefined;

    if (msg.type === "bw:vault_locked") {
      hideOverlay();
      return undefined;
    }

    if (msg.type === "bw:perform_fill") {
      const seed =
        (document.activeElement instanceof HTMLInputElement ? document.activeElement : null) ||
        activeInput;
      const { usernameEl, passwordEl } = findFormPair(seed);
      if (usernameEl && msg.username) {
        setNativeInputValue(usernameEl, msg.username);
      }
      if (passwordEl && msg.password) {
        setNativeInputValue(passwordEl, msg.password);
      } else if (!usernameEl && seed && isVisibleControl(seed) && msg.password) {
        setNativeInputValue(seed, msg.password);
      }
      hideOverlay();
      return Promise.resolve({ ok: true });
    }

    if (msg.type === "bw:fill_generated_password") {
      const target =
        (document.activeElement instanceof HTMLInputElement ? document.activeElement : null) ||
        activeInput;
      if (target && isVisibleControl(target) && msg.password) {
        setNativeInputValue(target, msg.password);
      }
      return Promise.resolve({ ok: true });
    }

    if (msg.type === "bw:inspect_page_form") {
      const { usernameEl, passwordEl } = findFormPair(activeInput);
      return Promise.resolve({
        title: document.title || "",
        url: location.href,
        username: usernameEl ? usernameEl.value : "",
        password: passwordEl ? passwordEl.value : "",
      });
    }

    return undefined;
  });
})();
