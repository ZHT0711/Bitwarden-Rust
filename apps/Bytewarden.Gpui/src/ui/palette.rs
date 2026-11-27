//! Lively spring-animated Command Palette (`Ctrl+K`) for instant item jump and app commands.
//! Searches only non-sensitive display metadata (name + subtitle) and never decrypts secrets.

use std::time::Instant;

use bytewarden_core::desktop::Session;
use gpui::{Context, KeyDownEvent, MouseButton, Window, div, prelude::*, px};

use crate::{
    Page, VaultPreview,
    theme::{self, Theme, hex},
    vault_list::{self, Filter},
};

use super::{
    components::{elevated_card, glyph_icon, item_monogram},
    spring::clamp_opacity,
    state::SettingsTab,
    tokens::{family, font, glyph, radius, space},
};

#[derive(Clone)]
pub enum PaletteAction {
    OpenItem(usize),
    NewItem,
    Sync,
    Lock,
    OpenPage(Page),
    OpenSettings(SettingsTab),
    OpenFilter(Filter),
    EmptyTrash,
    ToggleTheme,
    ToggleAnimations,
    SwitchAccount(String),
    ShowShortcuts,
}

pub struct PaletteEntry {
    pub title: String,
    pub subtitle: String,
    pub icon_glyph: &'static str,
    pub shortcut: Option<&'static str>,
    pub item_type: Option<u8>,
    pub action: PaletteAction,
}

impl VaultPreview {
    pub fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing || !self.client.ready() || !self.navigation_available() {
            return;
        }
        let now = Instant::now();
        self.ui.close_popovers(self.animate);
        self.client.show_accounts = false;
        self.ui.shortcuts_open = false;
        self.ui.shortcuts_spring.retarget(0., now, self.animate);

