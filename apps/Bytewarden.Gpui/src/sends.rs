use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use bytewarden_core::desktop::{Request, SendDetail, SendDraft, SendSummary, Sends};
use chrono::{TimeDelta, Utc};
use gpui::{Context, Entity, div, prelude::*, px};

use crate::{
    Page, VaultPreview,
    client::status_text,
    detail_tools::{FileChooser, choose_file},
    editor::reveal_button,
    input::Input,
};

#[cfg(all(test, feature = "test-support"))]
#[path = "send_tests.rs"]
mod state_tests;

#[derive(Default)]
pub struct SendPage {
    list: Option<Sends>,
    detail: Option<SendDetail>,
    editor: Option<SendEditor>,
    confirm: Option<SendWrite>,
    revealed: bool,
    refresh_required: bool,
}

#[derive(Clone, Copy)]
enum SendWrite {
    Delete,
    RemoveAuth,
}

struct SendEditor {
    current: Option<SendSummary>,
    existing_file: bool,
    source: Option<PathBuf>,
    name: Entity<Input>,
    text: Entity<Input>,
    notes: Entity<Input>,
    password: Entity<Input>,
    emails: Entity<Input>,
    deletion: Entity<Input>,
    expiration: Entity<Input>,
    count: Entity<Input>,
    must_resync: bool,
    saving: bool,
    hidden: bool,
    hide_email: bool,
    disabled: bool,
}

impl SendEditor {
    fn new(detail: Option<&SendDetail>, source: Option<PathBuf>, cx: &mut gpui::App) -> Self {
        let new_field = |cx: &mut gpui::App, label, initial: &str, confidential| {
            cx.new(|cx| {
                let mut input = if matches!(label, "文本内容" | "备注") {
                    Input::multiline(label, confidential, cx)
                } else {
                    Input::new(label, confidential, cx)
                };
                input.set(initial, cx);
                input
            })
        };
        let existing_file = detail.is_some_and(|d| d.summary.kind == 1);
        let text = detail.and_then(|d| d.text.as_deref()).unwrap_or_default();
        let notes = detail.and_then(|d| d.notes.as_deref()).unwrap_or_default();
        let deletion = detail.map_or_else(
            || (Utc::now() + TimeDelta::days(7)).to_rfc3339(),
            |d| d.summary.deletion_date.clone(),
        );
        let expiration = detail
            .and_then(|d| d.summary.expiration_date.as_deref())
            .unwrap_or_default();
        let access_count = detail
            .and_then(|d| d.max_access_count)
            .map_or_else(String::new, |n| n.to_string());
        Self {
            current: detail.map(|d| d.summary.clone()),
            existing_file,
            source,
            name: new_field(
                cx,
                "名称",
                detail.map_or("", |d| d.summary.name.as_str()),
                false,
            ),
            text: new_field(cx, "文本内容", text, true),
            notes: new_field(cx, "备注", notes, true),
            password: new_field(cx, "新访问密码（可选）", "", true),
            emails: new_field(cx, "新接收邮箱（逗号分隔，可选）", "", false),
            deletion: new_field(cx, "删除时间（ISO 8601）", &deletion, false),
            expiration: new_field(cx, "访问到期（ISO 8601，可选）", expiration, false),
            count: new_field(cx, "最大访问次数（可选）", &access_count, false),
            must_resync: false,
            saving: false,
            hidden: detail.is_some_and(|d| d.hidden),
            hide_email: detail.is_some_and(|d| d.hide_email),
            disabled: detail.is_some_and(|d| d.summary.disabled),
        }
    }
    fn file(&self) -> bool {
        self.existing_file || self.source.is_some()
    }
    fn inputs(&self) -> [&Entity<Input>; 8] {
        [
            &self.name,
            &self.text,
            &self.notes,
            &self.password,
            &self.emails,
            &self.deletion,
            &self.expiration,
            &self.count,
        ]
    }

    fn committed(&self, cx: &gpui::App) -> bool {
        self.inputs().iter().all(|field| field.read(cx).committed())
    }

    fn set_read_only(&self, read_only: bool, cx: &mut gpui::App) {
        for field in self.inputs() {
            field.update(cx, |input, cx| input.set_read_only(read_only, cx));
        }
    }

