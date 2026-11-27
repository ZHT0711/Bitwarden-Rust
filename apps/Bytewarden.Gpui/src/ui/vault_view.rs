//! Upgraded Vault workspace view: single-row header, Type Filter Chips,
//! full-width 58px virtual list rows with hover quick-copy pills, resizable
//! pane splitter, inline Detail/Editor pane, and polished Hero welcome/lock screens.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use bytewarden_core::desktop::{Item, Overview, Session};
use gpui::{Context, CursorStyle, Div, MouseButton, Stateful, div, prelude::*, px, uniform_list};
use zeroize::Zeroizing;

use crate::{
    QuickCopyAction, Submit, VaultPreview,
    theme::{self, hex},
    vault_list::{self, FILTERS, Filter},
};

use super::{
    brand::brand_logo,
    components::{
        BtnVariant, badge, btn, elevated_card, empty_state, glyph_icon, icon_btn, item_monogram,
        make_tooltip, pill_chip, surface_card,
    },
    spring::clamp_opacity,
    tokens::{font, glyph, radius, size, space},
};

const TYPE_CHIPS: &[(Filter, &str)] = &[
    (Filter::All, "全部"),
    (Filter::Kind(1), "登录"),
    (Filter::Kind(2), "笔记"),
    (Filter::Kind(3), "银行卡"),
    (Filter::Kind(4), "身份"),
    (Filter::Kind(5), "SSH"),
    (Filter::Kind(6), "银行账户"),
    (Filter::Kind(7), "驾照"),
    (Filter::Kind(8), "护照"),
];

impl VaultPreview {
    pub fn render_vault_workspace(
        &mut self,
        indices: Rc<[usize]>,
        narrow: bool,
        list_progress: f32,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Div {
        let pal = theme::palette();

        // Signed-out / Locked / Unsynced hero screen — clean centered Fluent welcome card.
        if self.client.overview.is_none() {
            if self.lock_queued || self.client.busy.is_some() {
                return div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        surface_card()
                            .p(px(space::XL))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(space::M))
                            .child(brand_logo(56.))
                            .child(
                                div()
                                    .text_size(px(font::BODY_LG))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(self.client.busy.unwrap_or("正在锁定保险库…")),
                            ),
                    );
            }
            return self.render_vault_gate_screen(cx);
        }

        let folder_navigation = self.render_folder_navigation(now, cx);
        let count = indices.len();
        let show_type_chips = !self.selection.active
            && (self.client.selected_folder.is_some()
                || matches!(self.filter, Filter::All | Filter::Kind(_)));
        let (new_menu_p, _) =
            self.ui
                .new_menu_spring
                .sample_staggered(now, Duration::ZERO, self.animate);

        let heading_title: String = if let Some(folder_id) = self.client.selected_folder.as_deref()
        {
            if folder_id.is_empty() {
                "无文件夹".into()
            } else {
                self.folders
                    .name(folder_id, self.client.overview.as_ref())
                    .to_owned()
            }
        } else {
            FILTERS
                .iter()
                .find(|(filter, _, _)| *filter == self.filter)
                .map_or("全部条目", |(_, name, _)| {
                    if *name == "我的保险库" {
                        "全部条目"
                    } else {
                        *name
                    }
                })
                .to_owned()
        };

        let type_counts = self
            .client
            .overview
            .as_ref()
            .map(|o| count_by_type(o, self.client.selected_folder.as_deref()))
            .unwrap_or([0; 9]);