        self.ui.palette_open = true;
        self.ui.palette_selected = 0;
        self.ui.palette_query.clear();
        self.ui
            .palette_input
            .update(cx, |input, cx| input.set("", cx));
        self.ui.palette_input.read(cx).focus(window);
        self.ui.palette_spring.retarget(1., now, self.animate);
        cx.notify();
    }

    pub fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ui.palette_open {
            return;
        }
        let now = Instant::now();
        self.ui.palette_open = false;
        self.ui.palette_spring.retarget(0., now, self.animate);
        self.focus.focus(window);
        cx.notify();
    }

    pub fn collect_palette_entries(&mut self) -> Vec<PaletteEntry> {
        self.refresh_search_index();
        let query = self.ui.palette_query.trim().to_lowercase();
        let mut entries = Vec::new();

        if let Some(overview) = &self.client.overview {
            for &index in self.search_index.palette_items(&overview.items, &query) {
                let item = &overview.items[index];
                entries.push(PaletteEntry {
                    title: item.name.clone(),
                    subtitle: if item.subtitle.is_empty() {
                        "保险库条目".into()
                    } else {
                        item.subtitle.clone()
                    },
                    icon_glyph: vault_list::icon_for(item),
                    shortcut: None,
                    item_type: Some(item.item_type),
                    action: PaletteAction::OpenItem(index),
                });
            }
        }

        // 2. Application commands.
        let mut commands = Vec::new();
        if self.write_available() {
            commands.push(PaletteEntry {
                title: "新建保险库条目".into(),
                subtitle: "在当前保险库创建登录、笔记或银行卡".into(),
                icon_glyph: glyph::ADD,
                shortcut: Some("Ctrl+N"),
                item_type: None,
                action: PaletteAction::NewItem,
            });
            if self.page == Page::Vault
                && self.client.overview.as_ref().is_some_and(|overview| {
                    overview
                        .items
                        .iter()
                        .any(|item| item.deleted && !item.restricted)
                })
            {
                commands.push(PaletteEntry {
                    title: "清空回收站".into(),
                    subtitle: "确认并永久删除全部可操作的回收站条目（单次最多 500 个）".into(),
                    icon_glyph: glyph::DELETE,
                    shortcut: None,
                    item_type: None,
                    action: PaletteAction::EmptyTrash,
                });
            }
        }
        if self.client.sync_available() || self.client.reconnect_available() {
            commands.push(PaletteEntry {
                title: if self.client.reconnect_available() {
                    "重新连接账户"
                } else {
                    "同步保险库"
                }
                .into(),
                subtitle: "从服务器拉取最新加密数据".into(),
                icon_glyph: glyph::SYNC,
                shortcut: Some("Ctrl+R"),
                item_type: None,
                action: PaletteAction::Sync,
            });
        }
        if self.client.state() == Session::Unlocked {
            commands.push(PaletteEntry {
                title: "立即锁定保险库".into(),
                subtitle: "清除内存中的解密密钥".into(),
                icon_glyph: glyph::LOCK,
                shortcut: Some("Ctrl+L"),
                item_type: None,
                action: PaletteAction::Lock,
            });
        }
        commands.push(PaletteEntry {
            title: "打开密码生成器".into(),
            subtitle: "生成高强度密码、短语或用户名".into(),
            icon_glyph: glyph::KEY,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenPage(Page::Generator),
        });
        commands.push(PaletteEntry {
            title: "打开安全体检".into(),
            subtitle: "离线检测弱密码、重复密码、未启用 2FA 与 HTTP 链接".into(),
            icon_glyph: glyph::SHIELD,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenPage(Page::Health),
        });
        commands.push(PaletteEntry {
            title: "打开 Send 分享".into(),
            subtitle: "安全分享限时文本或文件".into(),
            icon_glyph: glyph::SEND,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenPage(Page::Sends),
        });
        commands.push(PaletteEntry {
            title: "查看收藏夹".into(),
            subtitle: "仅显示已收藏的条目".into(),
            icon_glyph: glyph::STAR,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenFilter(Filter::Favorites),
        });
        commands.push(PaletteEntry {
            title: "管理文件夹".into(),
            subtitle: "搜索、创建、重命名或删除文件夹".into(),
            icon_glyph: glyph::FOLDER,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenPage(Page::Folders),
        });
        commands.push(PaletteEntry {
            title: "导入与导出保险库".into(),
            subtitle: "备份或迁移 Bitwarden JSON / CSV".into(),
            icon_glyph: glyph::EXPORT,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenSettings(SettingsTab::Data),
        });
        commands.push(PaletteEntry {
            title: "切换浅色 / 深色主题".into(),
            subtitle: "在深色、浅色与跟随 Windows 间切换".into(),
            icon_glyph: glyph::PALETTE,
            shortcut: None,
            item_type: None,
            action: PaletteAction::ToggleTheme,
        });
        commands.push(PaletteEntry {
            title: if self.settings.animations {
                "关闭界面动效"
            } else {
                "开启灵动界面动效"
            }
            .into(),
            subtitle: "控制全局弹簧与过渡动画".into(),
            icon_glyph: glyph::SPARKLE,
            shortcut: None,
            item_type: None,
            action: PaletteAction::ToggleAnimations,
        });
        commands.push(PaletteEntry {
            title: "打开设置".into(),
            subtitle: "主题、超时、托盘与安全选项".into(),
            icon_glyph: glyph::SETTINGS,
            shortcut: None,
            item_type: None,
            action: PaletteAction::OpenSettings(SettingsTab::Appearance),
        });
        commands.push(PaletteEntry {
            title: "快捷键指南".into(),
            subtitle: "查看全部键盘快捷键".into(),
            icon_glyph: glyph::KEYBOARD,
            shortcut: Some("Ctrl+/"),
            item_type: None,
            action: PaletteAction::ShowShortcuts,
        });

        for account in &self.client.accounts {
            if !account.active {
                commands.push(PaletteEntry {
                    title: format!("切换到账户 {}", account.email),
                    subtitle: "切换当前活动账户".into(),
                    icon_glyph: glyph::PERSON,
                    shortcut: None,
                    item_type: None,
                    action: PaletteAction::SwitchAccount(account.id.clone()),
                });
            }
        }

        for cmd in commands {
            if query.is_empty()
                || cmd.title.to_lowercase().contains(&query)
                || cmd.subtitle.to_lowercase().contains(&query)
            {
                entries.push(cmd);
            }
        }
        entries
    }

    pub fn execute_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Recheck at execution as an entry can outlive the state it was built
        // from. A rejected page change must not continue into a detail read.
        if self.closing || !self.client.ready() || !self.navigation_available() {
            return;
        }
        self.close_command_palette(window, cx);
        match action {
            PaletteAction::OpenItem(index) => {
                let Some(item) = self
                    .client
                    .overview
                    .as_ref()
                    .and_then(|o| o.items.get(index))
                    .filter(|item| !item.deleted)
                else {
                    return;
                };
                let filter = if item.archived {
                    Filter::Archived
                } else {
                    Filter::All
                };
                self.open_page(Page::Vault, cx);
                self.filter = filter;
                self.client.selected_folder = None;
                self.client.search.update(cx, |input, cx| input.set("", cx));
                self.query.clear();
                self.invalidate_query();
                self.choose_item(index, cx);
            }
            PaletteAction::NewItem => {
                self.open_page(Page::Vault, cx);
                self.new_item(window, cx);
            }
            PaletteAction::Sync => self.sync_or_reconnect(window, cx),
            PaletteAction::Lock => self.request_lock(cx),
            PaletteAction::OpenPage(page) => self.open_page(page, cx),
            PaletteAction::OpenSettings(tab) => {
                self.ui.settings_tab = tab;
                self.open_page(Page::Settings, cx);
            }
            PaletteAction::OpenFilter(filter) => {
                self.open_page(Page::Vault, cx);
                self.filter = filter;
                self.client.selected_folder = None;
                self.invalidate_query();
                self.ui.trigger_list_motion(self.animate);
                cx.notify();
            }
            PaletteAction::EmptyTrash => self.empty_trash(cx),
            PaletteAction::ToggleTheme => {
                let next = match self.settings.theme {
                    Theme::System => Theme::Dark,
                    Theme::Dark => Theme::Light,
                    Theme::Light => Theme::System,
                };
                self.settings.theme = next;
                theme::apply(next, window);
                self.save_settings(cx);
                let label = Theme::CHOICES
                    .iter()
                    .find(|(t, _)| *t == next)
                    .map_or("主题", |(_, l)| *l);
                self.ui.toast.show(
                    super::toast::ToastKind::Info,
                    format!("已切换主题：{label}"),
                    Instant::now(),
                    self.animate,
                );
            }
            PaletteAction::ToggleAnimations => {
                self.settings.animations = !self.settings.animations;
                self.animate = self.settings.animations && crate::motion::effects_enabled();
                self.save_settings(cx);
                self.ui.toast.show(
                    super::toast::ToastKind::Info,
                    if self.settings.animations {
                        "已开启灵动界面动画"
                    } else {
                        "已关闭界面动画"
                    },
                    Instant::now(),
                    self.animate,
                );
            }
            PaletteAction::SwitchAccount(id) => self.select_account(id, cx),
            PaletteAction::ShowShortcuts => self.toggle_shortcuts_modal(cx),
        }
    }

    pub(crate) fn submit_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ui.palette_open || !self.ui.palette_input.read(cx).committed() {
            return;
        }
        let entries = self.collect_palette_entries();
        if let Some(entry) = entries.get(
            self.ui
                .palette_selected
                .min(entries.len().saturating_sub(1)),
        ) {
            self.execute_palette_action(entry.action.clone(), window, cx);
        }
    }

    pub fn render_command_palette(
        &mut self,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pal = theme::palette();
        let entries = self.collect_palette_entries();
        let selected = if entries.is_empty() {
            0
        } else {
            self.ui.palette_selected.min(entries.len() - 1)
        };
        let opacity = clamp_opacity(progress);
        let y_offset = 24. * (1. - progress);

        div()
            .id("command-palette-backdrop")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(pal.scrim(opacity))
            .occlude()
            .flex()
            .justify_center()
            .pt(px(64. + y_offset))
            .px(px(space::L))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_command_palette(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let entries = this.collect_palette_entries();
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        this.close_command_palette(window, cx);
                    }
                    "down" if !entries.is_empty() => {
                        cx.stop_propagation();
                        this.ui.palette_selected = (this.ui.palette_selected + 1) % entries.len();
                        cx.notify();
                    }
                    "up" if !entries.is_empty() => {
                        cx.stop_propagation();
                        this.ui.palette_selected = this
                            .ui
                            .palette_selected
                            .checked_sub(1)
                            .unwrap_or(entries.len() - 1);
                        cx.notify();
                    }
                    "enter" => {
                        cx.stop_propagation();
                        this.submit_command_palette(window, cx);
                    }
                    _ => {}
                }
            }))
            .child(
                elevated_card()
                    .w_full()
                    .max_w(px(560.))
                    .max_h(px(460.))
                    .overflow_hidden()
                    .opacity(opacity)
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(10.))
                            .border_b_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_color(hex(pal.accent_text))
                                    .child(glyph_icon(glyph::SEARCH, 16.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.ui.palette_input.clone()),
                            )
                            .child(
                                div()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(radius::SM))
                                    .bg(hex(pal.layer))
                                    .border_1()
                                    .border_color(hex(pal.border))
                                    .text_size(px(10.5))
                                    .font_family(family::MONO)
                                    .text_color(hex(pal.text_muted))
                                    .child("ESC"),
                            ),
                    )
                    .child(
                        div()
                            .id("palette-results")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .when(entries.is_empty(), |el| {
                                el.child(
                                    div()
                                        .py(px(28.))
                                        .text_center()
                                        .text_size(px(font::CAPTION))
                                        .text_color(hex(pal.text_muted))
                                        .child("没有匹配的条目或命令"),
                                )
                            })
                            .children(entries.into_iter().enumerate().map(|(i, entry)| {
                                let is_sel = i == selected;
                                let action = entry.action;
                                div()
                                    .id(("palette-row", i))
                                    .cursor_pointer()
                                    .px(px(10.))
                                    .py(px(8.))
                                    .rounded(px(radius::CONTROL))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .when(is_sel, |row| {
                                        row.bg(pal.tint(pal.accent_fill))
                                            .border_1()
                                            .border_color(pal.tint_border(pal.accent_fill))
                                    })
                                    .when(!is_sel, |row| {
                                        row.border_1()
                                            .border_color(gpui::hsla(0., 0., 0., 0.))
                                            .hover(move |s| s.bg(hex(pal.hover)))
                                    })
                                    .child(if let Some(kind) = entry.item_type {
                                        item_monogram(&entry.title, kind, entry.icon_glyph, 28., 6.)
                                    } else {
                                        div()
                                            .size(px(28.))
                                            .rounded(px(6.))
                                            .bg(hex(pal.layer))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(hex(if is_sel {
                                                pal.accent_text
                                            } else {
                                                pal.text_secondary
                                            }))
                                            .child(glyph_icon(entry.icon_glyph, 13.))
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(font::BODY))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .text_color(hex(pal.text))
                                                    .child(entry.title),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(font::MICRO))
                                                    .text_color(hex(pal.text_muted))
                                                    .child(entry.subtitle),
                                            ),
                                    )
                                    .when_some(entry.shortcut, |row, keys| {
                                        row.child(
                                            div()
                                                .px(px(6.))
                                                .py(px(2.))
                                                .rounded(px(radius::SM))
                                                .bg(hex(pal.layer))
                                                .border_1()
                                                .border_color(hex(pal.border))
                                                .text_size(px(10.5))
                                                .font_family(family::MONO)
                                                .text_color(hex(pal.text_secondary))
                                                .child(keys),
                                        )
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.execute_palette_action(action.clone(), window, cx);
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(7.))
                            .border_t_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(font::MICRO))
                            .text_color(hex(pal.text_muted))
                            .child("↑↓ 选择 · Enter 执行 · Esc 关闭")
                            .child("仅搜索标题与副标题元数据"),
                    ),
            )
    }
}