    fn clear(&self, cx: &mut gpui::App) {
        for field in self.inputs() {
            field.update(cx, |input, cx| input.set("", cx));
        }
    }

    fn draft(&self, cx: &gpui::App) -> Option<SendDraft> {
        if self.must_resync || !self.committed(cx) {
            return None;
        }
        let read = |field: &Entity<Input>| field.read(cx).value();
        let optional = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        let deletion = read(&self.deletion).trim();
        let expiration = read(&self.expiration).trim();
        chrono::DateTime::parse_from_rfc3339(deletion).ok()?;
        if !expiration.is_empty() {
            chrono::DateTime::parse_from_rfc3339(expiration).ok()?;
        }
        let count = read(&self.count).trim();
        let max_access_count = if count.is_empty() {
            None
        } else {
            Some(count.parse().ok()?)
        };
        // Validate first; only create owned confidential values in the zeroizing DTO.
        let emails = read(&self.emails);
        Some(SendDraft {
            name: read(&self.name).into(),
            text: if self.file() {
                None
            } else {
                optional(read(&self.text))
            },
            notes: optional(read(&self.notes)),
            hidden: self.hidden && !self.file(),
            disabled: self.disabled,
            hide_email: self.hide_email,
            max_access_count,
            deletion_date: deletion.into(),
            expiration_date: optional(expiration),
            password: optional(read(&self.password)),
            emails: (!emails.trim().is_empty()).then(|| {
                emails
                    .split([',', ';'])
                    .map(|address| address.trim().to_owned())
                    .collect()
            }),
        })
    }
}

impl SendPage {
    pub(crate) fn editing(&self) -> bool {
        self.editor.is_some()
    }

    fn close_editor(&mut self, cx: &mut gpui::App) {
        if let Some(editor) = self.editor.take() {
            editor.clear(cx);
        }
    }

    fn mark_refresh_required(&mut self) {
        self.refresh_required = true;
        self.list = None;
        self.detail = None;
        self.confirm = None;
        self.revealed = false;
    }

    fn begin_refresh(&mut self, cx: &mut gpui::App) {
        let required = self.refresh_required;
        self.clear(cx);
        self.refresh_required = required;
    }

    fn apply_refresh(&mut self, list: Sends) {
        self.list = Some(list);
        self.refresh_required = false;
    }

    pub(crate) fn clear(&mut self, cx: &mut gpui::App) {
        self.close_editor(cx);
        *self = Self::default();
    }
}

impl VaultPreview {
    pub(crate) fn load_sends(&mut self, cx: &mut Context<Self>) {
        if !self.client.sync_available() {
            return;
        }
        let Some(account) = self.client.active().map(|a| a.id.clone()) else {
            return;
        };
        self.sends.begin_refresh(cx);
        let request = Request::begin();
        self.run(
            "正在读取 Send",
            Some(request),
            cx,
            move || request.sends(&account),
            |this, result, _| match result {
                Ok(list) if !this.lock_queued => this.sends.apply_refresh(list),
                Ok(_) => {}
                Err(e) => {
                    this.client.notice = if this.sends.refresh_required {
                        "刷新 Send 失败。列表仍不可写，请重试刷新并核对结果。".into()
                    } else {
                        status_text(&e)
                    };
                }
            },
        );
    }

    fn select_send(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.client.sync_available() || self.sends.editor.is_some() {
            return;
        }
        let Some(account) = self.client.active().map(|a| a.id.clone()) else {
            return;
        };
        self.sends.detail = None;
        self.sends.revealed = false;
        self.sends.confirm = None;
        let request = Request::begin();
        self.run(
            "正在读取 Send 详情",
            Some(request),
            cx,
            move || request.send_detail(&account, &id),
            |this, result, _| match result {
                Ok(item) if !this.lock_queued && this.page == Page::Sends => {
                    this.sends.detail = Some(item);
                    this.ui.trigger_send_detail_motion(this.animate);
                }
                Ok(_) => {}
                Err(e) => this.client.notice = status_text(&e),
            },
        );
    }

