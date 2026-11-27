//! Windows native WebAuthn Passkey Plugin Provider integration for Bytewarden GPUI.
//! Registers the in-process COM server provided by `bytewarden-core`, polls with a
//! single zero-render background timer (`500ms`), and presents a centered Fluent
//! approval modal anchored to Windows Hello whenever a browser or app requests a passkey.

#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use bytewarden_core::desktop::{self, PasskeyRequest, Session};
#[cfg(windows)]
use gpui::{Context, Div, Entity, Task, Window, div, prelude::*, px};

#[cfg(not(windows))]
use gpui::{Context, Div, Window, div};

use crate::VaultPreview;
#[cfg(windows)]
use crate::input::Input;

#[cfg(windows)]
struct ActivePasskeyModal {
    request: PasskeyRequest,
    /// `None` means "create new login item" (valid when `request.creating == true`).
    selected_choice: Option<usize>,
    password: Entity<Input>,
    busy: bool,
    status: Option<String>,
}

#[derive(Default)]
pub struct PasskeyState {
    pub provider_state: String,
    pub registering: bool,
    pub syncing_metadata: bool,
    #[cfg(windows)]
    active: Option<ActivePasskeyModal>,
    #[cfg(windows)]
    checking: bool,
    #[cfg(windows)]
    watcher: Option<Task<()>>,
}