        // Compact single-row header above the list pane.
        let list_header = div()
            .px(px(space::M))
            .pt(px(space::M))
            .pb(px(space::S))
            .flex()
            .flex_col()
            .gap(px(space::S))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::S))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(17.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(heading_title),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_muted))
                                    .child(format!("{count} 项")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .flex_shrink_0()
                            .child(
                                icon_btn(
                                    "sort",
                                    glyph::SORT,
                                    format!("排序：{}（点击切换）", self.sort.label()),
                                    None,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.sort = this.sort.next();
                                        this.settings.sort = this.sort;
                                        this.save_settings(cx);
                                        this.invalidate_query();
                                        this.ui.trigger_list_motion(this.animate);
                                        this.ui.toast.show(
                                            super::toast::ToastKind::Info,
                                            format!("已按「{}」排序", this.sort.label()),
                                            Instant::now(),
                                            this.animate,
                                        );
                                        cx.notify();
                                    },
                                )),
                            )
                            .when(
                                (self.selection_ready()
                                    || (self.selection.active && self.client.ready()))
                                    && self.selection.pending.is_none(),
                                |el| {
                                    el.child(
                                        icon_btn(
                                            "toggle-selection",
                                            glyph::CHECK_BOX,
                                            if self.selection.active {
                                                "退出批量选择"
                                            } else {
                                                "批量选择条目"
                                            },
                                            Some("Ctrl+Shift+S"),
                                        )
                                        .when(self.selection.active, |b| {
                                            b.bg(pal.tint(pal.accent_fill))
                                                .text_color(hex(pal.accent_text))
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_selection(cx)),
                                        ),
                                    )
                                },
                            )
                            .when(self.write_available() && !self.selection.active, |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .child(
                                            btn("new-item", BtnVariant::Primary)
                                                .h(px(28.))
                                                .px(px(10.))
                                                .rounded_r_none()
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(glyph::ADD, 12.))
                                                .child("新增")
                                                .tooltip(|_, cx| {
                                                    make_tooltip("新增条目", Some("Ctrl+N"), cx)
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.ui.close_popovers(this.animate);
                                                    this.new_item(window, cx)
                                                })),
                                        )
                                        .child(
                                            btn("new-item-menu", BtnVariant::Primary)
                                                .h(px(28.))
                                                .w(px(22.))
                                                .px_0()
                                                .rounded_l_none()
                                                .border_l_1()
                                                .border_color(gpui::hsla(0., 0., 1., 0.22))
                                                .child(glyph_icon(glyph::CHEVRON_DOWN, 9.))
                                                .tooltip(|_, cx| {
                                                    make_tooltip("按类型新建条目", None, cx)
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let now = Instant::now();
                                                    this.ui.new_menu_open = !this.ui.new_menu_open;
                                                    this.ui.new_menu_spring.retarget(
                                                        if this.ui.new_menu_open { 1. } else { 0. },
                                                        now,
                                                        this.animate,
                                                    );
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            }),
                    ),
            )
            // Type Filter Chips row (when in All / Kind / Folder views and not batch selecting).
            .when(show_type_chips, |el| {
                el.child(
                    div()
                        .id("type-filter-chips")
                        .w_full()
                        .overflow_x_scroll()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .pb(px(2.))
                        .children(TYPE_CHIPS.iter().enumerate().filter_map(
                            |(i, &(filter, label))| {
                                let chip_count = match filter {
                                    Filter::All => type_counts[0],
                                    Filter::Kind(k) => {
                                        type_counts.get(usize::from(k)).copied().unwrap_or(0)
                                    }
                                    _ => 0,
                                };
                                let active = self.filter == filter;
                                // Always show "全部" and the active chip; hide 0-count unused types to stay crisp.
                                if !active && filter != Filter::All && chip_count == 0 {
                                    return None;
                                }
                                Some(
                                    pill_chip(("type-chip", i), label, Some(chip_count), active)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.filter != filter {
                                                this.selection.clear();
                                                this.filter = filter;
                                                this.invalidate_query();
                                                this.ui.trigger_list_motion(this.animate);
                                                cx.notify();
                                            }
                                        })),
                                )
                            },
                        )),
                )
            })
            // Active folder filter badge chip (clearable with 1 click).
            .when(self.client.selected_folder.is_some(), |el| {
                el.child(folder_navigation)
            })
            // Batch selection toolbar.
            .when(self.selection.active, |el| {
                el.child(self.render_selection_controls(now, cx))
            })
            .when(self.selection.move_picker, |el| {
                el.child(self.render_selection_folders(now, cx))
            });

        let list_opacity = clamp_opacity(0.45 + 0.55 * list_progress);
        let list_y = 8. * (1. - list_progress);

        let list_body = if count == 0 {
            let (icon_code, empty_title, empty_desc) = if !self.query.trim().is_empty() {
                (
                    glyph::SEARCH,
                    format!("没有匹配「{}」的条目", self.query.trim()),
                    "尝试其他关键词或清除搜索条件。".to_owned(),
                )
            } else if self.filter == Filter::Trash {
                (
                    glyph::DELETE,
                    "回收站是空的".to_owned(),
                    "删除的条目会暂时保留在这里。".to_owned(),
                )
            } else if self.filter == Filter::Favorites {
                (
                    glyph::STAR,
                    "还没有收藏的条目".to_owned(),
                    "在条目详情中点亮星标即可快速访问。".to_owned(),
                )
            } else {
                (
                    glyph::SHIELD,
                    "这里还没有条目".to_owned(),
                    "点击「新增」创建你的第一条加密凭据。".to_owned(),
                )
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(empty_state(icon_code, empty_title, empty_desc))
                .when(!self.query.trim().is_empty(), |el| {
                    el.child(
                        btn("empty-clear-search", BtnVariant::Secondary)
                            .child("清除搜索")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.client.search.update(cx, |input, cx| input.set("", cx));
                                this.query.clear();
                                this.invalidate_query();
                                cx.notify();
                            })),
                    )
                })
                .when(
                    self.query.trim().is_empty()
                        && self.write_available()
                        && self.filter != Filter::Trash
                        && self.filter != Filter::Archived,
                    |el| {
                        el.child(
                            btn("empty-new-item", BtnVariant::Primary)
                                .child(glyph_icon(glyph::ADD, 13.))
                                .child("新增条目")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.new_item(window, cx)),
                                ),
                        )
                    },
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .px(px(6.))
                .pb(px(6.))
                .relative()
                .top(px(list_y))
                .opacity(list_opacity)
                .child(
                    uniform_list(
                        "item-list",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let Some(overview) = this.client.overview.as_ref() else {
                                return Vec::new();
                            };
                            range
                                .filter_map(|i| {
                                    let index = *indices.get(i)?;
                                    Some(this.render_vault_row(
                                        index,
                                        overview.items.get(index)?,
                                        now,
                                        cx,
                                    ))
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .size_full(),
                )
                .into_any_element()
        };

        let list_pane = surface_card()
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .when(narrow, |el| el.flex_1())
            .when(!narrow, |el| el.w(px(self.ui.list_width)).flex_shrink_0())
            .child(list_header)
            .child(
                div()
                    .h(px(1.))
                    .mx(px(space::M))
                    .bg(hex(pal.border_subtle))
                    .flex_shrink_0(),
            )
            .child(list_body)
            .when(new_menu_p > 0.02 && self.write_available(), |pane| {
                pane.child(
                    elevated_card()
                        .absolute()
                        .top(px(44. + 8. * (1. - new_menu_p)))
                        .right(px(space::M))
                        .w(px(172.))
                        .p(px(6.))
                        .opacity(clamp_opacity(new_menu_p))
                        .occlude()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .children(
                            [
                                (1u8, "登录", glyph::GLOBE),
                                (2, "安全笔记", "\u{e8a5}"),
                                (3, "银行卡", "\u{e8c7}"),
                                (4, "身份信息", glyph::PERSON),
                                (5, "SSH 密钥", glyph::KEY),
                            ]
                            .into_iter()
                            .map(|(kind, label, icon_code)| {
                                btn(("new-kind", usize::from(kind)), BtnVariant::Subtle)
                                    .w_full()
                                    .justify_start()
                                    .h(px(30.))
                                    .text_size(px(font::CAPTION))
                                    .child(
                                        div()
                                            .text_color(hex(pal.accent_text))
                                            .child(glyph_icon(icon_code, 12.)),
                                    )
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.ui.close_popovers(this.animate);
                                        this.new_item_of_kind(kind, window, cx);
                                    }))
                            }),
                        ),
                )
            });

        // Draggable splitter between List & Detail on wide windows.
        let splitter = div()
            .id("vault-pane-splitter")
            .w(px(10.))
            .h_full()
            .flex_shrink_0()
            .cursor(CursorStyle::ResizeLeftRight)
            .flex()
            .items_center()
            .justify_center()
            .group("splitter")
            .child(
                div()
                    .w(px(if self.ui.resizing_list { 3. } else { 2. }))
                    .h(px(if self.ui.resizing_list { 72. } else { 36. }))
                    .rounded(px(radius::PILL))
                    .bg(if self.ui.resizing_list {
                        hex(pal.accent_fill)
                    } else {
                        hex(pal.border)
                    })
                    .group_hover("splitter", move |s| {
                        s.w(px(3.)).h(px(64.)).bg(hex(pal.accent_fill))
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.ui.resizing_list = true;
                    this.ui.resize_origin_x = f32::from(event.position.x);
                    this.ui.resize_start_width = this.ui.list_width;
                    cx.notify();
                }),
            );

        let right_pane = if self.client.editor.is_some() {
            self.render_inline_editor_pane(narrow, now, cx)
                .into_any_element()
        } else {
            self.render_detail_pane(narrow, now, cx).into_any_element()
        };

        let show_right = !narrow || self.selected.is_some() || self.client.editor.is_some();
        let show_list = !narrow || (!show_right);
        let (narrow_p, _) =
            self.ui
                .vault_narrow_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let narrow_x = if narrow {
            22. * self.ui.vault_narrow_dir * (1. - narrow_p)
        } else {
            0.
        };
        let narrow_opacity = if narrow {
            clamp_opacity(0.35 + 0.65 * narrow_p)
        } else {
            1.
        };

        div()
            .relative()
            .left(px(narrow_x))
            .opacity(narrow_opacity)
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .when(show_list, |el| el.child(list_pane))
            .when(!narrow, |el| el.child(splitter))
            .when(show_right, |el| el.child(right_pane))
    }

    fn render_vault_row(
        &self,
        index: usize,
        item: &Item,
        now: Instant,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let pal = theme::palette();
        let id = item.id.clone();
        let keyboard_id = id.clone();
        let submit_id = id.clone();
        let restricted = item.restricted;
        let checked = self.selection.ids.contains(&item.id);
        let is_selected = if self.selection.active {
            checked
        } else {
            self.selected == Some(index)
        };
        let (sel_p, _) =
            self.ui
                .selection_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let (copy_pulse, _) =
            self.ui
                .copy_pulse
                .sample_staggered(now, Duration::ZERO, self.animate);

        // Row actions authorize and fetch the target detail on demand.
        let can_quick_copy =
            !self.selection.active && !restricted && !item.deleted && item.item_type == 1;
        let show_totp_quick_copy = self.selected_item_id.as_deref() == Some(item.id.as_str())
            && self
                .client
                .detail
                .as_ref()
                .is_some_and(|detail| detail.totp.is_some());

        let group_name: gpui::SharedString = format!("vault-row-{index}").into();
        let copy_user_tag = format!("row-user-{}", item.id);
        let copy_pass_tag = format!("row-pass-{}", item.id);
        let copy_totp_tag = format!("row-totp-{}", item.id);
        let user_copied = self.ui.is_recently_copied(&copy_user_tag, now);
        let pass_copied = self.ui.is_recently_copied(&copy_pass_tag, now);
        let totp_copied = self.ui.is_recently_copied(&copy_totp_tag, now);
        let any_copied = user_copied || pass_copied || totp_copied;
        let quick_copy_id = item.id.clone();
        let quick_bg = if is_selected {
            hex(pal.card)
        } else {
            hex(pal.hover)
        };

        div()
            .id(("item", index))
            .group(group_name.clone())
            .cursor_pointer()
            .relative()
            .w_full()
            .h(px(size::ROW))
            .px(px(10.))
            .rounded(px(radius::CARD - 2.))
            .flex()
            .items_center()
            .gap(px(10.))
            .when(is_selected, |row| {
                row.bg(pal.tint(pal.accent_fill))
                    .border_1()
                    .border_color(pal.tint_border(pal.accent_fill))
                    .child(
                        div()
                            .absolute()
                            .left(px(2.))
                            .top(px(14.))
                            .w(px(3.))
                            .h(px(30.))
                            .rounded(px(radius::PILL))
                            .bg(hex(pal.accent_fill)),
                    )
            })
            .when(!is_selected, |row| {
                row.border_1()
                    .border_color(gpui::hsla(0., 0., 0., 0.))
                    .hover(move |s| s.bg(hex(pal.hover)))
            })
            .when(self.selection.active || sel_p > 0.02, |row| {
                row.when(self.selection.active, |r| r.focusable().tab_index(0))
                    .child(
                        div()
                            .w(px((20. * sel_p).clamp(0., 22.)))
                            .overflow_hidden()
                            .opacity(clamp_opacity(sel_p))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .size(px(20.))
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(hex(if restricted {
                                        pal.border
                                    } else if checked {
                                        pal.accent_fill
                                    } else {
                                        pal.text_muted
                                    }))
                                    .when(checked && !restricted, |b| b.bg(hex(pal.accent_fill)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(checked && !restricted, |b| {
                                        b.text_color(hex(pal.on_accent))
                                            .child(glyph_icon(glyph::CHECK, 11.))
                                    }),
                            ),
                    )
                    .when(self.selection.active, |r| {
                        r.on_action(cx.listener(move |this, _: &Submit, _, cx| {
                            cx.stop_propagation();
                            if !restricted {
                                this.toggle_selected_id(&submit_id, cx);
                            }
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    if !restricted {
                                        this.toggle_selected_id(&keyboard_id, cx);
                                    }
                                }
                            },
                        ))
                    })
            })
            .child(self.render_item_tile(item, size::FAVICON, 8., cx))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(font::BODY))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(item.name.clone()),
                            )
                            .when(item.favorite, |el| {
                                el.child(
                                    div()
                                        .text_color(hex(pal.warning))
                                        .flex_shrink_0()
                                        .child(glyph_icon(glyph::STAR_SOLID, 10.)),
                                )
                            })
                            .when(item.reprompt, |el| {
                                el.child(
                                    div()
                                        .text_color(hex(pal.warning))
                                        .flex_shrink_0()
                                        .child(glyph_icon(glyph::LOCK, 10.)),
                                )
                            })
                            .when(item.organization_id.is_some(), |el| {
                                el.child(
                                    div()
                                        .text_color(hex(pal.text_muted))
                                        .flex_shrink_0()
                                        .child(glyph_icon(glyph::BUILDING, 10.)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child(if item.subtitle.is_empty() {
                                type_label(item.item_type).to_owned()
                            } else {
                                item.subtitle.clone()
                            }),
                    ),
            )
            // Hover/focus revealed quick-copy pill cluster floating cleanly on the right edge
            // so titles and usernames keep full horizontal breathing room at rest.
            .when(can_quick_copy, |row| {
                let username_id = quick_copy_id.clone();
                let password_id = quick_copy_id.clone();
                let totp_id = quick_copy_id.clone();
                row.child(
                    div()
                        .absolute()
                        .right(px(8.))
                        .top(px(14.))
                        .px(px(3.))
                        .py(px(2.))
                        .rounded(px(radius::CONTROL))
                        .bg(quick_bg)
                        .border_1()
                        .border_color(hex(pal.border_subtle))
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .opacity(if any_copied { 1. } else { 0. })
                        .group_hover(group_name.clone(), |s| s.opacity(1.))
                        .child(
                            icon_btn(
                                ("row-copy-user", index),
                                if user_copied {
                                    glyph::CHECK
                                } else {
                                    glyph::PERSON
                                },
                                "复制此条目用户名",
                                None,
                            )
                            .size(px(24.))
                            .when(user_copied, |b| {
                                b.relative()
                                    .top(px(-copy_pulse * 2.5))
                                    .text_color(hex(pal.success))
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_quick_copy(
                                        username_id.clone(),
                                        QuickCopyAction::Username,
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(
                            icon_btn(
                                ("row-copy-pass", index),
                                if pass_copied {
                                    glyph::CHECK
                                } else {
                                    glyph::KEY
                                },
                                "复制此条目密码",
                                None,
                            )
                            .size(px(24.))
                            .when(pass_copied, |b| {
                                b.relative()
                                    .top(px(-copy_pulse * 2.5))
                                    .text_color(hex(pal.success))
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_quick_copy(
                                        password_id.clone(),
                                        QuickCopyAction::Password,
                                        cx,
                                    );
                                },
                            )),
                        )
                        .when(show_totp_quick_copy || totp_copied, |cluster| {
                            cluster.child(
                                icon_btn(
                                    ("row-copy-totp", index),
                                    if totp_copied {
                                        glyph::CHECK
                                    } else {
                                        glyph::CLOCK
                                    },
                                    "复制此条目验证码",
                                    Some("Ctrl+Shift+T"),
                                )
                                .size(px(24.))
                                .when(totp_copied, |b| {
                                    b.relative()
                                        .top(px(-copy_pulse * 2.5))
                                        .text_color(hex(pal.success))
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.request_quick_copy(
                                            totp_id.clone(),
                                            QuickCopyAction::Totp,
                                            cx,
                                        );
                                    },
                                )),
                            )
                        }),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.selection.active {
                    if !restricted {
                        this.toggle_selected_id(&id, cx);
                    }
                } else {
                    this.choose_item(index, cx);
                }
            }))
    }

    /// Render a crisp favicon tile or vibrant monogram tile for an item.
    pub fn render_item_tile(
        &self,
        item: &Item,
        tile_size: f32,
        corner: f32,
        cx: &Context<Self>,
    ) -> Div {
        let pal = theme::palette();
        let fallback = || {
            item_monogram(
                &item.name,
                item.item_type,
                vault_list::icon_for(item),
                tile_size,
                corner,
            )
        };
        if !self.settings.website_icons {
            return fallback();
        }
        let Some(account) = self.client.active() else {
            return fallback();
        };
        let server_mode = account.server_mode;
        let server_url = account.server_url.clone();
        match self.favicon_cache.lookup(item.website_host.as_deref()) {
            Some(crate::favicon::Lookup::Ready(image)) => div()
                .size(px(tile_size))
                .rounded(px(corner))
                .bg(hex(pal.layer))
                .border_1()
                .border_color(hex(pal.border_subtle))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    gpui::img(image)
                        .size(px((tile_size * 0.70).round()))
                        .object_fit(gpui::ObjectFit::Contain),
                ),
            Some(crate::favicon::Lookup::Start {
                host,
                request_id,
                registration,
            }) => {
                let executor = cx.background_executor().clone();
                let timeout_executor = executor.clone();
                let host_for_completion = host.clone();
                let cache = self.favicon_cache.clone();
                let worker = executor.spawn(async move {
                    cache
                        .fetch_icon(
                            timeout_executor,
                            host,
                            server_mode,
                            server_url,
                            registration,
                        )
                        .await
                });
                cx.spawn(async move |weak, app| {
                    let image = worker.await;
                    let _ = weak.update(app, |this, cx| {
                        if this
                            .favicon_cache
                            .complete(&host_for_completion, request_id, image)
                        {
                            cx.notify();
                        }
                    });
                })
                .detach();
                fallback()
            }
            _ => fallback(),
        }
    }

    /// Centered Hero welcome / lock / sync gate screen when no vault overview is active.
    fn render_vault_gate_screen(&self, cx: &mut Context<Self>) -> Div {
        let pal = theme::palette();
        let session = self.client.state();
        let active_account = self.client.active();
        let (title, subtitle) = match session {
            Session::Locked => (
                "保险库已锁定",
                "使用主密码离线解锁本机快照，或重新同步最新数据。",
            ),
            Session::Unlocked => (
                "准备就绪，开始同步",
                "连接服务器拉取并解密你的 Bitwarden 保险库。",
            ),
            Session::SignedOut => (
                "欢迎使用 Bytewarden",
                "原生 Rust 驱动的桌面密码保险库，安全、轻盈、即刻响应。",
            ),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(space::XL))
            .child(
                surface_card()
                    .w_full()
                    .max_w(px(420.))
                    .p(px(space::XXL))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(space::L))
                    .child(brand_logo(72.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(font::SUBTITLE))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_center()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child(subtitle),
                            ),
                    )
                    .when_some(active_account, |el, acc| {
                        el.child(
                            div()
                                .w_full()
                                .p(px(10.))
                                .rounded(px(radius::CONTROL))
                                .bg(hex(pal.layer))
                                .border_1()
                                .border_color(hex(pal.border_subtle))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(hex(pal.text))
                                        .child(acc.email.clone()),
                                )
                                .child(Self::render_server_badge(acc.server_mode, &acc.server_url)),
                        )
                    })
                    .when(
                        !self.client.initialized && self.client.busy.is_none(),
                        |el| {
                            el.child(
                                btn("retry-initialization", BtnVariant::Primary)
                                    .debug_selector(|| "retry-initialization".into())
                                    .w_full()
                                    .h(px(38.))
                                    .child(glyph_icon(glyph::SYNC, 15.))
                                    .child("重试初始化")
                                    .on_click(cx.listener(|this, _, _, cx| this.initialize(cx))),
                            )
                        },
                    )
                    .when(self.client.ready(), |el| {
                        el.child(match session {
                            Session::Locked => btn("unlock", BtnVariant::Primary)
                                .w_full()
                                .h(px(38.))
                                .child(glyph_icon(glyph::UNLOCK, 15.))
                                .child("解锁保险库")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.open_unlock(window, cx)),
                                ),
                            Session::Unlocked => btn("first-sync", BtnVariant::Primary)
                                .w_full()
                                .h(px(38.))
                                .child(glyph_icon(glyph::SYNC, 15.))
                                .child("同步保险库")
                                .on_click(cx.listener(|this, _, _, cx| this.request_sync(cx))),
                            Session::SignedOut => btn("login", BtnVariant::Primary)
                                .w_full()
                                .h(px(38.))
                                .child(glyph_icon(glyph::PERSON, 15.))
                                .child("连接 Bitwarden 账户")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.begin_sign_in(window, cx)
                                })),
                        })
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(badge("US / EU / 自托管", pal.info))
                            .child(badge("本地 DPAPI 加密", pal.success)),
                    ),
            )
    }

    pub(crate) fn complete_pending_quick_copy(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_quick_copy.take() else {
            return;
        };
        if self.selected_item_id.as_deref() != Some(pending.item_id.as_str())
            || self.client.detail.is_none()
        {
            return;
        }
        match pending.action {
            QuickCopyAction::Username => {
                self.quick_copy_named_field("用户名", &format!("row-user-{}", pending.item_id), cx)
            }
            QuickCopyAction::Password => {
                self.quick_copy_named_field("密码", &format!("row-pass-{}", pending.item_id), cx)
            }
            QuickCopyAction::Totp => {
                self.quick_copy_totp_with_tag(&format!("row-totp-{}", pending.item_id), cx)
            }
        }
        self.close_detail();
        cx.notify();
    }

    /// Quick-copy helper for shortcuts and item-row actions.
    pub fn quick_copy_named_field(
        &mut self,
        target_label: &str,
        tag: &str,
        cx: &mut Context<Self>,
    ) {
        let field = self.client.detail.as_ref().and_then(|detail| {
            detail
                .fields
                .iter()
                .find(|field| field.label.contains(target_label) && !field.value.is_empty())
                .map(|field| (field.label.clone(), Zeroizing::new(field.value.clone())))
        });
        if let Some((label, value)) = field {
            self.copy_protected_labeled(value, tag, &label, cx);
        } else {
            self.client.notice = format!("此登录条目没有可复制的{target_label}。");
            cx.notify();
        }
    }

    pub fn quick_copy_totp(&mut self, cx: &mut Context<Self>) {
        self.quick_copy_totp_with_tag("detail-totp", cx);
    }

    fn quick_copy_totp_with_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if !self.client.ready() || !self.totp_allowed_now() {
            self.client.notice = "当前无权读取此条目的验证码。".into();
            cx.notify();
            return;
        }
        if let Some(code) = self.current_totp_code() {
            self.copy_protected_labeled(code, tag, "验证码", cx);
        } else {
            self.client.notice = "此登录条目没有可用的验证码。".into();
            cx.notify();
        }
    }
}

pub fn type_label(item_type: u8) -> &'static str {
    FILTERS
        .iter()
        .find(|(filter, _, _)| *filter == Filter::Kind(item_type))
        .map_or("条目", |(_, label, _)| *label)
}

fn count_by_type(overview: &Overview, folder: Option<&str>) -> [usize; 9] {
    let mut counts = [0usize; 9];
    for item in &overview.items {
        if item.deleted || item.archived {
            continue;
        }
        if let Some(folder_id) = folder
            && item.folder_id.as_deref().unwrap_or("") != folder_id
        {
            continue;
        }
        counts[0] += 1;
        let idx = usize::from(item.item_type);
        if (1..=8).contains(&idx) {
            counts[idx] += 1;
        }
    }
    counts
}
