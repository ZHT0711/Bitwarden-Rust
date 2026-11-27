use std::{fs, io};

use gpui::{ClipboardItem, Context, Window, div, prelude::*, px};

use crate::{
    VaultPreview,
    client::profile_directory,
    theme::{self, Theme},
    vault_list::Sort,
};

pub struct Settings {
    pub theme: Theme,
    pub animations: bool,
    pub clipboard_seconds: u64,
    pub auto_prompt_hello: bool,
    pub prevent_screenshots: bool,
    pub browser_integration: bool,
    pub browser_biometric_gate: bool,
    pub sort: Sort,
    pub website_icons: bool,
    pub keep_in_tray: bool,
    pub start_hidden: bool,
    pub start_at_login: bool,
    pub minimize_after_copy: bool,
    pub list_width: f32,
    pub window_x: Option<f32>,
    pub window_y: Option<f32>,
    pub window_w: Option<f32>,
    pub window_h: Option<f32>,
    pub window_maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            animations: true,
            clipboard_seconds: 30,
            auto_prompt_hello: false,
            prevent_screenshots: false,
            browser_integration: true,
            browser_biometric_gate: false,
            sort: Sort::default(),
            website_icons: true,
            keep_in_tray: true,
            start_hidden: true,
            start_at_login: false,
            minimize_after_copy: false,
            list_width: crate::ui::tokens::size::LIST_DEFAULT,
            window_x: None,
            window_y: None,
            window_w: None,
            window_h: None,
            window_maximized: false,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut settings = match profile_directory()
            .and_then(|root| fs::read_to_string(root.join("appearance.conf")))
        {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        };
        settings.start_at_login = crate::startup::is_enabled();
        settings
    }

    fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "theme" => settings.theme = Theme::parse(value),
                "animations" => settings.animations = value != "false",
                "sort" => settings.sort = Sort::parse(value),
                "auto_prompt_hello" => settings.auto_prompt_hello = value == "true",
                "prevent_screenshots" => settings.prevent_screenshots = value == "true",
                "browser_integration" => settings.browser_integration = value != "false",
                "browser_biometric_gate" => settings.browser_biometric_gate = value == "true",
                "website_icons" => settings.website_icons = value != "false",
                "keep_in_tray" => settings.keep_in_tray = value != "false",
                "start_hidden" => settings.start_hidden = value != "false",
                "minimize_after_copy" => settings.minimize_after_copy = value == "true",
                "window_maximized" => settings.window_maximized = value == "true",
                "window_x" => {
                    if let Ok(v) = value.parse::<f32>()
                        && v.is_finite()
                        && (-16_000.0..=16_000.0).contains(&v)
                    {
                        settings.window_x = Some(v);
                    }
                }
                "window_y" => {
                    if let Ok(v) = value.parse::<f32>()
                        && v.is_finite()
                        && (-16_000.0..=16_000.0).contains(&v)
                    {
                        settings.window_y = Some(v);
                    }
                }
                "window_w" => {
                    if let Ok(v) = value.parse::<f32>()
                        && v.is_finite()
                    {
                        settings.window_w = Some(v.clamp(420., 3840.));
                    }
                }
                "window_h" => {
                    if let Ok(v) = value.parse::<f32>()
                        && v.is_finite()
                    {
                        settings.window_h = Some(v.clamp(410., 2160.));
                    }
                }
                "clipboard_seconds" => {
                    if let Ok(seconds @ (10 | 30 | 60 | 120)) = value.parse() {
                        settings.clipboard_seconds = seconds;
                    }
                }
                "list_width" => {
                    if let Ok(width) = value.parse::<f32>()
                        && width.is_finite()
                    {
                        settings.list_width = width.clamp(
                            crate::ui::tokens::size::LIST_MIN,
                            crate::ui::tokens::size::LIST_MAX,
                        );
                    }
                }
                _ => {}
            }
        }
        settings
    }

    pub fn save_silent(&self) {
        let _ = self.save();
    }

    pub fn capture_window_bounds(&mut self, window: &Window) {
        let maximized = window.is_maximized();
        self.window_maximized = maximized;
        if !maximized {
            let bounds = window.bounds();
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let x = f32::from(bounds.origin.x);
            let y = f32::from(bounds.origin.y);
            if w >= 420. && h >= 410. && x.is_finite() && y.is_finite() {
                self.window_x = Some(x);
                self.window_y = Some(y);
                self.window_w = Some(w.clamp(420., 3840.));
                self.window_h = Some(h.clamp(410., 2160.));
            }
        }
    }

    /// Compute startup `WindowBounds`, restoring saved geometry only if it remains
    /// clearly visible on at least one currently connected display.
    pub fn initial_window_bounds(&self, cx: &gpui::App) -> gpui::WindowBounds {
        let default_size = gpui::size(
            px(crate::ui::tokens::size::WINDOW_DEFAULT_W),
            px(crate::ui::tokens::size::WINDOW_DEFAULT_H),
        );
        let desired_size = match (self.window_w, self.window_h) {
            (Some(w), Some(h)) => gpui::size(px(w.clamp(420., 3840.)), px(h.clamp(410., 2160.))),
            _ => default_size,
        };
        let bounds = match (self.window_x, self.window_y) {
            (Some(x), Some(y)) => {
                let candidate = gpui::Bounds::new(gpui::point(px(x), px(y)), desired_size);
                let visible_on_any_display = cx.displays().iter().any(|display| {
                    let db = display.bounds();
                    let ix1 = f32::from(candidate.left()).max(f32::from(db.left()));
                    let iy1 = f32::from(candidate.top()).max(f32::from(db.top()));
                    let ix2 = f32::from(candidate.right()).min(f32::from(db.right()));
                    let iy2 = f32::from(candidate.bottom()).min(f32::from(db.bottom()));
                    (ix2 - ix1) >= 120. && (iy2 - iy1) >= 80.
                });
                if visible_on_any_display {
                    candidate
                } else {
                    gpui::Bounds::centered(None, desired_size, cx)
                }
            }
            _ => gpui::Bounds::centered(None, desired_size, cx),
        };
        if self.window_maximized {
            gpui::WindowBounds::Maximized(bounds)
        } else {
            gpui::WindowBounds::Windowed(bounds)
        }
    }

    fn save(&self) -> io::Result<()> {
        let root = profile_directory()?;
        fs::create_dir_all(&root)?;
        let path = root.join("appearance.conf");
        // Only non-sensitive application preferences are stored here.
        fs::write(path, self.serialized())
    }

    fn serialized(&self) -> String {
        let mut out = format!(
            "theme={}\nanimations={}\nclipboard_seconds={}\nauto_prompt_hello={}\nprevent_screenshots={}\nbrowser_integration={}\nbrowser_biometric_gate={}\nsort={}\nwebsite_icons={}\nkeep_in_tray={}\nstart_hidden={}\nminimize_after_copy={}\nlist_width={:.0}\nwindow_maximized={}\n",
            self.theme.key(),
            self.animations,
            self.clipboard_seconds,
            self.auto_prompt_hello,
            self.prevent_screenshots,
            self.browser_integration,
            self.browser_biometric_gate,
            self.sort.key(),
            self.website_icons,
            self.keep_in_tray,
            self.start_hidden,
            self.minimize_after_copy,
            self.list_width,
            self.window_maximized,
        );
        if let Some(x) = self.window_x {
            out.push_str(&format!("window_x={x:.0}\n"));
        }
        if let Some(y) = self.window_y {
            out.push_str(&format!("window_y={y:.0}\n"));
        }
        if let Some(w) = self.window_w {
            out.push_str(&format!("window_w={w:.0}\n"));
        }
        if let Some(h) = self.window_h {
            out.push_str(&format!("window_h={h:.0}\n"));
        }
        out
    }
}