    fn start_send_editor(&mut self, existing: bool, file: bool, cx: &mut Context<Self>) {
        if !self.client.sync_available() || self.sends.refresh_required {
            if self.sends.refresh_required {
                self.client.notice = "请先刷新 Send 列表并核对写入结果。".into();
                cx.notify();
            }
            return;
        }
        if existing {
            let Some(detail) = self
                .sends
                .detail
                .as_ref()
                .filter(|d| matches!(d.summary.kind, 0 | 1))
            else {
                return;
            };
            self.sends.editor = Some(SendEditor::new(Some(detail), None, cx));
            self.ui.trigger_send_editor_motion(self.animate);
        } else if file {
            choose_file(
                cx,
                self.client.generation,
                FileChooser::Open,
                None,
                |this, path, cx| {
                    if this.page == Page::Sends {
                        this.sends.detail = None;
                        this.sends.editor = Some(SendEditor::new(None, Some(path), cx));
                        this.ui.trigger_send_editor_motion(this.animate);
                        cx.notify();
                    }
                },
            );
        } else {
            self.sends.detail = None;
            self.sends.editor = Some(SendEditor::new(None, None, cx));
            self.ui.trigger_send_editor_motion(self.animate);
        }
        self.sends.revealed = false;
        self.sends.confirm = None;
        cx.notify();
    }

    fn save_send(&mut self, cx: &mut Context<Self>) {
        if !self.client.sync_available() || self.sends.refresh_required {
            return;
        }
        let (Some(account), Some(editor)) = (
            self.client.active().map(|a| a.id.clone()),
            self.sends.editor.as_ref(),
        ) else {
            return;
        };
        if editor.must_resync {
            self.client.notice =
                "保存结果需要核对。请保留草稿，取消后刷新 Send，核对后再编辑。".into();
            cx.notify();
            return;
        }
        if !editor.committed(cx) {
            self.client.notice = "请先完成输入法文字输入。".into();
            cx.notify();
            return;
        }
        let Some(draft) = editor.draft(cx) else {
            self.client.notice = "请检查 ISO 8601 日期和最大访问次数。".into();
            cx.notify();
            return;
        };
        let summary = editor.current.clone();
        let source = editor.source.clone();
        if let Some(editor) = &mut self.sends.editor {
            editor.saving = true;
            editor.set_read_only(true, cx);
        }
        let request = Request::begin();
        self.run(
            "正在保存 Send",
            Some(request),
            cx,
            move || request.save_send(&account, summary.as_ref(), &draft, source.as_deref()),
            |this, result, cx| match result {
                Ok(()) if !this.lock_queued => {
                    this.sends.close_editor(cx);
                    this.client.notice = "Send 已保存。".into();
                    this.load_sends(cx);
                }
                Ok(()) => {}
                Err(e) => {
                    if matches!(e.code(), "conflict" | "notSynced" | "writeUncertain") {
                        this.sends.mark_refresh_required();
                        if let Some(editor) = &mut this.sends.editor {
                            editor.must_resync = true;
                            editor.saving = false;
                        }
                        this.client
                            .notice
                            .push_str(" 请取消编辑后刷新 Send 列表并核对结果。");
                    } else {
                        this.client.notice = status_text(&e);
                        if let Some(editor) = &mut this.sends.editor {
                            editor.saving = false;
                            editor.set_read_only(false, cx);
                        }
                    }
                }
            },
        );
    }

    fn copy_send_link(&mut self, cx: &mut Context<Self>) {
        if !self.client.sync_available() || self.sends.refresh_required {
            return;
        }
        let (Some(account), Some(id)) = (
            self.client.active().map(|a| a.id.clone()),
            self.sends.detail.as_ref().map(|d| d.summary.id.clone()),
        ) else {
            return;
        };
        let request = Request::begin();
        self.run(
            "正在获取分享链接",
            Some(request),
            cx,
            move || request.send_link(&account, &id),
            |this, result, cx| match result {
                Ok(value) if !this.lock_queued && this.page == Page::Sends => {
                    this.copy_protected(value, cx)
                }
                Ok(_) => {}
                Err(e) => this.client.notice = status_text(&e),
            },
        );
    }

