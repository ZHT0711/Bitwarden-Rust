//! Upgraded Detail Hero, staggered Fluent FieldGroup cards, syntax-colored passwords,
//! Canvas TOTP countdown ring, collapsible metadata, and Inline Detail Editor pane.

use std::time::{Duration, Instant};

use bytewarden_core::desktop::{Detail, Field, ItemAction};
use gpui::{Context, Div, MouseButton, Stateful, div, prelude::*, px};
use zeroize::Zeroizing;

use crate::{
    VaultPreview,
    theme::{self, hex},
};

use super::{
    components::{
        BtnVariant, badge, btn, card_group, detail_skeleton, elevated_card, empty_state,
        format_totp, glyph_icon, icon_btn, make_tooltip, password_colored, rotating_chevron,
        strength_meter, surface_card, totp_ring,
    },
    spring::clamp_opacity,
    tokens::{family, font, glyph, radius, size, space},
    vault_view::type_label,
};

impl VaultPreview {
    pub fn render_detail_pane(
        &self,
        narrow: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pal = theme::palette();
        let detail = self.client.detail.as_ref();
        let item = self
            .selected
            .and_then(|index| self.client.overview.as_ref()?.items.get(index));

        // Sample staggered spring offsets for buttery Hero -> Credentials -> Tools entrance.
        let (hero_p, _) = self
            .ui
            .detail_spring
            .sample_staggered(now, Duration::ZERO, self.animate);
        let (fields_p, _) =
            self.ui
                .detail_spring
                .sample_staggered(now, Duration::from_millis(35), self.animate);
        let (tools_p, _) =
            self.ui
                .detail_spring
                .sample_staggered(now, Duration::from_millis(70), self.animate);
        let (fav_pulse, _) =
            self.ui
                .favorite_pulse
                .sample_staggered(now, Duration::ZERO, self.animate);

        let mut container = surface_card()
            .id("details")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        if narrow {
            container = container.child(
                div()
                    .px(px(space::L))
                    .pt(px(space::M))
                    .pb(px(space::S))
                    .border_b_1()
                    .border_color(hex(pal.border_subtle))
                    .child(
                        btn("back", BtnVariant::Subtle)
                            .h(px(30.))
                            .px(px(8.))
                            .child(glyph_icon(glyph::BACK, 13.))
                            .child("返回条目列表")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ui.trigger_vault_narrow(false, this.animate);
                                this.close_detail();
                                cx.notify();
                            })),
                    ),
            );
        }

        let Some(detail) = detail else {
            return if item.is_some() {
                let elapsed = self
                    .ui
                    .detail_loading_since
                    .map_or(Duration::ZERO, |started| {
                        now.saturating_duration_since(started)
                    });
                if elapsed >= Duration::from_millis(120) {
                    let phase = elapsed.as_secs_f32() * 0.75;
                    container.child(detail_skeleton(phase))
                } else {
                    // Hold calm canvas during fast local decryption (< 120ms) to avoid empty-state flash.
                    container
                }
            } else {
                container.child(div().flex_1().flex().items_center().justify_center().child(
                    empty_state(
                        glyph::SHIELD,
                        "选择一个条目查看详情",
                        "点击左侧列表中的任意条目，或按 Ctrl+K 快速检索。",
                    ),
                ))
            };
        };

        let can_mutate = item.is_some()
            && self.client.ready()
            && self
                .client
                .overview
                .as_ref()
                .is_some_and(|overview| !overview.read_only);

        let folder_id = item.and_then(|it| it.folder_id.clone());
        let folder_label = folder_id.as_deref().map(|id| {
            self.folders
                .name(id, self.client.overview.as_ref())
                .to_owned()
        });

        // 1. Detail Hero Header Card (stagger 0ms).
        let hero_icon = if let Some(it) = item {
            self.render_item_tile(it, size::HERO_ICON, 12., cx)
        } else {
            super::components::item_monogram(
                &detail.name,
                detail.item_type,
                glyph::SHIELD,
                size::HERO_ICON,
                12.,
            )
        };

        let is_favorite = item.is_some_and(|it| it.favorite);
        let is_deleted = item.is_some_and(|it| it.deleted);
        let is_archived = item.is_some_and(|it| it.archived);
        let is_restricted = item.is_some_and(|it| it.restricted) || detail.restricted;

        let hero = div()
            .relative()
            .top(px(12. * (1. - hero_p)))
            .opacity(clamp_opacity(hero_p))
            .flex()
            .items_start()
            .justify_between()
            .gap(px(space::M))
            .pb(px(space::M))
            .border_b_1()
            .border_color(hex(pal.border_subtle))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .min_w_0()
                    .flex_1()
                    .child(hero_icon)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(font::SUBTITLE))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(detail.name.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(badge(type_label(detail.item_type), pal.info))
                                    .when_some(folder_label, |el, name| {
                                        let target_folder = folder_id.clone();
                                        el.child(
                                            div()
                                                .id("detail-folder-jump")
                                                .cursor_pointer()
                                                .px(px(8.))
                                                .py(px(2.))
                                                .rounded(px(radius::PILL))
                                                .bg(hex(pal.layer))
                                                .border_1()
                                                .border_color(hex(pal.border))
                                                .flex()
                                                .items_center()
                                                .gap(px(4.))
                                                .text_size(px(font::MICRO))
                                                .text_color(hex(pal.text_secondary))
                                                .hover(move |s| {
                                                    s.bg(hex(pal.hover)).text_color(hex(pal.text))
                                                })
                                                .child(glyph_icon(glyph::FOLDER, 10.))
                                                .child(name)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_vault_view(
                                                        crate::vault_list::Filter::All,
                                                        target_folder.clone(),
                                                        cx,
                                                    );
                                                })),
                                        )
                                    })
                                    .when(is_archived, |el| el.child(badge("已归档", pal.warning)))
                                    .when(is_deleted, |el| el.child(badge("回收站", pal.danger)))
                                    .when(item.is_some_and(|it| it.reprompt), |el| {
                                        el.child(badge("主密码复验", pal.warning))
                                    }),
                            ),
                    ),
            )
            .when(can_mutate, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .flex_shrink_0()
                        .when(!is_deleted && !is_restricted, |bar| {
                            bar.child(
                                icon_btn(
                                    "hero-favorite",
                                    if is_favorite {
                                        glyph::STAR_SOLID
                                    } else {
                                        glyph::STAR
                                    },
                                    if is_favorite {
                                        "取消收藏"
                                    } else {
                                        "收藏此条目"
                                    },
                                    None,
                                )
                                .relative()
                                .top(px(-fav_pulse * 4.))
                                .when(is_favorite, |b| b.text_color(hex(pal.warning)))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.ui.favorite_pulse.impulse(
                                            14.,
                                            Instant::now(),
                                            this.animate,
                                        );
                                        this.begin_action(
                                            if is_favorite {
                                                ItemAction::Unfavorite
                                            } else {
                                                ItemAction::Favorite
                                            },
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                btn("edit-item", BtnVariant::Primary)
                                    .h(px(30.))
                                    .px(px(12.))
                                    .text_size(px(font::CAPTION))
                                    .child(glyph_icon(glyph::EDIT, 12.))
                                    .child("编辑")
                                    .tooltip(|_, cx| make_tooltip("编辑条目", Some("Ctrl+E"), cx))
                                    .on_click(cx.listener(|this, _, _, cx| this.edit_item(cx))),
                            )
                        })
                        .when(is_deleted, |bar| {
                            bar.child(
                                btn("hero-restore", BtnVariant::Tinted)
                                    .h(px(30.))
                                    .text_size(px(font::CAPTION))
                                    .child(glyph_icon(glyph::RESTORE, 12.))
                                    .child("还原")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.begin_action(ItemAction::Restore, cx)
                                    })),
                            )
                            .child(
                                btn("hero-delete-perm", BtnVariant::Danger)
                                    .h(px(30.))
                                    .text_size(px(font::CAPTION))
                                    .child(glyph_icon(glyph::DELETE, 12.))
                                    .child("永久删除")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.begin_action(ItemAction::Delete, cx)
                                    })),
                            )
                        })
                        .when(!is_deleted, |bar| {
                            bar.child(
                                icon_btn("hero-more", glyph::MORE, "更多操作", None)
                                    .when(self.ui.item_menu_open, |b| b.bg(hex(pal.selected)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let now = Instant::now();
                                        this.ui.item_menu_open = !this.ui.item_menu_open;
                                        this.ui.item_menu_spring.retarget(
                                            if this.ui.item_menu_open { 1. } else { 0. },
                                            now,
                                            this.animate,
                                        );
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            });

        // 2. Group fields cleanly into Credentials/Main, Websites, Notes, Other.
        let mut primary_fields: Vec<(usize, &Field)> = Vec::new();
        let mut website_fields: Vec<(usize, &Field)> = Vec::new();
        let mut note_fields: Vec<(usize, &Field)> = Vec::new();
        for (i, field) in detail.fields.iter().enumerate() {
            if field.launch_uri.is_some()
                || field.label.contains("网站")
                || field.label.contains("URI")
            {
                website_fields.push((i, field));
            } else if field.label == "备注" || field.value.contains('\n') {
                note_fields.push((i, field));
            } else {
                primary_fields.push((i, field));
            }
        }

        let mut cards_stack = div()
            .relative()
            .top(px(14. * (1. - fields_p)))
            .opacity(clamp_opacity(fields_p))
            .flex()
            .flex_col()
            .gap(px(space::M));

        if !primary_fields.is_empty() || detail.totp.is_some() {
            let mut cred_card = card_group("凭据与信息", Some(glyph::KEY));
            let total_rows = primary_fields.len();
            for (pos, (idx, field)) in primary_fields.into_iter().enumerate() {
                let is_last = pos + 1 == total_rows && detail.totp.is_none();
                cred_card = cred_card.child(self.render_field_row(idx, field, is_last, now, cx));
            }
            if detail.totp.is_some() {
                cred_card = cred_card.child(self.render_totp_row(detail, now, cx));
            }
            if let Some(score) = detail.password_strength {
                cred_card = cred_card.child(
                    div()
                        .px(px(space::L))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(hex(pal.border_subtle))
                        .child(strength_meter(score)),
                );
            }
            cards_stack = cards_stack.child(cred_card);
        }

        if !website_fields.is_empty() {
            let mut web_card = card_group("网站与地址", Some(glyph::GLOBE));
            let total = website_fields.len();
            for (pos, (idx, field)) in website_fields.into_iter().enumerate() {
                web_card =
                    web_card.child(self.render_field_row(idx, field, pos + 1 == total, now, cx));
            }
            cards_stack = cards_stack.child(web_card);
        }

        if !note_fields.is_empty() {
            let mut note_card = card_group("安全备注与长文本", Some("\u{e8a5}"));
            let total = note_fields.len();
            for (pos, (idx, field)) in note_fields.into_iter().enumerate() {
                note_card =
                    note_card.child(self.render_field_row(idx, field, pos + 1 == total, now, cx));
            }
            cards_stack = cards_stack.child(note_card);
        }

        // 3. Tools / Passkeys / Attachments / Collapsible Meta (stagger +70ms).
        let tools_section = div()
            .relative()
            .top(px(16. * (1. - tools_p)))
            .opacity(clamp_opacity(tools_p))
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(self.render_detail_extras(detail, now, cx));

        // Floating `⋯` item actions menu.
        let (menu_p, _) =
            self.ui
                .item_menu_spring
                .sample_staggered(now, Duration::ZERO, self.animate);

        let inner = div()
            .relative()
            .w_full()
            .max_w(px(size::DETAIL_MAX))
            .mx_auto()
            .p(px(space::XL))
            .flex()
            .flex_col()
            .gap(px(space::L))
            .child(hero)
            .child(cards_stack)
            .child(tools_section)
            .when(can_mutate && !is_deleted && menu_p > 0.02, |el| {
                el.child(
                    elevated_card()
                        .absolute()
                        .top(px(62. + 8. * (1. - menu_p)))
                        .right(px(space::XL))
                        .w(px(184.))
                        .p(px(6.))
                        .opacity(clamp_opacity(menu_p))
                        .occlude()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            btn("menu-archive", BtnVariant::Subtle)
                                .w_full()
                                .justify_start()
                                .h(px(32.))
                                .text_size(px(13.))
                                .child(glyph_icon(glyph::ARCHIVE, 13.))
                                .child(if is_archived {
                                    "取消归档"
                                } else {
                                    "归档"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ui.close_popovers(this.animate);
                                    this.begin_action(
                                        if is_archived {
                                            ItemAction::Unarchive
                                        } else {
                                            ItemAction::Archive
                                        },
                                        cx,
                                    );
                                })),
                        )
                        .when(!is_restricted, |menu| {
                            menu.child(
                                btn("menu-clone", BtnVariant::Subtle)
                                    .w_full()
                                    .justify_start()
                                    .h(px(32.))
                                    .text_size(px(13.))
                                    .child(glyph_icon(glyph::COPY, 13.))
                                    .child("克隆条目")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.ui.close_popovers(this.animate);
                                        this.clone_item(cx);
                                    })),
                            )
                        })
                        .child(
                            btn("menu-trash", BtnVariant::Danger)
                                .w_full()
                                .justify_start()
                                .h(px(32.))
                                .text_size(px(13.))
                                .child(glyph_icon(glyph::DELETE, 13.))
                                .child("移至回收站")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.ui.close_popovers(this.animate);
                                    this.begin_action(ItemAction::Trash, cx);
                                })),
                        ),
                )
            });

        container.child(inner)
    }

    fn render_field_row(
        &self,
        index: usize,
        field: &Field,
        is_last: bool,
        now: Instant,
        cx: &Context<Self>,
    ) -> Div {
        let pal = theme::palette();
        let shown = self.client.revealed.contains(&index);
        let copy_tag = format!("detail-field-{index}");
        let copied = self.ui.is_recently_copied(&copy_tag, now);
        let (copy_pulse, _) =
            self.ui
                .copy_pulse
                .sample_staggered(now, Duration::ZERO, self.animate);
        let is_password_like =
            field.sensitive || field.label.contains("密码") || field.label.contains("密钥");

        let value_view = if field.sensitive && !shown {
            div()
                .font_family(family::MONO)
                .text_size(px(font::BODY))
                .text_color(hex(pal.text_secondary))
                .child("••••••••••••")
        } else if is_password_like && !field.value.contains('\n') {
            password_colored(&self.client.rendered_secrets, &field.value, font::BODY)
        } else {
            div()
                .text_size(px(font::BODY))
                .text_color(hex(pal.text))
                .when(field.value.contains('\n'), |el| {
                    el.font_family(family::MONO)
                })
                .child(self.client.rendered_secrets.text(&field.value, false))
        };

        let copy_tag_click = copy_tag.clone();
        let label_for_toast = field.label.clone();

        div()
            .px(px(space::L))
            .py(px(10.))
            .when(!is_last, |el| {
                el.border_b_1().border_color(hex(pal.border_subtle))
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(px(space::M))
            .hover(move |s| s.bg(hex(pal.hover)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child(field.label.clone()),
                    )
                    .child(value_view),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .flex_shrink_0()
                    .when(field.sensitive, |el| {
                        el.child(
                            icon_btn(
                                ("reveal", index),
                                if shown { glyph::EYE_OFF } else { glyph::EYE },
                                if shown {
                                    "隐藏内容"
                                } else {
                                    "显示内容"
                                },
                                None,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.client.rendered_secrets.clear();
                                    if !this.client.revealed.remove(&index) {
                                        this.client.revealed.insert(index);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .when(field.launch_uri.is_some(), |el| {
                        el.child(
                            icon_btn(
                                ("open-website", index),
                                glyph::OPEN_EXTERNAL,
                                "在浏览器中打开",
                                None,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(uri) = this
                                        .client
                                        .detail
                                        .as_ref()
                                        .and_then(|detail| detail.fields.get(index))
                                        .and_then(|field| field.launch_uri.as_deref())
                                    {
                                        cx.open_url(uri);
                                    }
                                },
                            )),
                        )
                    })
                    .when(!field.value.is_empty(), |el| {
                        el.child(
                            icon_btn(
                                ("copy-field", index),
                                if copied { glyph::CHECK } else { glyph::COPY },
                                if copied {
                                    "已复制"
                                } else {
                                    "复制到剪贴板"
                                },
                                None,
                            )
                            .when(copied, |b| {
                                b.relative()
                                    .top(px(-copy_pulse * 2.5))
                                    .bg(pal.tint(pal.success))
                                    .text_color(hex(pal.success))
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(value) = this
                                        .client
                                        .detail
                                        .as_ref()
                                        .and_then(|detail| detail.fields.get(index))
                                    {
                                        this.copy_protected_labeled(
                                            Zeroizing::new(value.value.clone()),
                                            &copy_tag_click,
                                            &label_for_toast,
                                            cx,
                                        );
                                    }
                                },
                            )),
                        )
                    }),
            )
    }

    fn render_totp_row(&self, detail: &Detail, now: Instant, cx: &Context<Self>) -> Div {
        let pal = theme::palette();
        let Some(totp_view) = self.totp_display_state(detail) else {
            return div();
        };
        let copied = self.ui.is_recently_copied("detail-totp", now);
        let (copy_pulse, _) =
            self.ui
                .copy_pulse
                .sample_staggered(now, Duration::ZERO, self.animate);

        div()
            .px(px(space::L))
            .py(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(space::M))
            .hover(move |s| s.bg(hex(pal.hover)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child("一次性验证码（TOTP）"),
                    )
                    .child(match &totp_view.code {
                        Some(code) => div()
                            .font_family(family::MONO)
                            .text_size(px(font::TOTP))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(hex(if totp_view.urgent {
                                pal.danger
                            } else {
                                pal.accent_text
                            }))
                            .child(
                                self.client
                                    .rendered_secrets
                                    .text(&Zeroizing::new(format_totp(code)), false),
                            ),
                        None => div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_muted))
                            .child(totp_view.status_message.clone()),
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_shrink_0()
                    .when_some(totp_view.remaining_secs, |el, secs| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(totp_ring(totp_view.fraction, totp_view.urgent, 28.))
                                .child(
                                    div()
                                        .w(px(24.))
                                        .font_family(family::MONO)
                                        .text_size(px(font::CAPTION))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(hex(if totp_view.urgent {
                                            pal.danger
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .child(format!("{secs}s")),
                                ),
                        )
                    })
                    .when(totp_view.can_copy, |el| {
                        el.child(
                            icon_btn(
                                "copy-totp",
                                if copied { glyph::CHECK } else { glyph::COPY },
                                "复制验证码",
                                Some("Ctrl+Shift+T"),
                            )
                            .when(copied, |b| {
                                b.relative()
                                    .top(px(-copy_pulse * 2.5))
                                    .bg(pal.tint(pal.success))
                                    .text_color(hex(pal.success))
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.quick_copy_totp(cx))),
                        )
                    }),
            )
    }

    fn render_detail_extras(&self, detail: &Detail, now: Instant, cx: &Context<Self>) -> Div {
        let pal = theme::palette();
        let busy = !self.client.ready();
        let mut stack = div().flex().flex_col().gap(px(space::M));

        if !detail.passkeys.is_empty() {
            let mut card = card_group("通行密钥（Passkeys）", Some(glyph::PASSKEY));
            for (i, key) in detail.passkeys.iter().enumerate() {
                card = card.child(
                    div()
                        .px(px(space::L))
                        .py(px(10.))
                        .when(i + 1 < detail.passkeys.len(), |el| {
                            el.border_b_1().border_color(hex(pal.border_subtle))
                        })
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(space::S))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(font::BODY))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(hex(pal.text))
                                        .child(key.relying_party.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(font::CAPTION))
                                        .text_color(hex(pal.text_secondary))
                                        .child(format!(
                                            "{} · 创建于 {}",
                                            key.user_name.as_deref().unwrap_or("通行密钥"),
                                            key.created_at
                                        )),
                                ),
                        )
                        .child(badge("已保存", pal.success)),
                );
            }
            stack = stack.child(card);
        }

        if !detail.attachment_files.is_empty()
            || detail.can_upload_attachments
            || detail.attachment_failures > 0
        {
            let mut card = card_group("加密附件", Some(glyph::ATTACH));
            if detail.attachment_failures > 0 {
                card = card.child(
                    div()
                        .px(px(space::L))
                        .py(px(8.))
                        .text_size(px(font::CAPTION))
                        .text_color(hex(pal.warning))
                        .child(format!(
                            "{} 个附件元数据无法读取",
                            detail.attachment_failures
                        )),
                );
            }
            for (i, file) in detail.attachment_files.iter().enumerate() {
                let download = (file.id.clone(), file.name.clone());
                let delete = file.id.clone();
                let confirm = file.id.clone();
                let size_text = file.size.map(format_bytes);
                card =
                    card.child(
                        div()
                            .px(px(space::L))
                            .py(px(10.))
                            .border_b_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(space::S))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_color(hex(pal.accent_text))
                                            .child(glyph_icon(glyph::ATTACH, 14.)),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(font::BODY))
                                            .text_color(hex(pal.text))
                                            .child(file.name.clone()),
                                    )
                                    .when_some(size_text, |el, s| {
                                        el.child(
                                            div()
                                                .text_size(px(font::MICRO))
                                                .text_color(hex(pal.text_muted))
                                                .child(s),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .flex_shrink_0()
                                    .when(
                                        !busy
                                            && file.can_download
                                            && detail.attachment_lease().is_some(),
                                        |el| {
                                            el.child(
                                                icon_btn(
                                                    ("download", i),
                                                    glyph::DOWNLOAD,
                                                    "下载附件",
                                                    None,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.pick_attachment_ui(
                                                        Some(download.clone()),
                                                        cx,
                                                    )
                                                })),
                                            )
                                        },
                                    )
                                    .when(
                                        !busy
                                            && detail.can_delete_attachments
                                            && detail.attachment_lease().is_some()
                                            && self.confirm_attachment.as_deref()
                                                != Some(file.id.as_str()),
                                        |el| {
                                            el.child(
                                                icon_btn(
                                                    ("delete-attachment", i),
                                                    glyph::DELETE,
                                                    "删除附件",
                                                    None,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.confirm_attachment = Some(delete.clone());
                                                    cx.notify();
                                                })),
                                            )
                                        },
                                    )
                                    .when(
                                        !busy
                                            && self.confirm_attachment.as_deref()
                                                == Some(file.id.as_str()),
                                        |el| {
                                            el.child(
                                                btn(("confirm-attachment", i), BtnVariant::Danger)
                                                    .h(px(26.))
                                                    .px(px(10.))
                                                    .text_size(px(font::CAPTION))
                                                    .child("确认删除")
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.delete_attachment_ui(&confirm, cx)
                                                        },
                                                    )),
                                            )
                                            .child(
                                                btn(("cancel-attachment", i), BtnVariant::Subtle)
                                                    .h(px(26.))
                                                    .px(px(8.))
                                                    .text_size(px(font::CAPTION))
                                                    .child("取消")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.confirm_attachment = None;
                                                        cx.notify();
                                                    })),
                                            )
                                        },
                                    ),
                            ),
                    );
            }
            if detail.can_upload_attachments && !busy && detail.attachment_lease().is_some() {
                card = card.child(
                    div().p(px(10.)).child(
                        btn("upload-attachment", BtnVariant::Subtle)
                            .h(px(30.))
                            .text_size(px(font::CAPTION))
                            .child(glyph_icon(glyph::UPLOAD, 13.))
                            .child("上传新附件")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.pick_attachment_ui(None, cx)),
                            ),
                    ),
                );
            }
            stack = stack.child(card);
        }

        // Collapsible Metadata & Password History footer.
        let history_count = detail.password_history.len();
        let meta_summary = if history_count > 0 {
            format!(
                "修改于 {} · 密码历史 ({history_count})",
                detail.revision_date
            )
        } else {
            format!("修改于 {}", detail.revision_date)
        };

        let meta_key = gpui::ElementId::Name("detail-meta-accordion".into());
        let meta_p =
            self.ui
                .collapsible_progress(&meta_key, self.ui.meta_expanded, now, self.animate);
        let mut meta_box = surface_card().overflow_hidden().child(
            div()
                .id("toggle-detail-meta")
                .cursor_pointer()
                .px(px(space::L))
                .py(px(10.))
                .flex()
                .items_center()
                .justify_between()
                .hover(move |s| s.bg(hex(pal.hover)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_color(hex(pal.text_muted))
                                .child(glyph_icon(glyph::HISTORY, 13.)),
                        )
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child(meta_summary),
                        ),
                )
                .child(rotating_chevron(meta_p, 13., hex(pal.text_muted).into()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ui.meta_expanded = !this.ui.meta_expanded;
                    this.ui.animate_collapsible(
                        gpui::ElementId::Name("detail-meta-accordion".into()),
                        this.ui.meta_expanded,
                        this.animate,
                    );
                    cx.notify();
                })),
        );

        if self.ui.meta_expanded || meta_p > 0.02 {
            meta_box = meta_box.child(
                div()
                    .relative()
                    .top(px(-6. * (1. - meta_p)))
                    .opacity(clamp_opacity(meta_p))
                    .px(px(space::L))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(hex(pal.border_subtle))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(!detail.creation_date.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_muted))
                                .child(format!("创建时间：{}", detail.creation_date)),
                        )
                    })
                    .when_some(detail.password_revision_date.clone(), |el, dt| {
                        el.child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_muted))
                                .child(format!("密码上次更新：{dt}")),
                        )
                    })
                    .when(!detail.password_history.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .pt(px(4.))
                                .child(
                                    div()
                                        .text_size(px(font::CAPTION))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(hex(pal.text_secondary))
                                        .child("密码历史"),
                                )
                                .children(detail.password_history.iter().enumerate().map(
                                    |(i, row)| {
                                        let tag = format!("hist-pass-{i}");
                                        let copied = self.ui.is_recently_copied(&tag, now);
                                        div()
                                            .py(px(4.))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(font::CAPTION))
                                                    .text_color(hex(pal.text_secondary))
                                                    .child(row.last_used_date.clone()),
                                            )
                                            .child(
                                                btn(
                                                    ("copy-historical", i),
                                                    if copied {
                                                        BtnVariant::Tinted
                                                    } else {
                                                        BtnVariant::Subtle
                                                    },
                                                )
                                                .h(px(26.))
                                                .px(px(8.))
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(
                                                    if copied { glyph::CHECK } else { glyph::COPY },
                                                    11.,
                                                ))
                                                .child(if copied { "已复制" } else { "复制" })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(row) = this
                                                        .client
                                                        .detail
                                                        .as_ref()
                                                        .and_then(|d| d.password_history.get(i))
                                                    {
                                                        this.copy_protected_labeled(
                                                            Zeroizing::new(row.password.clone()),
                                                            &tag,
                                                            "历史密码",
                                                            cx,
                                                        );
                                                    }
                                                })),
                                            )
                                    },
                                )),
                        )
                    }),
            );
        }

        stack.child(meta_box)
    }

    /// Inline Detail Editor pane with sticky top Save/Cancel header bar and smooth spring slide-in!
    pub fn render_inline_editor_pane(
        &self,
        _narrow: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Div {
        let pal = theme::palette();
        let (p, _) = self
            .ui
            .editor_spring
            .sample_staggered(now, Duration::ZERO, self.animate);
        let editor = self.client.editor.as_ref().expect("editor open");
        let is_new = editor.draft.id.is_none();
        let header_title = if is_new {
            "新增保险库条目".to_owned()
        } else {
            format!("编辑 · {}", editor.draft.name)
        };
        let must_resync = editor.must_resync;

        surface_card()
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if !this.client.ready() {
                    cx.stop_propagation();
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, _, _, cx| {
                if !this.client.ready() {
                    cx.stop_propagation();
                }
            }))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .relative()
            .top(px(14. * (1. - p)))
            .opacity(clamp_opacity(p))
            // Sticky top bar — Save & Cancel are always immediately reachable!
            .child(
                div()
                    .px(px(space::L))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(hex(pal.border_subtle))
                    .bg(hex(pal.card))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::M))
                    .flex_shrink_0()
                    .child(
                        btn("cancel-editor", BtnVariant::Subtle)
                            .h(px(30.))
                            .px(px(10.))
                            .text_size(px(font::CAPTION))
                            .child(glyph_icon(glyph::CLOSE, 11.))
                            .child("取消")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.client.ready() {
                                    return;
                                }
                                let restore_index = this
                                    .client
                                    .editor
                                    .as_ref()
                                    .and_then(|e| e.draft.id.as_ref())
                                    .and(this.selected)
                                    .filter(|&idx| {
                                        this.client
                                            .overview
                                            .as_ref()
                                            .and_then(|o| o.items.get(idx))
                                            .is_some_and(|it| !it.reprompt)
                                    });
                                this.ui.close_popovers(this.animate);
                                this.close_form(cx);
                                if let Some(idx) = restore_index {
                                    this.choose_item(idx, cx);
                                } else {
                                    this.close_detail();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_center()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(hex(pal.text))
                            .child(header_title),
                    )
                    .child(
                        btn(
                            "save-editor",
                            if must_resync {
                                BtnVariant::Secondary
                            } else {
                                BtnVariant::Primary
                            },
                        )
                        .h(px(30.))
                        .px(px(14.))
                        .text_size(px(font::CAPTION))
                        .when(must_resync, |el| el.opacity(0.55))
                        .child(glyph_icon(glyph::CHECK, 12.))
                        .child(if must_resync {
                            "请先同步核对"
                        } else {
                            "保存"
                        })
                        .tooltip(|_, cx| make_tooltip("保存修改", Some("Ctrl+S"), cx))
                        .on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))),
                    ),
            )
            .child(
                div()
                    .id("edit-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(space::XL))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(size::DETAIL_MAX))
                            .mx_auto()
                            .child(self.render_editor(now, cx)),
                    ),
            )
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}