impl PasskeyState {
    pub fn has_modal(&self) -> bool {
        #[cfg(windows)]
        {
            self.active.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    pub fn state_label(&self) -> (&'static str, &'static str) {
        match self.provider_state.as_str() {
            "enabled" => (
                "已启用",
                "Windows 已启用 Bytewarden 作为系统通行密钥提供程序，浏览器与应用可直接原生调用。",
            ),
            "disabled" => (
                "需在系统开启",
                "已向 Windows 注册，但尚未在「Windows 设置 → 账户 → 通行密钥」中开启 Bytewarden。",
            ),
            "notRegistered" | "unregistered" => (
                "未注册",
                "点击右侧「注册到 Windows」后，即可在系统通行密钥设置中启用 Bytewarden。",
            ),
            "unsupported" | "unavailable" => (
                "不可用",
                "当前运行环境暂不支持系统通行密钥（需通过 MSIX 应用包运行于 Windows 11 24H2/25H2 及以上）。",
            ),
            _ => ("检测中", "正在读取 Windows 通行密钥提供程序状态…"),
        }
    }
}

impl VaultPreview {
    #[cfg(windows)]
    pub(crate) fn start_passkey_provider(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        let clock = bg.clone();
        cx.spawn(async move |weak, app| {
            let state = bg
                .spawn(async {
                    let _ = desktop::start_passkey_provider();
                    desktop::passkey_provider_state().unwrap_or_default()
                })
                .await;
            let _ = weak.update(app, |this, cx| {
                this.passkeys.provider_state = state;
                cx.notify();
            });
        })
        .detach();

        // Single lightweight 500ms background poll: zero UI thread wakes while idle.
        self.passkeys.watcher = Some(cx.spawn_in(window, async move |weak, app| {
            loop {
                clock.timer(Duration::from_millis(500)).await;
                let has_pending = desktop::has_pending_passkey();
                if !has_pending {
                    let still_alive = weak
                        .update(app, |this, cx| {
                            if let Some(modal) = &this.passkeys.active
                                && !modal.busy
                            {
                                let id = modal.request.id.clone();
                                if !desktop::passkey_is_active(&id).unwrap_or(false) {
                                    this.clear_passkey_modal(false, cx);
                                }
                            }
                            !this.closing
                        })
                        .unwrap_or(false);
                    if !still_alive {
                        break;
                    }
                    continue;
                }
                let proceed = weak
                    .update(app, |this, cx| {
                        if this.closing || this.passkeys.checking || this.passkeys.active.is_some()
                        {
                            return false;
                        }
                        if this.client.form.is_some() || this.client.editor.is_some() {
                            return false;
                        }
                        this.passkeys.checking = true;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !proceed {
                    continue;
                }
                let fetched = clock.spawn(async { desktop::pending_passkey() }).await;
                let _ = weak.update_in(app, |this, window, cx| {
                    this.passkeys.checking = false;
                    if this.closing {
                        return;
                    }
                    let Ok(request) = fetched else {
                        return;
                    };
                    if request.id.is_empty() {
                        return;
                    }
                    crate::tray::restore_window_to_front(window);
                    window.activate_window();

                    if request.blocked_reason.as_deref() == Some("ownVault") {
                        this.open_passkey_modal(request, cx);
                        return;
                    }
                    match request.session() {
                        Session::SignedOut => {
                            this.ui.toast.show(
                                crate::ui::toast::ToastKind::Info,
                                "收到 Windows 通行密钥请求，请先登录保险库",
                                std::time::Instant::now(),
                                this.animate,
                            );
                            this.begin_sign_in(window, cx);
                            return;
                        }
                        Session::Locked => {
                            this.ui.toast.show(
                                crate::ui::toast::ToastKind::Info,
                                "收到 Windows 通行密钥请求，请先解锁保险库",
                                std::time::Instant::now(),
                                this.animate,
                            );
                            this.open_unlock(window, cx);
                            return;
                        }
                        Session::Unlocked => {}
                    }
                    if request.needs_sync {
                        if this.client.sync_available() {
                            this.ui.toast.show(
                                crate::ui::toast::ToastKind::Info,
                                "收到 Windows 通行密钥请求，正在同步保险库…",
                                std::time::Instant::now(),
                                this.animate,
                            );
                            this.request_sync(cx);
                        }
                        return;
                    }
                    this.open_passkey_modal(request, cx);
                });
            }
        }));
    }

    #[cfg(not(windows))]
    pub(crate) fn start_passkey_provider(&mut self, _: &Window, _: &mut Context<Self>) {}

    #[cfg(windows)]
    fn open_passkey_modal(&mut self, request: PasskeyRequest, cx: &mut Context<Self>) {
        let selected_choice = if request.creating {
            None
        } else if !request.choices.is_empty() {
            Some(0)
        } else {
            None
        };
        let password = cx.new(|cx| Input::new("输入主密码以复验此条目", true, cx));
        self.passkeys.active = Some(ActivePasskeyModal {
            request,
            selected_choice,
            password,
            busy: false,
            status: None,
        });
        cx.notify();
    }

    pub(crate) fn clear_passkey_modal(&mut self, cancel_core: bool, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            if let Some(modal) = self.passkeys.active.take() {
                modal.password.update(cx, |input, cx| input.set("", cx));
                if cancel_core {
                    let id = modal.request.id;
                    if modal.request.blocked_reason.as_deref() == Some("ownVault") {
                        let _ = desktop::dismiss_passkey_refusal(&id);
                    } else {
                        let _ = desktop::cancel_passkey(&id);
                    }
                }
                cx.notify();
            } else if cancel_core {
                let _ = desktop::cancel_passkey("");
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (cancel_core, cx);
        }
    }

    pub(crate) fn stop_passkey_provider(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            self.passkeys.watcher = None;
            self.clear_passkey_modal(true, cx);
        }
        #[cfg(not(windows))]
        {
            let _ = cx;
        }
    }

    pub(crate) fn register_windows_passkey_provider(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            if self.passkeys.registering {
                return;
            }
            self.passkeys.registering = true;
            cx.notify();
            let bg = cx.background_executor().clone();
            cx.spawn(async move |weak, app| {
                let result = bg
                    .spawn(async {
                        let reg = desktop::register_passkey_provider();
                        let state = desktop::passkey_provider_state().unwrap_or_default();
                        (reg, state)
                    })
                    .await;
                let _ = weak.update(app, |this, cx| {
                    this.passkeys.registering = false;
                    this.passkeys.provider_state = result.1;
                    match result.0 {
                        Ok(()) => {
                            this.ui.toast.show(
                                crate::ui::toast::ToastKind::Success,
                                "已注册为 Windows 通行密钥提供程序，可在系统设置中开启",
                                std::time::Instant::now(),
                                this.animate,
                            );
                            if this.passkeys.provider_state == "enabled" {
                                this.sync_windows_passkey_metadata(true, cx);
                            }
                        }
                        Err(err) => {
                            let detail = crate::client::status_text(&err);
                            this.client.notice = detail.clone();
                            this.ui.toast.show(
                                crate::ui::toast::ToastKind::Error,
                                format!("注册失败：{detail}"),
                                std::time::Instant::now(),
                                this.animate,
                            );
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        #[cfg(not(windows))]
        {
            let _ = cx;
        }
    }

    pub(crate) fn refresh_windows_passkey_state(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            let bg = cx.background_executor().clone();
            cx.spawn(async move |weak, app| {
                let state = bg
                    .spawn(async { desktop::passkey_provider_state().unwrap_or_default() })
                    .await;
                let _ = weak.update(app, |this, cx| {
                    if this.passkeys.provider_state != state {
                        this.passkeys.provider_state = state;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        #[cfg(not(windows))]
        {
            let _ = cx;
        }
    }

    pub(crate) fn sync_windows_passkey_metadata(&mut self, quiet: bool, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            if self.passkeys.syncing_metadata {
                return;
            }
            if quiet && self.passkeys.provider_state != "enabled" {
                return;
            }
            self.passkeys.syncing_metadata = true;
            if !quiet {
                cx.notify();
            }
            let bg = cx.background_executor().clone();
            cx.spawn(async move |weak, app| {
                let res = bg.spawn(async { desktop::sync_passkey_metadata() }).await;
                let _ = weak.update(app, |this, cx| {
                    this.passkeys.syncing_metadata = false;
                    if !quiet {
                        match res {
                            Ok(true) => this.ui.toast.show(
                                crate::ui::toast::ToastKind::Success,
                                "已同步通行密钥元数据到 Windows",
                                std::time::Instant::now(),
                                this.animate,
                            ),
                            Ok(false) => this.ui.toast.show(
                                crate::ui::toast::ToastKind::Info,
                                "请先解锁并确保已在 Windows 设置中开启通行密钥提供程序",
                                std::time::Instant::now(),
                                this.animate,
                            ),
                            Err(err) => {
                                this.client.notice = crate::client::status_text(&err);
                            }
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        #[cfg(not(windows))]
        {
            let _ = (quiet, cx);
        }
    }

    #[cfg(windows)]
    fn submit_passkey_approval(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(hwnd) = crate::tray::window_hwnd_usize(window) else {
            return;
        };
        let Some(modal) = self.passkeys.active.as_mut() else {
            return;
        };
        if modal.busy {
            return;
        }
        let choice = modal
            .selected_choice
            .and_then(|idx| modal.request.choices.get(idx))
            .cloned();
        if !modal.request.creating && choice.is_none() {
            return;
        }
        let needs_password = choice.as_ref().is_some_and(|c| c.reprompt);
        let password = modal.password.update(cx, |input, cx| input.take(cx));
        if needs_password && password.is_empty() {
            modal.status = Some("此条目需要输入主密码复验。".into());
            cx.notify();
            return;
        }
        modal.busy = true;
        modal.status = Some("正在通过 Windows Hello 验证并处理通行密钥请求…".into());
        let request_id = modal.request.id.clone();
        let creating = modal.request.creating;
        cx.notify();

        let bg = cx.background_executor().clone();
        cx.spawn(async move |weak, app| {
            let res = bg
                .spawn(async move {
                    desktop::respond_to_passkey(&request_id, choice.as_ref(), &password, hwnd)
                })
                .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(()) => {
                        this.clear_passkey_modal(false, cx);
                        this.ui.toast.show(
                            crate::ui::toast::ToastKind::Success,
                            if creating {
                                "通行密钥已验证并保存到保险库"
                            } else {
                                "已通过通行密钥完成登录验证"
                            },
                            std::time::Instant::now(),
                            this.animate,
                        );
                        if creating && this.client.sync_available() {
                            this.request_sync(cx);
                        }
                    }
                    Err(err) => {
                        if let Some(modal) = this.passkeys.active.as_mut() {
                            modal.busy = false;
                            modal.status = Some(if err.code() == "permissionDenied" {
                                "Windows Hello 验证未完成或条目权限不足，请关闭后重试。".into()
                            } else {
                                crate::client::status_text(&err)
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(windows)]
    pub(crate) fn render_passkey_modal(&self, cx: &Context<Self>) -> Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{BtnVariant, btn, elevated_card, glyph_icon, info_bar},
            toast::ToastKind,
            tokens::{font, glyph, radius, space},
        };
        let Some(modal) = &self.passkeys.active else {
            return div();
        };
        let pal = theme::palette();
        let is_refusal = modal.request.blocked_reason.as_deref() == Some("ownVault");
        let rp = modal
            .request
            .relying_party
            .as_deref()
            .unwrap_or("未知网站")
            .to_owned();
        let selected_choice = modal
            .selected_choice
            .and_then(|idx| modal.request.choices.get(idx));
        let needs_password = selected_choice.is_some_and(|c| c.reprompt);
        let can_submit = !modal.busy && (modal.request.creating || selected_choice.is_some());

        let mut card = elevated_card()
            .p(px(20.))
            .w_full()
            .max_w(px(460.))
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(36.))
                            .rounded(px(radius::PILL))
                            .bg(pal.tint(if is_refusal {
                                pal.warning
                            } else {
                                pal.accent_fill
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(if is_refusal {
                                pal.warning
                            } else {
                                pal.accent_text
                            }))
                            .child(glyph_icon(glyph::PASSKEY, 16.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(font::BODY_LG))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(if is_refusal {
                                        "无法保存此保险库的通行密钥"
                                    } else if modal.request.creating {
                                        "保存通行密钥"
                                    } else {
                                        "使用通行密钥登录"
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child(rp),
                            ),
                    ),
            );

        if is_refusal {
            card = card
                .child(info_bar(
                    ToastKind::Warning,
                    "不能把登录当前保险库的通行密钥存回它自己：保险库锁定或退出后可能无法再登录。请在浏览器中选择 Windows Hello 或其他安全密钥。",
                ))
                .child(
                    div().flex().justify_end().child(
                        btn("passkey-refusal-ok", BtnVariant::Primary)
                            .h(px(36.))
                            .px(px(18.))
                            .child("知道了")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clear_passkey_modal(true, cx);
                            })),
                    ),
                );
            return card;
        }

        if let Some(user) = modal.request.user_name.as_deref().filter(|s| !s.is_empty()) {
            card = card.child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(radius::CONTROL))
                    .bg(hex(pal.layer))
                    .border_1()
                    .border_color(hex(pal.border_subtle))
                    .text_size(px(font::CAPTION))
                    .text_color(hex(pal.text))
                    .child(format!("账户：{user}")),
            );
        }

        let mut choice_list = div()
            .id("passkey-choice-list")
            .flex()
            .flex_col()
            .gap(px(4.))
            .max_h(px(200.))
            .overflow_y_scroll();

        if modal.request.creating {
            let active = modal.selected_choice.is_none();
            choice_list = choice_list.child(
                div()
                    .id("passkey-choice-new")
                    .cursor_pointer()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(radius::CONTROL))
                    .border_1()
                    .border_color(if active {
                        pal.tint_border(pal.accent_fill)
                    } else {
                        hex(pal.border_subtle).into()
                    })
                    .bg(if active {
                        pal.tint(pal.accent_fill)
                    } else {
                        hex(pal.layer).into()
                    })
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(glyph_icon(glyph::ADD, 13.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(hex(pal.text))
                            .child("新建登录条目并保存"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(modal) = this.passkeys.active.as_mut()
                            && !modal.busy
                        {
                            modal.selected_choice = None;
                            cx.notify();
                        }
                    })),
            );
        }

        for (i, choice) in modal.request.choices.iter().enumerate() {
            let active = modal.selected_choice == Some(i);
            let sub = choice.user_name.clone().unwrap_or_default();
            choice_list = choice_list.child(
                div()
                    .id(("passkey-choice", i))
                    .cursor_pointer()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(radius::CONTROL))
                    .border_1()
                    .border_color(if active {
                        pal.tint_border(pal.accent_fill)
                    } else {
                        hex(pal.border_subtle).into()
                    })
                    .bg(if active {
                        pal.tint(pal.accent_fill)
                    } else {
                        hex(pal.layer).into()
                    })
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(hex(pal.text))
                            .child(choice.name.clone()),
                    )
                    .when(!sub.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child(sub),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(modal) = this.passkeys.active.as_mut()
                            && !modal.busy
                        {
                            modal.selected_choice = Some(i);
                            cx.notify();
                        }
                    })),
            );
        }

        card = card
            .child(choice_list)
            .when(needs_password, |el| el.child(modal.password.clone()))
            .when_some(modal.status.as_ref(), |el, msg| {
                el.child(info_bar(ToastKind::Info, msg.clone()))
            })
            .child(
                div()
                    .text_size(px(font::MICRO))
                    .text_color(hex(pal.text_muted))
                    .child("点击确认后将唤起 Windows Hello 完成身份验证。"),
            )
            .child(
                div()
                    .pt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        btn("passkey-approve", BtnVariant::Primary)
                            .flex_1()
                            .h(px(36.))
                            .when(!can_submit, |b| b.opacity(0.45))
                            .child(if modal.request.creating {
                                "验证并保存"
                            } else {
                                "验证并登录"
                            })
                            .when(can_submit, |b| {
                                b.on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_passkey_approval(window, cx);
                                }))
                            }),
                    )
                    .child(
                        btn("passkey-cancel", BtnVariant::Secondary)
                            .h(px(36.))
                            .px(px(16.))
                            .child("取消")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clear_passkey_modal(true, cx);
                            })),
                    ),
            );
        card
    }

    #[cfg(not(windows))]
    pub(crate) fn render_passkey_modal(&self, _: &Context<Self>) -> Div {
        div()
    }
}