impl VaultPreview {
    pub(crate) fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.client.notice = if self.settings.save().is_ok() {
            "设置已保存。".into()
        } else {
            "本次设置已生效，但无法保存到本机。".into()
        };
        cx.refresh_windows();
    }

    fn toggle_keep_in_tray(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.settings.keep_in_tray {
            self.tray.take();
            self.settings.keep_in_tray = false;
        } else {
            let weak = cx.weak_entity();
            let app = cx.to_async();
            let executor = app.foreground_executor().clone();
            let tray = crate::tray::Tray::for_window(window, move || {
                let weak = weak.clone();
                let app = app.clone();
                executor
                    .spawn(async move {
                        let _ = app.update(|cx| {
                            let _ = weak.update(cx, |this, cx| this.request_lock(cx));
                        });
                    })
                    .detach();
            });
            if let Some(tray) = tray {
                self.tray = Some(tray);
                self.settings.keep_in_tray = true;
            } else {
                self.client.notice = "无法创建系统托盘图标；设置未更改。".into();
                cx.notify();
                return;
            }
        }
        self.save_settings(cx);
    }

    fn toggle_start_hidden(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.settings.start_hidden;
        if enabled
            && (!self.settings.keep_in_tray
                || !self
                    .tray
                    .as_ref()
                    .is_some_and(crate::tray::Tray::is_registered))
        {
            self.client.notice = "请先启用并确认系统托盘可用，再开启启动时隐藏。".into();
            cx.notify();
            return;
        }
        self.settings.start_hidden = enabled;
        self.save_settings(cx);
    }

    fn toggle_start_at_login(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.settings.start_at_login;
        match crate::startup::set_enabled(enabled) {
            Ok(()) => {
                self.settings.start_at_login = enabled;
                self.save_settings(cx);
            }
            Err(error) => {
                self.client.notice = error;
                cx.notify();
            }
        }
    }

    fn toggle_website_icons(&mut self, cx: &mut Context<Self>) {
        self.settings.website_icons = !self.settings.website_icons;
        if !self.settings.website_icons {
            self.favicon_cache.clear();
        }
        self.save_settings(cx);
    }

    pub fn render_settings(&self, now: std::time::Instant, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::hex;
        use crate::ui::{
            SettingsTab,
            components::{
                BtnVariant, btn, card_group, glyph_icon, info_bar, pill_chip, segmented_track,
                toggle_switch_animated,
            },
            toast::ToastKind,
            tokens::{font, glyph, radius, space},
        };

        let pal = theme::palette();
        let active_tab = self.ui.settings_tab;
        let tab_pos = self.ui.segmented_position(
            &gpui::ElementId::Name("settings-tabs".into()),
            active_tab.index(),
            now,
            self.animate,
        );
        let timeout = self.timeouts.policy(self.client.active());
        let caps = self.timeouts.capabilities();
        let timeout_enabled = self.client.ready()
            && self.client.state() == bytewarden_core::desktop::Session::Unlocked;

        let setting_row =
            |title: &'static str, subtitle: &'static str, control: gpui::AnyElement, last: bool| {
                div()
                    .px(px(space::L))
                    .py(px(12.))
                    .when(!last, |el| {
                        el.border_b_1().border_color(hex(pal.border_subtle))
                    })
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::M))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(font::BODY))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(hex(pal.text))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child(subtitle),
                            ),
                    )
                    .child(control)
            };

        let tab_bar = segmented_track(SettingsTab::ALL.len(), tab_pos).children(
            SettingsTab::ALL
                .into_iter()
                .enumerate()
                .map(|(i, (tab, label, code))| {
                    let selected = active_tab == tab;
                    div()
                        .id(("settings-tab", i))
                        .relative()
                        .cursor_pointer()
                        .flex_1()
                        .h(px(32.))
                        .px(px(8.))
                        .rounded(px(radius::CONTROL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .font_weight(if selected {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::NORMAL
                        })
                        .text_color(hex(if selected {
                            pal.accent_text
                        } else {
                            pal.text_secondary
                        }))
                        .when(!selected, |el| {
                            el.hover(move |s| s.text_color(hex(pal.text)))
                        })
                        .child(glyph_icon(code, 13.))
                        .child(div().truncate().child(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.ui.settings_tab != tab {
                                let prev = this.ui.settings_tab.index();
                                this.ui.settings_tab = tab;
                                this.ui.animate_segmented(
                                    gpui::ElementId::Name("settings-tabs".into()),
                                    prev,
                                    tab.index(),
                                    this.animate,
                                );
                                this.ui.trigger_page_motion(this.animate);
                                cx.notify();
                            }
                        }))
                }),
        );

        let tab_body = match active_tab {
            SettingsTab::Appearance => div()
                .flex()
                .flex_col()
                .gap(px(space::M))
                .child(
                    card_group("主题配色", Some(glyph::PALETTE)).child(
                        div()
                            .p(px(space::L))
                            .flex()
                            .flex_wrap()
                            .gap(px(10.))
                            .children(Theme::CHOICES.into_iter().enumerate().map(
                                |(i, (choice, label))| {
                                    let selected = self.settings.theme == choice;
                                    let preview_bg = match choice {
                                        Theme::Light => 0xf3f3f3,
                                        Theme::Dark => 0x202020,
                                        Theme::System => pal.shell,
                                    };
                                    let preview_card = match choice {
                                        Theme::Light => 0xffffff,
                                        Theme::Dark => 0x2d2d2d,
                                        Theme::System => pal.card,
                                    };
                                    div()
                                        .id(("theme", i))
                                        .cursor_pointer()
                                        .w(px(156.))
                                        .p(px(10.))
                                        .rounded(px(radius::CARD))
                                        .bg(if selected {
                                            pal.tint(pal.accent_fill)
                                        } else {
                                            hex(pal.layer).into()
                                        })
                                        .border_1()
                                        .border_color(if selected {
                                            hex(pal.accent_fill).into()
                                        } else {
                                            gpui::Hsla::from(hex(pal.border))
                                        })
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.))
                                        .hover(move |s| s.border_color(hex(pal.accent_fill)))
                                        .child(
                                            div()
                                                .h(px(46.))
                                                .w_full()
                                                .rounded(px(6.))
                                                .bg(hex(preview_bg))
                                                .border_1()
                                                .border_color(hex(pal.border))
                                                .p(px(6.))
                                                .flex()
                                                .gap(px(5.))
                                                .child(
                                                    div()
                                                        .w(px(20.))
                                                        .h_full()
                                                        .rounded(px(3.))
                                                        .bg(hex(preview_card)),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .h_full()
                                                        .rounded(px(3.))
                                                        .bg(hex(preview_card))
                                                        .p(px(5.))
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(4.))
                                                        .child(
                                                            div()
                                                                .h(px(5.))
                                                                .w(px(36.))
                                                                .rounded(px(2.))
                                                                .bg(hex(pal.accent_fill)),
                                                        )
                                                        .child(
                                                            div()
                                                                .h(px(4.))
                                                                .w_full()
                                                                .rounded(px(2.))
                                                                .bg(hex(pal.border)),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .font_weight(if selected {
                                                            gpui::FontWeight::SEMIBOLD
                                                        } else {
                                                            gpui::FontWeight::NORMAL
                                                        })
                                                        .text_color(hex(pal.text))
                                                        .child(label),
                                                )
                                                .when(selected, |el| {
                                                    el.child(
                                                        div()
                                                            .text_color(hex(pal.accent_text))
                                                            .child(glyph_icon(glyph::CHECK, 12.)),
                                                    )
                                                }),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.settings.theme = choice;
                                            theme::apply(choice, window);
                                            this.save_settings(cx);
                                        }))
                                },
                            )),
                    ),
                )
                .child(
                    card_group("界面动效与图标", Some(glyph::SPARKLE))
                        .child(setting_row(
                            "灵动界面动画",
                            "启用弹簧过渡、导航指示条滑动与弹窗动效（同时遵循 Windows 动画效果设置）。",
                            toggle_switch_animated(
                                "animation-effects",
                                self.settings.animations,
                                self.ui.toggle_progress(
                                    &gpui::ElementId::Name("animation-effects".into()),
                                    self.settings.animations,
                                    now,
                                    self.animate,
                                ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.animations = !this.settings.animations;
                                this.animate = this.settings.animations
                                    && crate::motion::effects_enabled();
                                this.ui.animate_toggle(
                                    "animation-effects",
                                    this.settings.animations,
                                    this.animate,
                                );
                                this.save_settings(cx);
                            }))
                            .into_any_element(),
                            false,
                        ))
                        .child(setting_row(
                            "显示网站图标",
                            "仅将网站主机名发送到图标服务；不会发送网址路径、查询参数或条目内容。",
                            toggle_switch_animated(
                                "website-icons",
                                self.settings.website_icons,
                                self.ui.toggle_progress(
                                    &gpui::ElementId::Name("website-icons".into()),
                                    self.settings.website_icons,
                                    now,
                                    self.animate,
                                ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_website_icons(cx);
                                this.ui.animate_toggle(
                                    "website-icons",
                                    this.settings.website_icons,
                                    this.animate,
                                );
                            }))
                            .into_any_element(),
                            true,
                        )),
                )
                .into_any_element(),
            SettingsTab::Security => div()
                .flex()
                .flex_col()
                .gap(px(space::M))
                .when(cfg!(windows), |view| {
                    let enabled = self.client.active().is_some_and(|account| account.hello_enabled);
                    let can_change = timeout_enabled
                        && (enabled || self.client.overview.is_some() && !self.client.write_guard);
                    let (pk_badge, pk_desc) = self.passkeys.state_label();
                    view.child(
                        card_group("Windows Hello", Some(glyph::LOCK))
                            .child(setting_row(
                                "本机解锁",
                                if enabled {
                                    "当前账户已启用 Windows Hello，可在锁定后解锁本机保险库。"
                                } else {
                                    "联网同步后可启用。设置时需要主密码和 Windows Hello 验证。"
                                },
                                btn("settings-hello", BtnVariant::Secondary)
                                    .child(if enabled { "关闭" } else { "启用" })
                                    .when(!can_change, |button| button.opacity(0.45))
                                    .when(can_change, |button| button.on_click(
                                        cx.listener(|this, _, window, cx| this.change_hello(window, cx)),
                                    ))
                                    .into_any_element(),
                                false,
                            ))
                            .child(setting_row(
                                "自动提示 Windows Hello",
                                "打开已启用 Windows Hello 的账户解锁页时自动请求验证。",
                                toggle_switch_animated(
                                    "auto-prompt-hello",
                                    self.settings.auto_prompt_hello,
                                    self.ui.toggle_progress(
                                        &gpui::ElementId::Name("auto-prompt-hello".into()),
                                        self.settings.auto_prompt_hello,
                                        now,
                                        self.animate,
                                    ),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings.auto_prompt_hello = !this.settings.auto_prompt_hello;
                                    this.ui.animate_toggle(
                                        gpui::ElementId::Name("auto-prompt-hello".into()),
                                        this.settings.auto_prompt_hello,
                                        this.animate,
                                    );
                                    this.save_settings(cx);
                                }))
                                .into_any_element(),
                                true,
                            )),
                    )
                    .child(
                        card_group("Windows 通行密钥提供程序", Some(glyph::PASSKEY))
                            .child(setting_row(
                                "系统原生通行密钥联动",
                                pk_desc,
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(crate::ui::components::badge(
                                        pk_badge,
                                        if self.passkeys.provider_state == "enabled" {
                                            pal.success
                                        } else if self.passkeys.provider_state == "disabled" {
                                            pal.warning
                                        } else {
                                            pal.text_muted
                                        },
                                    ))
                                    .child(
                                        btn("passkey-register", BtnVariant::Secondary)
                                            .h(px(30.))
                                            .px(px(10.))
                                            .text_size(px(font::CAPTION))
                                            .when(self.passkeys.registering, |b| b.opacity(0.45))
                                            .child(if self.passkeys.registering {
                                                "注册中…"
                                            } else {
                                                "注册到 Windows"
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.register_windows_passkey_provider(cx);
                                            })),
                                    )
                                    .child(
                                        btn("passkey-open-win-settings", BtnVariant::Subtle)
                                            .h(px(30.))
                                            .px(px(10.))
                                            .text_size(px(font::CAPTION))
                                            .child("系统设置")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                cx.open_url("ms-settings:savedpasskeys");
                                                this.refresh_windows_passkey_state(cx);
                                            })),
                                    )
                                    .into_any_element(),
                                false,
                            ))
                            .child(setting_row(
                                "同步通行密钥元数据",
                                "将当前已解锁保险库中的通行密钥索引同步给 Windows 系统选择器，以便浏览网页时直接显示。",
                                btn("passkey-sync-meta", BtnVariant::Secondary)
                                    .h(px(30.))
                                    .px(px(10.))
                                    .text_size(px(font::CAPTION))
                                    .when(!timeout_enabled || self.passkeys.syncing_metadata, |b| {
                                        b.opacity(0.45)
                                    })
                                    .child(if self.passkeys.syncing_metadata {
                                        "同步中…"
                                    } else {
                                        "立即同步"
                                    })
                                    .when(timeout_enabled && !self.passkeys.syncing_metadata, |b| {
                                        b.on_click(cx.listener(|this, _, _, cx| {
                                            this.sync_windows_passkey_metadata(false, cx);
                                        }))
                                    })
                                    .into_any_element(),
                                true,
                            )),
                    )
                    .child(
                        card_group("Firefox 浏览器扩展联动", Some(glyph::GLOBE))
                            .child(setting_row(
                                "启用浏览器原生通信联动",
                                "桌面端解锁或锁定时实时同步到 Firefox 轻量扩展，浏览器端零密码库常驻、零重复加密核心。",
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(crate::ui::components::badge(
                                        if !self.settings.browser_integration {
                                            "已关闭"
                                        } else if self.browser_bridge.connected_clients > 0 {
                                            "Firefox 已连接"
                                        } else if self.browser_bridge.host_registered {
                                            "已就绪"
                                        } else {
                                            "待注册"
                                        },
                                        if !self.settings.browser_integration {
                                            pal.text_muted
                                        } else if self.browser_bridge.connected_clients > 0 {
                                            pal.success
                                        } else if self.browser_bridge.host_registered {
                                            pal.accent_text
                                        } else {
                                            pal.warning
                                        },
                                    ))
                                    .child(
                                        toggle_switch_animated(
                                            "browser-integration",
                                            self.settings.browser_integration,
                                            self.ui.toggle_progress(
                                                &gpui::ElementId::Name(
                                                    "browser-integration".into(),
                                                ),
                                                self.settings.browser_integration,
                                                now,
                                                self.animate,
                                            ),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.settings.browser_integration =
                                                !this.settings.browser_integration;
                                            this.ui.animate_toggle(
                                                gpui::ElementId::Name("browser-integration".into()),
                                                this.settings.browser_integration,
                                                this.animate,
                                            );
                                            this.sync_browser_bridge_service(window, cx);
                                            this.save_settings(cx);
                                        })),
                                    )
                                    .into_any_element(),
                                false,
                            ))
                            .child(setting_row(
                                "填充前要求生物识别或手动授权",
                                "开启后，Firefox 请求自动填充或复制密码前需通过 Windows Hello 验证（或在桌面端确认）。",
                                toggle_switch_animated(
                                    "browser-biometric-gate",
                                    self.settings.browser_biometric_gate,
                                    self.ui.toggle_progress(
                                        &gpui::ElementId::Name("browser-biometric-gate".into()),
                                        self.settings.browser_biometric_gate,
                                        now,
                                        self.animate,
                                    ),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings.browser_biometric_gate =
                                        !this.settings.browser_biometric_gate;
                                    this.ui.animate_toggle(
                                        gpui::ElementId::Name("browser-biometric-gate".into()),
                                        this.settings.browser_biometric_gate,
                                        this.animate,
                                    );
                                    this.broadcast_browser_state();
                                    this.save_settings(cx);
                                }))
                                .into_any_element(),
                                false,
                            ))
                            .child(setting_row(
                                "Firefox 原生通信清单与扩展包",
                                "一键写入 Mozilla NativeMessagingHosts 清单，并打开配套轻量级 Bytewarden Firefox 扩展目录。",
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        btn("browser-register-firefox", BtnVariant::Secondary)
                                            .h(px(30.))
                                            .px(px(10.))
                                            .text_size(px(font::CAPTION))
                                            .child(if self.browser_bridge.host_registered {
                                                "重新注册清单"
                                            } else {
                                                "注册到 Firefox"
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                match crate::browser_bridge::register_firefox_native_host() {
                                                    Ok(_) => {
                                                        this.browser_bridge.host_registered = true;
                                                        this.ui.toast.show(
                                                            ToastKind::Success,
                                                            "已注册 Firefox 原生通信清单",
                                                            std::time::Instant::now(),
                                                            this.animate,
                                                        );
                                                    }
                                                    Err(err) => {
                                                        this.ui.toast.show(
                                                            ToastKind::Error,
                                                            err,
                                                            std::time::Instant::now(),
                                                            this.animate,
                                                        );
                                                    }
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        btn("browser-open-ext-dir", BtnVariant::Subtle)
                                            .h(px(30.))
                                            .px(px(10.))
                                            .text_size(px(font::CAPTION))
                                            .child("扩展目录")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let dir = crate::browser_bridge::locate_extension_directory();
                                                let _ = std::process::Command::new("explorer.exe")
                                                    .arg(&dir)
                                                    .spawn();
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    "about:debugging#/runtime/this-firefox".into(),
                                                ));
                                                this.ui.toast.show(
                                                    ToastKind::Info,
                                                    "已打开扩展目录，并复制 about:debugging 地址",
                                                    std::time::Instant::now(),
                                                    this.animate,
                                                );
                                                cx.notify();
                                            })),
                                    )
                                    .into_any_element(),
                                true,
                            )),
                    )
                    .child(
                        card_group("屏幕隐私保护", Some(glyph::SHIELD)).child(setting_row(
                            "防止截图与录屏捕获",
                            "开启后调用 Windows 原生保护，截图工具、录屏与远程屏幕共享将无法捕获本窗口内容。",
                            toggle_switch_animated(
                                "prevent-screenshots",
                                self.settings.prevent_screenshots,
                                self.ui.toggle_progress(
                                    &gpui::ElementId::Name("prevent-screenshots".into()),
                                    self.settings.prevent_screenshots,
                                    now,
                                    self.animate,
                                ),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings.prevent_screenshots =
                                    !this.settings.prevent_screenshots;
                                crate::tray::apply_screen_capture_protection(
                                    window,
                                    this.settings.prevent_screenshots,
                                );
                                this.ui.animate_toggle(
                                    gpui::ElementId::Name("prevent-screenshots".into()),
                                    this.settings.prevent_screenshots,
                                    this.animate,
                                );
                                this.save_settings(cx);
                            }))
                            .into_any_element(),
                            true,
                        )),
                    )
                })
                .child(
                    card_group("保险库超时策略", Some(glyph::LOCK)).child(
                        div()
                            .p(px(space::L))
                            .flex()
                            .flex_col()
                            .gap(px(space::M))
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("离开应用后计算指定分钟；系统空闲按 Windows 最近输入计算。锁定只清除解密密钥，退出登录同时撤销本机缓存。"),
                            )
                            .when(!timeout_enabled, |el| {
                                el.child(info_bar(
                                    ToastKind::Info,
                                    "登录并解锁后，可修改当前账户的超时设置。",
                                ))
                            })
                            .when(timeout_enabled, |el| {
                                el.child(
                                    div().flex().flex_wrap().gap(px(6.)).children(
                                        crate::timeout::Mode::CHOICES.into_iter().enumerate().map(
                                            |(i, (mode, label))| {
                                                let available = match mode {
                                                    crate::timeout::Mode::SystemLock => caps.lock,
                                                    crate::timeout::Mode::SystemSleep => caps.sleep,
                                                    crate::timeout::Mode::SystemIdle => caps.idle,
                                                    _ => true,
                                                };
                                                pill_chip(
                                                    ("vault-timeout-mode", i),
                                                    if available {
                                                        label.to_owned()
                                                    } else {
                                                        format!("{label}（不可用）")
                                                    },
                                                    None,
                                                    timeout.mode == mode,
                                                )
                                                .when(!available, |c| c.opacity(0.45))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if !available {
                                                        return;
                                                    }
                                                    if mode == crate::timeout::Mode::Never
                                                        && timeout.mode != mode
                                                    {
                                                        this.timeouts.never_confirmation = true;
                                                        cx.notify();
                                                    } else {
                                                        this.save_timeout_policy(
                                                            crate::timeout::Policy {
                                                                mode,
                                                                ..timeout
                                                            },
                                                            cx,
                                                        );
                                                    }
                                                }))
                                            },
                                        ),
                                    ),
                                )
                                .when(timeout.mode == crate::timeout::Mode::Inactivity, |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .children(self.timeouts.minutes_input.clone())
                                            .child(
                                                btn("save-timeout-minutes", BtnVariant::Primary)
                                                    .h(px(32.))
                                                    .child("保存分钟数")
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            let minutes = this
                                                                .timeouts
                                                                .minutes_input
                                                                .as_ref()
                                                                .and_then(|input| {
                                                                    let input = input.read(cx);
                                                                    input
                                                                        .committed()
                                                                        .then(|| {
                                                                            input
                                                                                .value()
                                                                                .trim()
                                                                                .parse::<u32>()
                                                                                .ok()
                                                                        })
                                                                        .flatten()
                                                                });
                                                            if let Some(minutes) =
                                                                minutes.filter(|minutes| {
                                                                    (1..=i32::MAX as u32)
                                                                        .contains(minutes)
                                                                })
                                                            {
                                                                this.save_timeout_policy(
                                                                    crate::timeout::Policy {
                                                                        minutes,
                                                                        ..timeout
                                                                    },
                                                                    cx,
                                                                );
                                                            } else {
                                                                this.client.notice =
                                                                    "请填写 1 至 2147483647 之间的整数分钟。"
                                                                        .into();
                                                                cx.notify();
                                                            }
                                                        },
                                                    )),
                                            ),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_size(px(font::CAPTION))
                                                .text_color(hex(pal.text_secondary))
                                                .child("超时动作："),
                                        )
                                        .children(
                                            [
                                                (crate::timeout::Action::Lock, "锁定"),
                                                (crate::timeout::Action::LogOut, "退出登录"),
                                            ]
                                            .into_iter()
                                            .enumerate()
                                            .map(|(i, (action, label))| {
                                                pill_chip(
                                                    ("vault-timeout-action", i),
                                                    label,
                                                    None,
                                                    timeout.action == action,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.save_timeout_policy(
                                                        crate::timeout::Policy {
                                                            action,
                                                            ..timeout
                                                        },
                                                        cx,
                                                    )
                                                }))
                                            }),
                                        ),
                                )
                                .when(self.timeouts.never_confirmation, |el| {
                                    el.child(
                                        info_bar(
                                            ToastKind::Warning,
                                            "保持保险库解锁？选择「从不」后，本次会话将保持解锁直到手动锁定或退出。",
                                        )
                                        .child(
                                            btn("confirm-timeout-never", BtnVariant::Danger)
                                                .h(px(26.))
                                                .px(px(10.))
                                                .text_size(px(font::CAPTION))
                                                .child("保持解锁")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.save_timeout_policy(
                                                        crate::timeout::Policy {
                                                            mode: crate::timeout::Mode::Never,
                                                            ..timeout
                                                        },
                                                        cx,
                                                    )
                                                })),
                                        )
                                        .child(
                                            btn("cancel-timeout-never", BtnVariant::Subtle)
                                                .h(px(26.))
                                                .px(px(8.))
                                                .text_size(px(font::CAPTION))
                                                .child("取消")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.timeouts.never_confirmation = false;
                                                    cx.notify();
                                                })),
                                        ),
                                    )
                                })
                            })
                            .when(timeout.effective_mode(caps) != timeout.mode, |el| {
                                el.child(info_bar(
                                    ToastKind::Warning,
                                    if timeout.effective_mode(caps)
                                        == crate::timeout::Mode::SystemSleep
                                    {
                                        "系统锁定监听不可用，当前按系统休眠触发。"
                                    } else {
                                        "系统事件监听不可用，当前按应用重启触发。"
                                    },
                                ))
                            }),
                    ),
                )
                .child(
                    card_group("自动清除剪贴板", Some(glyph::COPY)).child(
                        div()
                            .p(px(space::L))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div().flex().flex_wrap().gap(px(6.)).children(
                                    [10, 30, 60, 120].into_iter().enumerate().map(|(i, seconds)| {
                                        pill_chip(
                                            ("clipboard-delay", i),
                                            format!("{seconds} 秒"),
                                            None,
                                            self.settings.clipboard_seconds == seconds,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.settings.clipboard_seconds = seconds;
                                            this.schedule_clipboard_clear(
                                                std::time::Duration::from_secs(seconds),
                                                cx,
                                            );
                                            this.save_settings(cx);
                                        }))
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_muted))
                                    .child("仅清除本应用仍拥有的内容。锁定、切换账户和退出时仍会尝试清除。"),
                            ),
                    ),
                )
                .into_any_element(),
            SettingsTab::Windows => card_group("Windows 启动与系统托盘", Some("\u{e7f4}"))
                .child(setting_row(
                    "关闭窗口时隐藏到系统托盘",
                    "点击关闭按钮时最小化到托盘图标，可在托盘右键彻底退出或锁定。",
                    toggle_switch_animated(
                        "keep-in-tray",
                        self.settings.keep_in_tray,
                        self.ui.toggle_progress(
                            &gpui::ElementId::Name("keep-in-tray".into()),
                            self.settings.keep_in_tray,
                            now,
                            self.animate,
                        ),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_keep_in_tray(window, cx);
                        this.ui.animate_toggle(
                            "keep-in-tray",
                            this.settings.keep_in_tray,
                            this.animate,
                        );
                    }))
                    .into_any_element(),
                    false,
                ))
                .child(setting_row(
                    "启动时静默隐藏到托盘",
                    "需先启用系统托盘并确保托盘图标已成功注册。",
                    toggle_switch_animated(
                        "start-hidden",
                        self.settings.start_hidden,
                        self.ui.toggle_progress(
                            &gpui::ElementId::Name("start-hidden".into()),
                            self.settings.start_hidden,
                            now,
                            self.animate,
                        ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_start_hidden(cx);
                        this.ui.animate_toggle(
                            "start-hidden",
                            this.settings.start_hidden,
                            this.animate,
                        );
                    }))
                    .into_any_element(),
                    false,
                ))
                .child(setting_row(
                    "复制后最小化窗口",
                    "仅在安全剪贴板成功写入后最小化窗口；不会关闭、锁定或隐藏到系统托盘。",
                    toggle_switch_animated(
                        "minimize-after-copy",
                        self.settings.minimize_after_copy,
                        self.ui.toggle_progress(
                            &gpui::ElementId::Name("minimize-after-copy".into()),
                            self.settings.minimize_after_copy,
                            now,
                            self.animate,
                        ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.minimize_after_copy = !this.settings.minimize_after_copy;
                        this.ui.animate_toggle(
                            "minimize-after-copy",
                            this.settings.minimize_after_copy,
                            this.animate,
                        );
                        this.save_settings(cx);
                    }))
                    .into_any_element(),
                    false,
                ))
                .child(setting_row(
                    "Windows 登录时自动启动",
                    "仅在开启时写入当前用户 Run 启动项。",
                    toggle_switch_animated(
                        "start-at-login",
                        self.settings.start_at_login,
                        self.ui.toggle_progress(
                            &gpui::ElementId::Name("start-at-login".into()),
                            self.settings.start_at_login,
                            now,
                            self.animate,
                        ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_start_at_login(cx);
                        this.ui.animate_toggle(
                            "start-at-login",
                            this.settings.start_at_login,
                            this.animate,
                        );
                    }))
                    .into_any_element(),
                    true,
                ))
                .into_any_element(),
            SettingsTab::Data => self.render_backup(now, cx).into_any_element(),
            SettingsTab::About => self.render_about(cx).into_any_element(),
        };

        div()
            .id("settings-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::XL))
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .flex()
                    .flex_col()
                    .gap(px(space::L))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(font::SUBTITLE))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child("设置"),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("自定义外观、灵动动效、安全策略与 Windows 集成。"),
                            ),
                    )
                    .child(tab_bar)
                    .child(tab_body),
            )
    }

    pub fn render_about(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::hex;
        use crate::ui::{
            brand::brand_logo,
            components::{BtnVariant, badge, btn, card_group, glyph_icon, surface_card},
            tokens::{font, glyph, space},
        };

        let pal = theme::palette();
        div()
            .id("about-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(680.))
                    .flex()
                    .flex_col()
                    .gap(px(space::M))
                    .child(
                        surface_card()
                            .p(px(space::XL))
                            .flex()
                            .items_center()
                            .gap(px(space::L))
                            .child(brand_logo(64.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .text_size(px(font::SUBTITLE))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(hex(pal.text))
                                                    .child("Bytewarden"),
                                            )
                                            .child(badge(
                                                format!("v{}", env!("CARGO_PKG_VERSION")),
                                                pal.accent_text,
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(font::CAPTION))
                                            .text_color(hex(pal.text_secondary))
                                            .child(format!(
                                                "GPUI 0.2.2 · Rust 原生桌面客户端 · {} / {}",
                                                std::env::consts::OS,
                                                std::env::consts::ARCH
                                            )),
                                    )
                                    .child(
                                        div().pt(px(4.)).child(
                                            btn("copy-version", BtnVariant::Secondary)
                                                .h(px(30.))
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(glyph::COPY, 12.))
                                                .child("复制版本信息")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.clear_clipboard(cx);
                                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                                        format!(
                                                            "Bytewarden {}\nGPUI 0.2.2 / Rust\n{} / {}",
                                                            env!("CARGO_PKG_VERSION"),
                                                            std::env::consts::OS,
                                                            std::env::consts::ARCH
                                                        ),
                                                    ));
                                                    this.ui.toast.show(
                                                        crate::ui::toast::ToastKind::Success,
                                                        "已复制版本信息",
                                                        std::time::Instant::now(),
                                                        this.animate,
                                                    );
                                                    cx.notify();
                                                })),
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        card_group("项目资源与许可", Some(glyph::INFO)).child(
                            div()
                                .p(px(space::L))
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .children(
                                    [
                                        ("项目主页与中文文档", "https://github.com/ZHT0711/Bytewarden"),
                                        (
                                            "English documentation",
                                            "https://github.com/ZHT0711/Bytewarden/blob/main/README.en.md",
                                        ),
                                        (
                                            "构建状态与预览包",
                                            "https://github.com/ZHT0711/Bytewarden/actions/workflows/windows.yml",
                                        ),
                                        ("反馈问题或建议", "https://github.com/ZHT0711/Bytewarden/issues"),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, (label, url))| {
                                        btn(("project-link", i), BtnVariant::Subtle)
                                            .justify_between()
                                            .w_full()
                                            .text_color(hex(pal.accent_text))
                                            .child(label)
                                            .child(glyph_icon(glyph::OPEN_EXTERNAL, 12.))
                                            .on_click(move |_, _, cx| cx.open_url(url))
                                    }),
                                )
                                .child(
                                    div()
                                        .pt(px(6.))
                                        .text_size(px(font::CAPTION))
                                        .text_color(hex(pal.text_muted))
                                        .child("基于官方 Bitwarden Rust SDK 构建，采用 GPL-3.0 开源许可。本项目与 Bitwarden, Inc. 无任何关联。"),
                                ),
                        ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn application_preferences_default_and_roundtrip() {
        let defaults = Settings::default();
        assert!(defaults.website_icons);
        assert!(defaults.keep_in_tray);
        assert!(defaults.start_hidden);
        assert!(!defaults.minimize_after_copy);
        let parsed = Settings::parse(
            "theme=system\nwebsite_icons=false\nkeep_in_tray=false\nstart_hidden=false\nminimize_after_copy=true\n",
        );
        assert!(!parsed.website_icons);
        assert!(!parsed.keep_in_tray);
        assert!(!parsed.start_hidden);
        let roundtrip = Settings::parse(&parsed.serialized());
        assert!(!roundtrip.website_icons);
        assert!(!roundtrip.keep_in_tray);
        assert!(!roundtrip.start_hidden);
        assert!(roundtrip.minimize_after_copy);
    }
}
