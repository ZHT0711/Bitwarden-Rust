// Bytewarden Firefox Extension — Lightweight Focus-Driven Content Script.
// Zero MutationObserver overhead: activates ONLY when focusing a login field,
// submitting a login form, or executing autofill (Logins / TOTP / Cards / Identities).

(() => {
  if (window.__bytewardenInjected) return;
  window.__bytewardenInjected = true;

  let activeInput = null;
  let hostEl = null;
  let shadow = null;
  let menuOpen = false;
  let repositionRaf = 0;

  const USERNAME_HINTS = /user|email|login|account|acct|phone|mobile|id|账号|邮箱|手机|用户名/i;
  const OTP_HINTS = /otp|totp|2fa|mfa|token|auth.?code|verify|verification|验证码|动态码|安全码/i;
  const SEARCH_OR_NON_LOGIN = /search|query|coupon|promo|captcha|搜索/i;
  const MARK_DARK_URI =
    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAAOkSURBVFhH1Zfbq5ZVEIefLAsysxOJYaYVSWejzG4z8CLvuvBAieBNRBLddLyLbqIIvZH+gCgqLDUrAs00wRJStEhLO3vIrCQrUfOieGRGZq/e79vuLUL+YPGtvdbMrHnXmvnNbDgDcRZwfswnAVfH3DX3Thtuid+pwFfA9cAG4JOYuzatkT1ljADOjvk2YDXwNvAPcDh+6/wdYBXwZdF3DBvXAtuB54Bfy4E5FgIPd6z/FjrqXtcaPRlcCtwB3Nlh/BCwBvgFmA/MC+dc+6tD3me5HbisPaQffEOVfy+G9gNPAxNC5qY4UIdujrUrgaeAn4te2phS7PeE73VDfPmfxcgy4IpWGLgqRotxwFtFX0e9CW33jYmRwM7mCl9qhYaAJY2tr+OMvjB4UmFluzkMrCj2nm83K3z3rfHWCv8EXFj2JZlZwGPxzl3j8ZCphKSNvWHTwP0MuLXsD8C6EPwC+BG4uOzJdOa7BuSDD+L3DeA14HVgC3AUGFX0LgJ+CJvaXl/2TuAc4G7gvWA0YVq9UmQ0ui9Yr8I0fAh4ALgN2AOMLvsvA2tjLi+8D0yPM09gDLArPDwIvBrX9WyR0QGfRSpOeNDfoXcEmAl81zjwTNjSpraV1UlvZgDM7xSQSk3Fii4HLog1dXzne4HvGweEOn69cn8AE5v943T5IXAshB5tBeIwn0BWq2vpgHvpQA3exCMh5xk+yeS6qQMGYDrwRN0MnBuHZcUT1QGvdUYE3HlFJmH2pAMftQ4IaTSfYFG7GfgUeLL8rQOZYhLWg8Cmsl/xYnmC/7BnDUJHRm2L2RFsGUA6oLyl2ubEKji30UmYVWl/dxuEpsQ9wPIQsMCMrQIFS4Ouc/+FKEzejjWjC5eXSikzetaANExUL+tVtzC3LcELgPvi9kyzXpAhB7vd41T8eeSsgh5gX9ALUu43Mea0mwWXFJv+ekbf0lyLkdcl5IBKywk5v6sRVdb4EPmsjr7FSHSVYznc3D5QOuB+MBiVVSf5P8eg5dhm4cbI9WxIvgXGR3rd3yp0wOfYHDoeqA0D8K6w3bchSdhiqSgvWBU1qjGjXL7oBQ99s8SFpJTc0rMEd8EGUv72JuoV5teYLYuDMR3OXattXA6/XFtDakoTtuWWZ4Onqy3vNSQkddQdVlue8J+SJAyrmV/pPx8eIiPmgTl/N5qU7CnUz39sThmZu5ZpM8WA+hjYGN3ujrjuKntaYBRny2VaXhNz104qwv9X+BenKDBoqI0KaQAAAABJRU5ErkJggg==";
  const MARK_LIGHT_URI =
    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAAP4SURBVFhH1Zfbq1VVFMbNbtDJ1AyjSMuME6lpYXV6NaGHeuuhjJSglwgjerHUN/EljKiX8A+IIqXSSkUo7SJYgUoXvKWmXdTKirQkTfiN+Jbf3I013ft0RAX7YLHnnvMblzXnmGOMNWzY/w0RcQFwmccTgBs11pzWav5ZAzDVv3cCO4FbgI3Apx5rbiBzzxgRMTwiLtQY2Aa8D7wTEQH8pd88BlYB7wE7kvzwWu+QAdwEbAeeA34pBpPhJ4G5XeZ/tYxk+2u9/4mIGAPcAdzVRflRYD1wKCIeBebIOc/92YU/AEwHrqrt9ITO0MK/J0U/AwuB8eZMkUE7dKvmImIcsAD4Kck1OoDbajunQOcFTNKbA38kJSuAa7vwr9dTzwPXAG8leTmqnZg0aExExMXAriJo4aU1b6gAXq507ZaNmteCgicJvFuvny6At5O+JfV6Bzp34AudtckHI+KKsu4E9CAwz+fc7XlGnJyQpAM4YJ2HgC8jYlrHcCLKiY9M3Ap8B4wu68p0uu9S4Hywzr/LgdcjYhnwOXA8IvqS3lHAt9Jp3Rtkq4WIuAiYAaxRRtOcr9WridMH/Kisl2V9DZ8AZgO3A/uBEWn9FeBDj5UX1gL3yGZHSUSMBL63h4eB17xdixNHDhxUKi5zMgT8bbljwP3A3sqBRdYlnYfN3a+dKZxCHJ8IO3QV83oPBy53rEjmAHAfsC87YJ7qx3bzjgA35HUR+iPiA+CESU+3CP8a0xFMr+aKA1prHMjBm7hPmXdCRxIRN+fFfgVgcSAinm1Jn+Rc4h1oKp7nsgM6+3sVcBFxaVu64c5LDnzcckBwGi1H8GJr0QA2AfPTfzlQrthS4HFgc1vqJIAXyhGckj1zEJrURG0N4CEFWwkgOWD+NjcnqoIP13KCblXS/0MrCHUlImImsNKEoxFxdUuDAbzhdN2sA8+7MGl3VtR8ARhbKqUz48zWNUzE7GVnq2v4bqsEPwY8oN3TNat5BcqQSa8CsKY0JKXir1znRVQDMqbmFTgt7/Ezq14vAK5MOpUPZKN3aa6Kkbar5IBOWi5Qzu/WiIrr+NC4OVbr612MhB7lWHVByeW30gEPBgejuJJp8n/SNXg5dkMy2c1D05AA3wDX6XoBj9QyNXQcwBbLyKB0qCG5W7oHbUgK1GJZUHVBVVFKdyvKlS9qfoGNvlniwlWwyS1dS3AvqIF0/h7IWyj4bVQpX1LG1KOx5zptXOLrzaVr6E1pgdtyfXAs6daW94ITkmQke/pteYE+SkrCcC3XW66yEWXEYrAZA6vdpDQ9heWbD5szRrm77ph3OVg/AT5zJ/21tjtzzwn8udW0XLqWwETP9w0pws83/AOJfhr/H4wopQAAAABJRU5ErkJggg==";

  function isVisibleControl(el) {
    if (!el || !(el instanceof HTMLInputElement || el instanceof HTMLSelectElement)) return false;
    if (el.disabled || el.readOnly || el.type === "hidden") return false;
    const rect = el.getBoundingClientRect();
    return rect.width >= 48 && rect.height >= 18;
  }

  function isLoginEligibleField(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    if (el.disabled || el.readOnly || el.type === "hidden") return false;
    // Run cheap attribute/type checks BEFORE getBoundingClientRect() to avoid forced layout on non-login inputs!
    if (!isPasswordField(el) && !isLikelyUsernameField(el) && !isOtpField(el)) return false;
    const rect = el.getBoundingClientRect();
    return rect.width >= 48 && rect.height >= 18;
  }

  function isPasswordField(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    if (el.type === "password") return true;
    const ac = (el.getAttribute("autocomplete") || "").toLowerCase();
    return ac.includes("current-password") || ac.includes("new-password");
  }

  function isOtpField(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    const ac = (el.getAttribute("autocomplete") || "").toLowerCase();
    if (ac.includes("one-time-code")) return true;
    const type = (el.type || "text").toLowerCase();
    if (!["text", "tel", "number", ""].includes(type)) return false;
    const meta = `${el.name || ""} ${el.id || ""} ${el.placeholder || ""} ${el.getAttribute("aria-label") || ""}`;
    return OTP_HINTS.test(meta) && !isPasswordField(el);
  }

  function isLikelyUsernameField(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    const type = (el.type || "text").toLowerCase();
    if (!["text", "email", "tel", ""].includes(type)) return false;
    const ac = (el.getAttribute("autocomplete") || "").toLowerCase();
    if (ac.includes("username") || ac === "email") return true;

    const meta = `${el.name || ""} ${el.id || ""} ${el.placeholder || ""} ${el.getAttribute("aria-label") || ""}`;
    if (SEARCH_OR_NON_LOGIN.test(meta) || OTP_HINTS.test(meta)) return false;

    const scope = el.form || el.closest("form, [role='dialog'], main, body") || document.body;
    const hasPwNearby = Boolean(scope.querySelector('input[type="password"]'));
    return hasPwNearby && (USERNAME_HINTS.test(meta) || type === "email" || meta.trim() === "");
  }

  function setNativeControlValue(control, value) {
    if (!control || value == null || value === "") return;
    if (control instanceof HTMLSelectElement) {
      const norm = String(value).trim().toLowerCase();
      const padded = norm.length === 1 && /^\d$/.test(norm) ? `0${norm}` : norm;
      const opt = Array.from(control.options).find((o) => {
        const v = o.value.trim().toLowerCase();
        const t = o.text.trim().toLowerCase();
        return v === norm || v === padded || t === norm || t.includes(norm);
      });
      if (opt) {
        control.value = opt.value;
        control.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
      }
      return;
    }
    control.focus({ preventScroll: true });
    const proto = Object.getPrototypeOf(control);
    const descriptor =
      Object.getOwnPropertyDescriptor(proto, "value") ||
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
    if (descriptor && descriptor.set) {
      descriptor.set.call(control, value);
    } else {
      control.value = value;
    }
    control.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, data: value }));
    control.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
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
        if (
          ["text", "email", "tel", ""].includes(t) &&
          !SEARCH_OR_NON_LOGIN.test(candidate.name || "") &&
          !OTP_HINTS.test(candidate.name || "")
        ) {
          if (!passwordEl.form || candidate.form === passwordEl.form) {
            usernameEl = candidate;
            break;
          }
        }
      }
    }

    const otpEl =
      (seedInput && isOtpField(seedInput) ? seedInput : null) ||
      pool.find(isOtpField) ||
      allInputs.find(isOtpField) ||
      null;

    return { usernameEl, passwordEl, otpEl };
  }

  function fillCardOrIdentity(kind, fields) {
    const controls = Array.from(document.querySelectorAll("input, select")).filter(isVisibleControl);
    const matchAndSet = (acList, regex, val) => {
      if (!val) return;
      let target = controls.find((el) => {
        const ac = (el.getAttribute("autocomplete") || "").toLowerCase();
        return acList.some((token) => ac === token || ac.endsWith(` ${token}`));
      });
      if (!target && regex) {
        target = controls.find((el) => {
          const meta = `${el.name || ""} ${el.id || ""} ${el.placeholder || ""} ${el.getAttribute("aria-label") || ""}`;
          return regex.test(meta);
        });
      }
      if (target) setNativeControlValue(target, val);
    };

    if (kind === "card") {
      matchAndSet(["cc-name"], /card.?holder|name.?on.?card|持卡人/i, fields.cardholderName);
      matchAndSet(["cc-number"], /card.?num|cc.?num|pan|卡号/i, fields.number);
      matchAndSet(["cc-exp-month"], /exp.?mon|cc.?mm|月份/i, fields.expMonth);
      matchAndSet(["cc-exp-year"], /exp.?year|cc.?yy|年份/i, fields.expYear);
      if (fields.expMonth && fields.expYear) {
        const yy = String(fields.expYear).slice(-2);
        const mm = String(fields.expMonth).padStart(2, "0");
        matchAndSet(["cc-exp"], /expiry|exp.?date|有效期/i, `${mm}/${yy}`);
      }
      matchAndSet(["cc-csc"], /cvv|cvc|csc|sec.?code|安全码/i, fields.code);
    } else {
      const fullName = [fields.firstName, fields.middleName, fields.lastName]
        .filter(Boolean)
        .join(" ")
        .trim();
      matchAndSet(["given-name"], /first.?name|fname|名/i, fields.firstName);
      matchAndSet(["family-name"], /last.?name|lname|surname|姓/i, fields.lastName);
      matchAndSet(["name"], /full.?name|your.?name|姓名|收货人/i, fullName);
      matchAndSet(["username"], /user.?name|用户名/i, fields.username);
      matchAndSet(["email"], /e-?mail|邮箱/i, fields.email);
      matchAndSet(["tel", "tel-national"], /phone|mobile|tel|电话|手机/i, fields.phone);
      matchAndSet(["organization"], /company|org|公司|单位/i, fields.company);
      matchAndSet(["address-line1", "street-address"], /addr.*1|street|详细地址|地址/i, fields.address1);
      matchAndSet(["address-line2"], /addr.*2|apt|suite/i, fields.address2);
      matchAndSet(["address-level2"], /city|城市/i, fields.city);
      matchAndSet(["address-level1"], /state|province|region|省|州/i, fields.state);
      matchAndSet(["postal-code"], /zip|postal|postcode|邮编/i, fields.postalCode);
      matchAndSet(["country", "country-name"], /country|国家/i, fields.country);
    }
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
      .bw-badge img { width: 15px; height: 15px; display: block; }
      .bw-popover, .bw-save-banner {
        position: fixed;
        border-radius: 10px;
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
        .bw-popover, .bw-save-banner {
          color: #18181B;
          background: rgba(255, 255, 255, 0.97);
          border-color: rgba(0, 0, 0, 0.1);
          box-shadow: 0 10px 28px rgba(0, 0, 0, 0.14);
        }
      }
      .bw-popover {
        min-width: 250px;
        max-width: 320px;
        padding: 6px;
      }
      .bw-save-banner {
        top: 16px;
        right: 16px;
        width: 300px;
        padding: 12px 14px;
        display: flex;
        flex-direction: column;
        gap: 9px;
      }
      .bw-banner-title { font-weight: 650; font-size: 13px; }
      .bw-banner-sub { font-size: 11.5px; opacity: 0.75; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .bw-banner-actions { display: flex; justify-content: flex-end; gap: 6px; }
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
      .bw-item:hover { background: rgba(139, 92, 246, 0.16); }
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
        padding: 6px 12px;
        font-size: 12px;
        font-weight: 600;
        color: #FFFFFF;
        background: #7C3AED;
        cursor: pointer;
      }
      .bw-action-btn:hover { background: #6D28D9; }
      .bw-ghost-btn {
        border: 1px solid rgba(128, 128, 138, 0.3);
        border-radius: 6px;
        padding: 6px 10px;
        font-size: 12px;
        color: inherit;
        background: transparent;
        cursor: pointer;
      }
    `;
    shadow.appendChild(style);
    const root = document.createElement("div");
    root.id = "bw-root";
    shadow.appendChild(root);
    const bannerRoot = document.createElement("div");
    bannerRoot.id = "bw-banner-root";
    shadow.appendChild(bannerRoot);
    (document.documentElement || document.body).appendChild(hostEl);
  }

  function hideOverlay() {
    menuOpen = false;
    if (shadow) {
      const root = shadow.getElementById("bw-root");
      if (root) root.replaceChildren();
    }
  }

  function hideSaveBanner() {
    if (shadow) {
      const br = shadow.getElementById("bw-banner-root");
      if (br) br.replaceChildren();
    }
  }

  function renderSaveBanner(host, username) {
    if (window !== window.top) return;
    ensureOverlay();
    const br = shadow.getElementById("bw-banner-root");
    if (!br) return;
    br.replaceChildren();

    const card = document.createElement("div");
    card.className = "bw-save-banner";
    const t = document.createElement("div");
    t.className = "bw-banner-title";
    t.textContent = "保存到 Bytewarden 保险库？";
    const sub = document.createElement("div");
    sub.className = "bw-banner-sub";
    sub.textContent = username ? `${host} · ${username}` : host;

    const actions = document.createElement("div");
    actions.className = "bw-banner-actions";
    const cancelBtn = document.createElement("button");
    cancelBtn.className = "bw-ghost-btn";
    cancelBtn.textContent = "忽略";
    cancelBtn.addEventListener("click", () => {
      hideSaveBanner();
      browser.runtime.sendMessage({ type: "bw:dismiss_save_banner" }).catch(() => {});
    });
    const saveBtn = document.createElement("button");
    saveBtn.className = "bw-action-btn";
    saveBtn.textContent = "保存登录项";
    saveBtn.addEventListener("click", async () => {
      saveBtn.textContent = "保存中…";
      await browser.runtime.sendMessage({ type: "bw:confirm_save_banner" }).catch(() => {});
      hideSaveBanner();
    });
    actions.append(cancelBtn, saveBtn);
    card.append(t, sub, actions);
    br.appendChild(card);
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
    const isDarkPage = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
    const iconImg = document.createElement("img");
    iconImg.src = isDarkPage ? MARK_LIGHT_URI : MARK_DARK_URI;
    iconImg.alt = "";
    btn.appendChild(iconImg);
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

  // Passive capture of login form submission to offer saving new credentials.
  document.addEventListener(
    "submit",
    (e) => {
      const form = e.target;
      if (!(form instanceof HTMLFormElement)) return;
      const { usernameEl, passwordEl } = findFormPair(form.querySelector('input[type="password"]'));
      if (passwordEl && passwordEl.value) {
        browser.runtime
          .sendMessage({
            type: "bw:form_submitted",
            url: location.href,
            title: document.title || "",
            username: usernameEl ? usernameEl.value.trim() : "",
            password: passwordEl.value,
          })
          .catch(() => {});
      }
    },
    true
  );

  document.addEventListener(
    "focusin",
    (e) => {
      const target = e.target;
      if (isLoginEligibleField(target)) {
        renderBadgeForInput(target);
      }
    },
    { capture: true, passive: true }
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
      hideSaveBanner();
      return undefined;
    }

    if (msg.type === "bw:show_save_banner") {
      renderSaveBanner(msg.host || location.hostname, msg.username || "");
      return undefined;
    }

    if (msg.type === "bw:perform_fill") {
      const seed =
        (document.activeElement instanceof HTMLInputElement ? document.activeElement : null) ||
        activeInput;
      const { usernameEl, passwordEl, otpEl } = findFormPair(seed);
      if (usernameEl && msg.username) {
        setNativeControlValue(usernameEl, msg.username);
      }
      if (passwordEl && msg.password) {
        setNativeControlValue(passwordEl, msg.password);
      } else if (!usernameEl && !otpEl && seed && isVisibleControl(seed) && msg.password) {
        setNativeControlValue(seed, msg.password);
      }
      if (otpEl && msg.totp) {
        setNativeControlValue(otpEl, msg.totp);
      }
      hideOverlay();
      return Promise.resolve({ ok: true });
    }

    if (msg.type === "bw:perform_profile_fill") {
      fillCardOrIdentity(msg.kind, msg.fields || {});
      hideOverlay();
      return Promise.resolve({ ok: true });
    }

    if (msg.type === "bw:fill_generated_password") {
      const target =
        (document.activeElement instanceof HTMLInputElement ? document.activeElement : null) ||
        activeInput;
      if (target && isVisibleControl(target) && msg.password) {
        setNativeControlValue(target, msg.password);
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