    fn write_send(&mut self, cx: &mut Context<Self>) {
        if !self.client.sync_available() || self.sends.refresh_required {
            if self.sends.refresh_required {
                self.client.notice = "请先刷新 Send 列表并核对写入结果。".into();
                cx.notify();
            }
            return;
        }
        let (Some(account), Some(item), Some(op)) = (
            self.client.active().map(|a| a.id.clone()),
            self.sends.detail.as_ref().map(|d| d.summary.clone()),
            self.sends.confirm.take(),
        ) else {
            return;
        };
        let request = Request::begin();
        self.run(
            "正在更新 Send",
            Some(request),
            cx,
            move || match op {
                SendWrite::Delete => request.delete_send(&account, &item),
                SendWrite::RemoveAuth => request.remove_send_auth(&account, &item),
            },
            |this, result, cx| match result {
                Ok(()) if !this.lock_queued => {
                    this.client.notice = "Send 已更新。".into();
                    this.load_sends(cx);
                }
                Ok(()) => {}
                Err(e) if matches!(e.code(), "conflict" | "notSynced" | "writeUncertain") => {
                    this.sends.mark_refresh_required();
                    this.client.notice = format!(
                        "{} Send 列表已禁写；请刷新并核对操作结果。",
                        status_text(&e)
                    );
                }
                Err(e) => this.client.notice = status_text(&e),
            },
        );
    }

    pub(crate) fn render_sends(
        &self,
        narrow: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{
                BtnVariant, badge, btn, card_group, empty_state, glyph_icon, info_bar,
                surface_card, toggle_switch_animated,
            },
            spring::clamp_opacity,
            toast::ToastKind,
            tokens::{font, glyph, radius, space},
        };

