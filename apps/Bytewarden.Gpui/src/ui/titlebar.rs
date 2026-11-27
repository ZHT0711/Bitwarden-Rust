//! Custom integrated Windows 11 Titlebar (44px) with centered Search/Command bar,
//! live spinning sync status, account avatar pill, and native WindowControlArea chrome.

use std::time::Instant;

use bytewarden_core::desktop::Session;
use gpui::{Context, Hsla, MouseButton, Stateful, Window, WindowControlArea, div, prelude::*, px};

use crate::{
    Page, VaultPreview,
    theme::{self, hex},
};

use super::{
    components::{avatar, badge, glyph_icon, icon_btn, make_tooltip, sync_spinner},
    tokens::{family, font, glyph, radius, size, space},
};

impl VaultPreview {
    pub fn render_titlebar(
        &self,
        minimal_navigation: bool,
        compact_chrome: bool,
        now: Instant,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let pal = theme::palette();
        let session = self.client.state();
        let active_account = self.client.active();
        let is_maximized = window.is_maximized();
        let is_busy = self.client.busy.is_some();
        let is_read_only = self
            .client
            .overview
            .as_ref()
            .is_some_and(|overview| overview.read_only);
        let search_active = self.page == Page::Vault
            && self.client.overview.is_some()
            && self.client.form.is_none()
            && self.selection.pending.is_none();

        let spin_angle = if is_busy && self.animate {
            now.saturating_duration_since(self.ui.sync_spin_start)
                .as_secs_f32()
                * std::f32::consts::TAU
                * 1.35
        } else {
            0.
        };

        let server_badge = active_account.map(|acc| match acc.server_mode {
            0 => "US",
            1 => "EU",
            _ => "自",
        });

        let left = div()
            .flex()
            .items_center()
            .gap(px(space::S))
            .pl(px(space::S))
            .flex_shrink_0()
            .when(minimal_navigation, |el| {
                el.child(
                    icon_btn("titlebar-menu", glyph::MENU, "打开导航菜单", Some("Ctrl+B"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(6.))
                    .h(px(size::TITLEBAR))
                    .window_control_area(WindowControlArea::Drag)
                    .child(super::brand::brand_logo(22.))
                    .when(!compact_chrome, |el| {
                        el.child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(hex(pal.text))
                                .child("Bytewarden"),
                        )
                    })
                    .when(is_read_only && !compact_chrome, |el| {
                        el.child(badge("离线只读", pal.warning))
                    }),
            );

        let ultra_compact = window.viewport_size().width < px(560.);
        let has_search_text =
            !self.query.is_empty() || !self.client.search.read(cx).value().is_empty();
        // Center Search + Command Palette trigger pill.
        let search_pill = if search_active {
            div()
                .w_full()
                .max_w(px(if compact_chrome { 300. } else { 460. }))
                .h(px(30.))
                .rounded(px(radius::CONTROL))
                .bg(hex(pal.input))
                .border_1()
                .border_color(hex(pal.border))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_color(hex(pal.text_muted))
                        .child(glyph_icon(glyph::SEARCH, 13.)),
                )
                .child(div().flex_1().min_w_0().child(self.client.search.clone()))
                .when(has_search_text, |box_el| {
                    box_el.child(
                        div()
                            .id("clear-search-pill")
                            .cursor_pointer()
                            .size(px(20.))
                            .rounded(px(radius::SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(pal.text_muted))
                            .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                            .child(glyph_icon(glyph::CLOSE, 10.))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.client.search.update(cx, |input, cx| input.set("", cx));
                                    this.query.clear();
                                    this.invalidate_query();
                                    this.client.search.read(cx).focus(window);
                                    cx.notify();
                                }),
                            ),
                    )
                })
                .when(!ultra_compact, |box_el| {
                    box_el.child(
                        div()
                            .id("open-cmd-palette-chip")
                            .cursor_pointer()
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(radius::SM))
                            .bg(hex(pal.layer))
                            .border_1()
                            .border_color(hex(pal.border_subtle))
                            .text_size(px(10.5))
                            .font_family(family::MONO)
                            .text_color(hex(pal.text_secondary))
                            .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                            .child("Ctrl+K")
                            .tooltip(|_, cx| make_tooltip("打开命令面板", Some("Ctrl+K"), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_command_palette(window, cx);
                            })),
                    )
                })
                .into_any_element()
        } else {
            div()
                .id("titlebar-palette-trigger")
                .cursor_pointer()
                .w_full()
                .max_w(px(if compact_chrome { 280. } else { 460. }))
                .h(px(30.))
                .rounded(px(radius::CONTROL))
                .bg(hex(pal.input))
                .border_1()
                .border_color(hex(pal.border))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .hover(move |s| s.bg(hex(pal.hover)))
                .child(
                    div()
                        .text_color(hex(pal.text_muted))
                        .child(glyph_icon(glyph::SEARCH, 13.)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(font::CAPTION))
                        .text_color(hex(pal.text_muted))
                        .child(if ultra_compact {
                            "搜索…"
                        } else {
                            "搜索保险库或命令…"
                        }),
                )
                .when(!ultra_compact, |box_el| {
                    box_el.child(
                        div()
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(radius::SM))
                            .bg(hex(pal.layer))
                            .border_1()
                            .border_color(hex(pal.border_subtle))
                            .text_size(px(10.5))
                            .font_family(family::MONO)
                            .text_color(hex(pal.text_secondary))
                            .child("Ctrl+K"),
                    )
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_command_palette(window, cx);
                }))
                .into_any_element()
        };

        let center = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .px(px(space::M))
            .window_control_area(WindowControlArea::Drag)
            .when(self.client.form.is_none(), |el| el.child(search_pill));

        // Right action cluster + native Windows 11 caption buttons.
        let sync_tooltip: gpui::SharedString = if let Some(busy_label) = self.client.busy {
            format!("{busy_label}…").into()
        } else if self.client.reconnect_available() {
            "重新连接账户并同步".into()
        } else if is_read_only {
            "离线只读 · 点击重新同步".into()
        } else if let Some(last) = self
            .client
            .overview
            .as_ref()
            .map(|o| o.last_sync.as_str())
            .filter(|s| !s.is_empty())
        {
            format!("同步保险库 · 上次同步 {last}").into()
        } else {
            "同步保险库".into()
        };

        let status_dot_color = if is_read_only {
            Some(pal.warning)
        } else if self.client.overview.is_some() {
            Some(pal.success)
        } else {
            None
        };

        let mut actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .pr(px(space::S))
            .flex_shrink_0();

        if is_busy {
            actions = actions.child(
                div()
                    .id("sync-busy-indicator")
                    .h(px(size::ICON_BTN))
                    .px(px(8.))
                    .rounded(px(radius::CONTROL))
                    .bg(pal.tint(pal.accent_fill))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(sync_spinner(spin_angle, 14., hex(pal.accent_text).into()))
                    .when(!compact_chrome, |el| {
                        el.child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.accent_text))
                                .child(self.client.busy.unwrap_or("处理中")),
                        )
                    }),
            );
        } else if (self.client.sync_available() || self.client.reconnect_available())
            && self.client.form.is_none()
            && self.selection.pending.is_none()
        {
            actions = actions.child(
                div()
                    .id("titlebar-sync")
                    .cursor_pointer()
                    .relative()
                    .size(px(size::ICON_BTN))
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hex(pal.text_secondary))
                    .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                    .active(move |s| s.bg(hex(pal.selected)).top(px(1.)))
                    .child(glyph_icon(
                        if is_read_only {
                            glyph::CLOUD_OFF
                        } else {
                            glyph::SYNC
                        },
                        14.,
                    ))
                    .when_some(status_dot_color, |el, dot| {
                        el.child(
                            div()
                                .absolute()
                                .bottom(px(4.))
                                .right(px(4.))
                                .size(px(6.))
                                .rounded(px(radius::PILL))
                                .bg(hex(dot)),
                        )
                    })
                    .tooltip(move |_, cx| make_tooltip(sync_tooltip.clone(), Some("Ctrl+R"), cx))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.sync_or_reconnect(window, cx)),
                    ),
            );
        }

        if session == Session::Unlocked && self.client.form.is_none() {
            actions = actions.child(
                icon_btn("titlebar-lock", glyph::LOCK, "锁定保险库", Some("Ctrl+L"))
                    .on_click(cx.listener(|this, _, _, cx| this.request_lock(cx))),
            );
        }

        actions = actions.child(
            icon_btn(
                "titlebar-shortcuts",
                glyph::KEYBOARD,
                "快捷键指南",
                Some("Ctrl+/"),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_shortcuts_modal(cx))),
        );

        // Account avatar pill.
        let account_pill = if let Some(account) = active_account {
            div()
                .id("account-menu")
                .cursor_pointer()
                .h(px(30.))
                .pl(px(4.))
                .pr(px(if compact_chrome { 4. } else { 10. }))
                .rounded(px(radius::PILL))
                .bg(if self.client.show_accounts {
                    hex(pal.selected)
                } else {
                    hex(pal.card)
                })
                .border_1()
                .border_color(hex(if self.client.show_accounts {
                    pal.accent_fill
                } else {
                    pal.border
                }))
                .flex()
                .items_center()
                .gap(px(7.))
                .hover(move |s| s.bg(hex(pal.hover)))
                .child(avatar(&account.email, server_badge, 22.))
                .when(!compact_chrome, |el| {
                    el.child(
                        div()
                            .max_w(px(140.))
                            .truncate()
                            .text_size(px(font::CAPTION))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(hex(pal.text))
                            .child(account.email.clone()),
                    )
                })
                .when(!self.client.show_accounts, |el| {
                    el.tooltip(|_, cx| make_tooltip("切换或管理账户", None, cx))
                })
        } else {
            div()
                .id("account-menu")
                .cursor_pointer()
                .h(px(28.))
                .px(px(10.))
                .rounded(px(radius::PILL))
                .bg(hex(pal.accent_fill))
                .text_color(hex(pal.on_accent))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(font::CAPTION))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(glyph_icon(glyph::PERSON, 12.))
                .child("登录")
        };

        actions = actions.child(account_pill.on_click(cx.listener(|this, _, window, cx| {
            if this.client.ready() && this.client.form.is_none() && this.selection.pending.is_none()
            {
                if this.client.accounts.is_empty() {
                    this.begin_sign_in(window, cx);
                } else {
                    this.ui.close_popovers(this.animate);
                    this.client.show_accounts = !this.client.show_accounts;
                    cx.notify();
                }
            }
        })));

        // Native Windows 11 window control buttons (Min / Max-Restore / Close).
        let close_hover: Hsla = hex(0xc42b1c).into();
        let caption_buttons = div()
            .flex()
            .items_center()
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .id("win-min")
                    .window_control_area(WindowControlArea::Min)
                    .w(px(46.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hex(pal.text_secondary))
                    .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                    .child(glyph_icon(glyph::CHROME_MIN, 10.)),
            )
            .child(
                div()
                    .id("win-max")
                    .window_control_area(WindowControlArea::Max)
                    .w(px(46.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hex(pal.text_secondary))
                    .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                    .child(glyph_icon(
                        if is_maximized {
                            glyph::CHROME_RESTORE
                        } else {
                            glyph::CHROME_MAX
                        },
                        10.,
                    )),
            )
            .child(
                div()
                    .id("win-close")
                    .window_control_area(WindowControlArea::Close)
                    .w(px(46.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hex(pal.text_secondary))
                    .hover(move |s| s.bg(close_hover).text_color(hex(0xffffff)))
                    .child(glyph_icon(glyph::CHROME_CLOSE, 10.)),
            );

        div()
            .id("custom-titlebar")
            .w_full()
            .h(px(size::TITLEBAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.ui.item_menu_open || this.ui.new_menu_open {
                        this.ui.close_popovers(this.animate);
                        cx.notify();
                    }
                }),
            )
            .child(left)
            .child(center)
            .child(actions)
            .child(caption_buttons)
    }
}