        let pal = theme::palette();
        let (detail_p, _) =
            self.ui
                .send_detail_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let (editor_p, _) =
            self.ui
                .send_editor_spring
                .sample_staggered(now, Duration::ZERO, self.animate);

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(space::M))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(font::SUBTITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(hex(pal.text))
                            .child("Send 加密分享"),
                    )
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child("端到端加密分享限时文本或文件。"),
                    ),
            )
            .when(
                self.client.sync_available() && self.sends.editor.is_none(),
                |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                btn("send-refresh", BtnVariant::Secondary)
                                    .h(px(30.))
                                    .text_size(px(font::CAPTION))
                                    .child(glyph_icon(glyph::SYNC, 12.))
                                    .child("刷新")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_sends(cx))),
                            )
                            .when(!self.sends.refresh_required, |tools| {
                                tools
                                    .child(
                                        btn("send-text", BtnVariant::Primary)
                                            .h(px(30.))
                                            .text_size(px(font::CAPTION))
                                            .child(glyph_icon(glyph::ADD, 12.))
                                            .child("文本 Send")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.start_send_editor(false, false, cx)
                                            })),
                                    )
                                    .child(
                                        btn("send-file", BtnVariant::Tinted)
                                            .h(px(30.))
                                            .text_size(px(font::CAPTION))
                                            .child(glyph_icon(glyph::UPLOAD, 12.))
                                            .child("文件 Send")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.start_send_editor(false, true, cx)
                                            })),
                                    )
                            }),
                    )
                },
            );

        let mut page = div()
            .id("send-page")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(space::M))
            .p(px(space::S))
            .child(header);

        if self.sends.refresh_required && self.sends.editor.is_none() {
            page = page.child(info_bar(
                ToastKind::Warning,
                "Send 写入结果需要核对。列表已禁写；请刷新列表并确认实际状态后再操作。",
            ));
        }

        if !self.client.sync_available()
            && self.client.busy.is_none()
            && self.sends.editor.is_none()
        {
            return page.child(
                surface_card()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(empty_state(
                        glyph::SEND,
                        "登录在线保险库后使用 Send",
                        "离线快照不允许创建或管理 Send 分享。",
                    )),
            );
        }

        // Responsive Master-Detail layout for Sends (list stays visible while editing on wide screens).
        let left_list = if let Some(list) = &self.sends.list {
            surface_card()
                .flex()
                .flex_col()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .when(narrow, |el| el.flex_1())
                .when(!narrow, |el| el.w(px(320.)).flex_shrink_0())
                .child(
                    div()
                        .px(px(space::M))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(hex(pal.border_subtle))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(hex(pal.text_secondary))
                                .child("已创建的 Send"),
                        )
                        .child(
                            div()
                                .text_size(px(font::MICRO))
                                .text_color(hex(pal.text_muted))
                                .child(format!(
                                    "{} 项{}",
                                    list.items.len(),
                                    if list.failures > 0 {
                                        format!(" · {} 无法读取", list.failures)
                                    } else {
                                        String::new()
                                    }
                                )),
                        ),
                )
                .child(
                    div()
                        .id("send-list-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .when(list.items.is_empty(), |el| {
                            el.child(
                                div()
                                    .py(px(28.))
                                    .text_center()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_muted))
                                    .child("暂无 Send，点击上方按钮新建"),
                            )
                        })
                        .children(list.items.iter().enumerate().map(|(i, item)| {
                            let id = item.id.clone();
                            let active = self
                                .sends
                                .detail
                                .as_ref()
                                .is_some_and(|d| d.summary.id == item.id);
                            div()
                                .id(("send-item", i))
                                .cursor_pointer()
                                .px(px(10.))
                                .py(px(9.))
                                .rounded(px(radius::CONTROL))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .when(active, |el| {
                                    el.bg(pal.tint(pal.accent_fill))
                                        .border_1()
                                        .border_color(pal.tint_border(pal.accent_fill))
                                })
                                .when(!active, |el| el.hover(move |s| s.bg(hex(pal.hover))))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .truncate()
                                        .text_size(px(font::BODY))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(hex(pal.text))
                                        .child(item.name.clone()),
                                )
                                .child(if item.disabled {
                                    badge("已停用", pal.warning)
                                } else if item.kind == 1 {
                                    badge("文件", pal.accent_text)
                                } else {
                                    badge("文本", pal.info)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_send(id.clone(), cx)
                                }))
                        })),
                )
        } else {
            surface_card()
                .flex()
                .flex_col()
                .min_w_0()
                .min_h_0()
                .when(narrow, |el| el.flex_1())
                .when(!narrow, |el| el.w(px(320.)).flex_shrink_0())
                .child(
                    div()
                        .p(px(space::L))
                        .text_size(px(font::CAPTION))
                        .text_color(hex(pal.text_muted))
                        .child("点击「刷新」加载 Send 列表。"),
                )
        };

        let right_pane = if let Some(editor) = &self.sends.editor {
            surface_card()
                .id("send-editor-scroll")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(space::L))
                .child(
                    div()
                        .relative()
                        .top(px(12. * (1. - editor_p)))
                        .opacity(clamp_opacity(editor_p))
                        .w_full()
                        .max_w(px(680.))
                        .mx_auto()
                        .child(
                            card_group(
                                if editor.current.is_some() {
                                    "编辑 Send"
                                } else {
                                    "新建 Send"
                                },
                                Some(glyph::SEND),
                            )
                            .child(
                                div()
                                    .p(px(space::L))
                                    .flex()
                                    .flex_col()
                                    .gap(px(space::M))
                                    .child(editor.name.clone())
                                    .when(!editor.file(), |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(5.))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .justify_between()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .text_size(px(font::CAPTION))
                                                                .text_color(hex(pal.text_secondary))
                                                                .child("文本内容"),
                                                        )
                                                        .child(reveal_button(&editor.text, cx)),
                                                )
                                                .child(editor.text.clone()),
                                        )
                                    })
                                    .when_some(
                                        editor.source.as_ref().and_then(|p| p.file_name()),
                                        |el, name| {
                                            el.child(badge(
                                                name.to_string_lossy().to_string(),
                                                pal.info,
                                            ))
                                        },
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(5.))
                                            .child(
                                                div()
                                                    .flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(font::CAPTION))
                                                            .text_color(hex(pal.text_secondary))
                                                            .child("备注"),
                                                    )
                                                    .child(reveal_button(&editor.notes, cx)),
                                            )
                                            .child(editor.notes.clone()),
                                    )
                                    .when(editor.saving, |el| {
                                        el.child(info_bar(
                                            ToastKind::Info,
                                            "正在保存。表单已暂时锁定，请等待保存结果。",
                                        ))
                                    })
                                    .when(editor.must_resync, |el| {
                                        el.child(info_bar(
                                            ToastKind::Warning,
                                            "草稿已保留。请取消后刷新 Send，核对保存结果。",
                                        ))
                                    })
                                    .child(editor.deletion.clone())
                                    .child(editor.expiration.clone())
                                    .child(editor.count.clone())
                                    .child(editor.password.clone())
                                    .child(editor.emails.clone())
                                    .child(
                                        div()
                                            .text_size(px(font::CAPTION))
                                            .text_color(hex(pal.text_muted))
                                            .child("不给新密码或邮箱时，保留已有访问验证。"),
                                    )
                                    .child(
                                        div().flex().flex_col().gap(px(6.)).children(
                                            [
                                                (editor.hidden, "默认隐藏文本"),
                                                (editor.hide_email, "隐藏发送邮箱"),
                                                (editor.disabled, "停用 Send"),
                                            ]
                                            .into_iter()
                                            .enumerate()
                                            .filter(|(i, _)| *i != 0 || !editor.file())
                                            .map(
                                                |(i, (on, label))| {
                                                    let prog = self.ui.toggle_progress(
                                                        &("send-option", i).into(),
                                                        on,
                                                        now,
                                                        self.animate,
                                                    );
                                                    div()
                                                        .py(px(4.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .when(editor.saving, |el| el.opacity(0.55))
                                                        .child(label)
                                                        .child(
                                                            toggle_switch_animated(
                                                                ("send-option", i),
                                                                on,
                                                                prog,
                                                            )
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    if this.client.ready()
                                                                        && let Some(editor) =
                                                                            &mut this.sends.editor
                                                                    {
                                                                        let value = match i {
                                                                            0 => &mut editor.hidden,
                                                                            1 => {
                                                                                &mut editor
                                                                                    .hide_email
                                                                            }
                                                                            _ => {
                                                                                &mut editor.disabled
                                                                            }
                                                                        };
                                                                        *value = !*value;
                                                                        let next = *value;
                                                                        this.ui.animate_toggle(
                                                                            ("send-option", i),
                                                                            next,
                                                                            this.animate,
                                                                        );
                                                                        cx.notify();
                                                                    }
                                                                },
                                                            )),
                                                        )
                                                },
                                            ),
                                        ),
                                    )
                                    .when(self.client.ready(), |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .gap(px(8.))
                                                .pt(px(4.))
                                                .child(
                                                    btn("send-save", BtnVariant::Primary)
                                                        .opacity(if editor.must_resync {
                                                            0.45
                                                        } else {
                                                            1.
                                                        })
                                                        .child(if editor.must_resync {
                                                            "请先刷新核对"
                                                        } else {
                                                            "保存 Send"
                                                        })
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.save_send(cx)
                                                        })),
                                                )
                                                .child(
                                                    btn("send-edit-cancel", BtnVariant::Secondary)
                                                        .child("取消")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.sends.close_editor(cx);
                                                            this.ui.trigger_send_detail_motion(
                                                                this.animate,
                                                            );
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                    }),
                            ),
                        ),
                )
        } else if let Some(detail) = &self.sends.detail {
            let body = detail.text.as_ref().map(|text| {
                if detail.hidden && !self.sends.revealed {
                    "••••••••••••".into()
                } else {
                    text.clone()
                }
            });
            surface_card()
                .id("send-detail-scroll")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .when(narrow, |el| {
                    el.child(
                        div()
                            .px(px(space::L))
                            .pt(px(space::M))
                            .pb(px(space::S))
                            .border_b_1()
                            .border_color(hex(pal.border_subtle))
                            .child(
                                btn("send-back", BtnVariant::Subtle)
                                    .h(px(30.))
                                    .px(px(8.))
                                    .child(glyph_icon(glyph::BACK, 13.))
                                    .child("返回 Send 列表")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sends.detail = None;
                                        this.sends.confirm = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                })
                .child(
                    div()
                        .relative()
                        .top(px(12. * (1. - detail_p)))
                        .opacity(clamp_opacity(detail_p))
                        .p(px(space::XL))
                        .w_full()
                        .max_w(px(680.))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(space::M))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .text_size(px(font::SUBTITLE))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(hex(pal.text))
                                        .child(detail.summary.name.clone()),
                                )
                                .child(badge(format!("{} 次访问", detail.access_count), pal.info)),
                        )
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_muted))
                                .child(format!("删除时间：{}", detail.summary.deletion_date)),
                        )
                        .when_some(body, |el, text| {
                            el.child(
                                div()
                                    .p(px(space::M))
                                    .rounded(px(radius::CONTROL))
                                    .bg(hex(pal.layer))
                                    .border_1()
                                    .border_color(hex(pal.border_subtle))
                                    .text_size(px(font::BODY))
                                    .text_color(hex(pal.text))
                                    .child(text),
                            )
                        })
                        .when_some(detail.file_name.clone(), |el, name| {
                            el.child(badge(format!("附件：{name}"), pal.accent_text))
                        })
                        .when(self.client.sync_available(), |el| {
                            el.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(8.))
                                    .child(
                                        btn("send-copy-link", BtnVariant::Primary)
                                            .child(glyph_icon(glyph::COPY, 13.))
                                            .child("复制分享链接")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.copy_send_link(cx)
                                            })),
                                    )
                                    .when(detail.hidden && detail.text.is_some(), |el| {
                                        el.child(
                                            btn("send-reveal", BtnVariant::Secondary)
                                                .child(glyph_icon(
                                                    if self.sends.revealed {
                                                        glyph::EYE_OFF
                                                    } else {
                                                        glyph::EYE
                                                    },
                                                    13.,
                                                ))
                                                .child(if self.sends.revealed {
                                                    "隐藏"
                                                } else {
                                                    "显示"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.sends.revealed = !this.sends.revealed;
                                                    cx.notify();
                                                })),
                                        )
                                    })
                                    .when(matches!(detail.summary.kind, 0 | 1), |el| {
                                        el.child(
                                            btn("send-edit", BtnVariant::Secondary)
                                                .child(glyph_icon(glyph::EDIT, 13.))
                                                .child("编辑")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.start_send_editor(true, false, cx)
                                                })),
                                        )
                                    })
                                    .when(detail.auth_type != 2 || detail.has_password, |el| {
                                        el.child(
                                            btn("send-remove-auth", BtnVariant::Secondary)
                                                .child("移除访问验证")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.sends.confirm =
                                                        Some(SendWrite::RemoveAuth);
                                                    cx.notify();
                                                })),
                                        )
                                    })
                                    .child(
                                        btn("send-delete", BtnVariant::Danger)
                                            .child(glyph_icon(glyph::DELETE, 13.))
                                            .child("删除")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sends.confirm = Some(SendWrite::Delete);
                                                cx.notify();
                                            })),
                                    ),
                            )
                        })
                        .when_some(self.sends.confirm, |el, op| {
                            el.child(
                                info_bar(
                                    ToastKind::Warning,
                                    match op {
                                        SendWrite::Delete => "确认永久删除此 Send？",
                                        SendWrite::RemoveAuth => "确认移除此 Send 的访问验证？",
                                    },
                                )
                                .child(
                                    btn("send-confirm", BtnVariant::Danger)
                                        .h(px(26.))
                                        .px(px(10.))
                                        .text_size(px(font::CAPTION))
                                        .child("确认")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.write_send(cx)),
                                        ),
                                )
                                .child(
                                    btn("send-cancel", BtnVariant::Subtle)
                                        .h(px(26.))
                                        .px(px(8.))
                                        .text_size(px(font::CAPTION))
                                        .child("取消")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sends.confirm = None;
                                            cx.notify();
                                        })),
                                ),
                            )
                        }),
                )
        } else {
            surface_card()
                .id("send-empty")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_state(
                    glyph::SEND,
                    "选择一个 Send 查看详情",
                    "或点击右上角创建新的文本 / 文件 Send。",
                ))
        };

        let show_right = !narrow || self.sends.detail.is_some() || self.sends.editor.is_some();
        let show_list = !narrow || !show_right;

        page.child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .gap(px(space::M))
                .when(show_list, |el| el.child(left_list))
                .when(show_right, |el| el.child(right_pane)),
        )
    }
}
