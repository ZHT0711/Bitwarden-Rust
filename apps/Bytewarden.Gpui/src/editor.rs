use bytewarden_core::desktop::{Draft, DraftField, DraftUri, Request};
use gpui::{Context, Entity, Window, div, prelude::*, px};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    VaultPreview,
    client::{Form, status_text},
    input::Input,
    vault_list::{FILTERS, Filter},
};

#[cfg(all(test, feature = "test-support"))]
#[path = "editor_tests.rs"]
mod state_tests;
#[cfg(all(test, feature = "test-support"))]
#[path = "editor_workflow_tests.rs"]
mod workflow_tests;

pub struct Editor {
    pub(crate) draft: Draft,
    password: Zeroizing<String>,
    name: Entity<Input>,
    notes: Editable,
    fields: Vec<(&'static str, Editable)>,
    uris: Vec<UriEditor>,
    custom_fields: Vec<CustomEditor>,
    type_fields: std::collections::BTreeMap<u8, Vec<(&'static str, Editable)>>,
    folder_id: Option<String>,
    show_folders: bool,
    pub(crate) must_resync: bool,
}

struct EditorLoad {
    form_generation: u64,
    item_id: String,
    clone_requested: bool,
}

impl EditorLoad {
    fn is_current(
        &self,
        form_generation: u64,
        selected_item_id: Option<&str>,
        clone_pending: bool,
        lock_queued: bool,
    ) -> bool {
        !lock_queued
            && self.form_generation == form_generation
            && selected_item_id == Some(self.item_id.as_str())
            && self.clone_requested == clone_pending
    }
}

struct Editable {
    label: &'static str,
    input: Entity<Input>,
    secret: bool,
}

#[derive(Clone, Copy, Default)]
struct EditorGenProps {
    open: bool,
    progress: f32,
    is_phrase: bool,
    length: u32,
    words: u32,
    has_special: bool,
}

impl Editable {
    fn new(label: &'static str, secret: bool, value: Option<&str>, cx: &mut gpui::App) -> Self {
        let value = value.unwrap_or_default();
        let multiline =
            matches!(label, "备注" | "SSH 私钥" | "SSH 公钥") || value.contains(['\n', '\r']);
        let input = cx.new(|cx| {
            let mut field = if multiline {
                Input::multiline(label, secret, cx)
            } else {
                Input::new(label, secret, cx)
            };
            field.set(value, cx);
            field
        });
        Self {
            label,
            input,
            secret,
        }
    }
    fn render(&self, gen_props: EditorGenProps, cx: &mut Context<VaultPreview>) -> gpui::Div {
        let EditorGenProps {
            open: gen_open,
            progress: gen_p,
            is_phrase,
            length: gen_length,
            words: gen_words,
            has_special,
        } = gen_props;
        let pal = crate::theme::palette();
        let is_password = self.label == "密码";
        let input_for_gen = self.input.clone();
        let input_for_opts = self.input.clone();
        let apply_generated =
            |this: &mut VaultPreview, target: &Entity<Input>, cx: &mut Context<VaultPreview>| {
                match this.generator.generate_for_editor() {
                    Ok(generated) => {
                        target.update(cx, |field, cx| {
                            field.set(&generated, cx);
                            field.set_revealed(true, cx);
                        });
                        this.ui.toast.show(
                            crate::ui::toast::ToastKind::Success,
                            this.generator.editor_generation_message(),
                            std::time::Instant::now(),
                            this.animate,
                        );
                        cx.notify();
                    }
                    Err(message) => {
                        this.client.notice = message.into();
                        cx.notify();
                    }
                }
            };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(crate::ui::tokens::font::CAPTION))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(crate::theme::hex(pal.text_secondary))
                            .child(self.label),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .when(is_password, |el| {
                                el.child(
                                    crate::ui::components::btn(
                                        (
                                            "editor-quick-gen",
                                            input_for_gen.entity_id().as_u64() as usize,
                                        ),
                                        crate::ui::components::BtnVariant::Tinted,
                                    )
                                    .h(px(24.))
                                    .px(px(8.))
                                    .text_size(px(crate::ui::tokens::font::MICRO))
                                    .child(crate::ui::components::glyph_icon(
                                        crate::ui::tokens::glyph::DICE,
                                        11.,
                                    ))
                                    .child("随机生成")
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            apply_generated(this, &input_for_gen, cx);
                                        },
                                    )),
                                )
                                .child(
                                    crate::ui::components::btn(
                                        "editor-gen-opts-toggle",
                                        crate::ui::components::BtnVariant::Subtle,
                                    )
                                    .h(px(24.))
                                    .w(px(24.))
                                    .px(px(0.))
                                    .child(crate::ui::components::rotating_chevron(
                                        gen_p,
                                        10.,
                                        crate::theme::hex(pal.text_secondary).into(),
                                    ))
                                    .tooltip(|_, cx| {
                                        crate::ui::components::make_tooltip(
                                            "快捷生成选项",
                                            None,
                                            cx,
                                        )
                                    })
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            let next = !this.ui.editor_gen_open;
                                            this.ui.editor_gen_open = next;
                                            this.ui.editor_gen_spring.retarget(
                                                if next { 1. } else { 0. },
                                                std::time::Instant::now(),
                                                this.animate,
                                            );
                                            cx.notify();
                                        },
                                    )),
                                )
                            })
                            .when(self.secret, |el| el.child(reveal_button(&self.input, cx))),
                    ),
            )
            .when(is_password && (gen_open || gen_p > 0.02), |el| {
                let input_pwd = input_for_opts.clone();
                let input_phr = input_for_opts.clone();
                let input_spec = input_for_opts.clone();
                el.child(
                    div()
                        .relative()
                        .top(px(-5. * (1. - gen_p)))
                        .opacity(crate::ui::spring::clamp_opacity(gen_p))
                        .p(px(8.))
                        .rounded(px(crate::ui::tokens::radius::CONTROL))
                        .bg(crate::theme::hex(pal.layer))
                        .border_1()
                        .border_color(crate::theme::hex(pal.border_subtle))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(5.))
                        .child(
                            crate::ui::components::pill_chip(
                                "editor-gen-mode-pwd",
                                "随机密码",
                                None,
                                !is_phrase,
                            )
                            .h(px(22.))
                            .px(px(8.))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.generator.set_editor_phrase(false);
                                    apply_generated(this, &input_pwd, cx);
                                },
                            )),
                        )
                        .child(
                            crate::ui::components::pill_chip(
                                "editor-gen-mode-phrase",
                                "密码短语",
                                None,
                                is_phrase,
                            )
                            .h(px(22.))
                            .px(px(8.))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.generator.set_editor_phrase(true);
                                    apply_generated(this, &input_phr, cx);
                                },
                            )),
                        )
                        .when(!is_phrase, |row| {
                            row.children([14u32, 16, 20, 24].into_iter().map(|preset| {
                                let input_len = input_for_opts.clone();
                                crate::ui::components::pill_chip(
                                    ("editor-gen-len", preset as usize),
                                    format!("{preset}位"),
                                    None,
                                    gen_length == preset,
                                )
                                .h(px(22.))
                                .px(px(7.))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.generator.set_editor_length(preset);
                                        apply_generated(this, &input_len, cx);
                                    },
                                ))
                            }))
                            .child(
                                crate::ui::components::pill_chip(
                                    "editor-gen-special",
                                    "含符号",
                                    None,
                                    has_special,
                                )
                                .h(px(22.))
                                .px(px(7.))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.generator.toggle_editor_special();
                                        apply_generated(this, &input_spec, cx);
                                    },
                                )),
                            )
                        })
                        .when(is_phrase, |row| {
                            row.children([4u32, 6, 8].into_iter().map(|preset| {
                                let input_words = input_for_opts.clone();
                                crate::ui::components::pill_chip(
                                    ("editor-gen-words", preset as usize),
                                    format!("{preset}词"),
                                    None,
                                    gen_words == preset,
                                )
                                .h(px(22.))
                                .px(px(7.))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.generator.set_editor_words(preset);
                                        apply_generated(this, &input_words, cx);
                                    },
                                ))
                            }))
                        }),
                )
            })
            .child(self.input.clone())
    }
    fn committed(&self, cx: &gpui::App) -> bool {
        self.input.read(cx).committed()
    }
}

fn clone_draft(mut draft: Draft) -> Draft {
    draft.id = None;
    draft.revision_date = None;
    draft.name = if draft.name.trim().is_empty() {
        "副本".into()
    } else {
        format!("{} - 副本", draft.name)
    };
    draft.passkeys = None;
    draft.remove_saved_passkey = false;
    draft.can_edit = true;
    draft.is_deleted = false;
    draft.edit_blocked_reason = None;
    draft
}

pub(crate) fn reveal_button(
    input: &Entity<Input>,
    cx: &mut Context<VaultPreview>,
) -> gpui::Stateful<gpui::Div> {
    let input = input.clone();
    let revealed = input.read(cx).revealed();
    crate::ui::components::btn(
        ("reveal-editor", input.entity_id().as_u64() as usize),
        if revealed {
            crate::ui::components::BtnVariant::Tinted
        } else {
            crate::ui::components::BtnVariant::Subtle
        },
    )
    .h(px(24.))
    .px(px(8.))
    .text_size(px(crate::ui::tokens::font::MICRO))
    .child(crate::ui::components::glyph_icon(
        if revealed {
            crate::ui::tokens::glyph::EYE_OFF
        } else {
            crate::ui::tokens::glyph::EYE
        },
        11.,
    ))
    .child(if revealed { "隐藏" } else { "显示" })
    .on_click(cx.listener(move |_, _, _, cx| {
        input.update(cx, |input, cx| input.set_revealed(!input.revealed(), cx));
        cx.notify();
    }))
}

const URI_MATCHES: &[(Option<u8>, &str)] = &[
    (None, "默认"),
    (Some(0), "域名"),
    (Some(1), "主机"),
    (Some(2), "开头匹配"),
    (Some(3), "完全匹配"),
    (Some(4), "正则表达式"),
    (Some(5), "从不"),
];

struct UriEditor {
    original_missing: bool,
    input: Entity<Input>,
    match_mode: Option<u8>,
}

impl UriEditor {
    fn new(uri: Option<&DraftUri>, cx: &mut gpui::App) -> Self {
        let original = uri.and_then(|uri| uri.uri.as_deref());
        let match_mode = uri.and_then(|uri| uri.r#match);
        let input = cx.new(|cx| {
            let mut field = Input::new("https://example.com", false, cx);
            if let Some(value) = original {
                field.set(value, cx);
            }
            field
        });
        Self {
            original_missing: original.is_none(),
            input,
            match_mode,
        }
    }

    fn collect(&self, cx: &gpui::App) -> DraftUri {
        let value = self.input.read(cx).value();
        DraftUri {
            uri: uri_value(self.original_missing, value),
            r#match: self.match_mode,
        }
    }
}

fn uri_value(original_missing: bool, value: &str) -> Option<String> {
    if original_missing && value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn remove_uri<T>(uris: &mut Vec<T>, index: usize) {
    if index < uris.len() {
        uris.remove(index);
    }
}

const CUSTOM_TYPES: &[(u8, &str)] = &[(0, "文本"), (1, "隐藏"), (2, "布尔值"), (3, "关联")];

fn linked_options(item_type: u8) -> &'static [(u32, &'static str)] {
    match item_type {
        1 => &[(100, "用户名"), (101, "密码")],
        3 => &[
            (300, "持卡人"),
            (301, "到期月份"),
            (302, "到期年份"),
            (303, "安全码"),
            (304, "卡组织"),
            (305, "卡号"),
        ],
        4 => &[
            (400, "称谓"),
            (401, "中间名"),
            (402, "地址 1"),
            (403, "地址 2"),
            (404, "地址 3"),
            (405, "城市"),
            (406, "省／州"),
            (407, "邮编"),
            (408, "国家"),
            (409, "公司"),
            (410, "邮箱"),
            (411, "电话"),
            (412, "社会保障号码"),
            (413, "用户名"),
            (414, "护照号码"),
            (415, "驾照号码"),
            (416, "名"),
            (417, "姓"),
            (418, "全名"),
        ],
        _ => &[],
    }
}

struct CustomEditor {
    name: Entity<Input>,
    value: Entity<Input>,
    kind: u8,
    linked_id: Option<u32>,
    boolean: bool,
    had_name: bool,
    had_value: bool,
    original_kind: u8,
}

impl CustomEditor {
    fn new(field: Option<&DraftField>, item_type: u8, cx: &mut gpui::App) -> Self {
        let kind = field.map_or(0, |field| field.r#type);
        let name_value = field
            .and_then(|field| field.name.as_deref())
            .unwrap_or_default();
        let value = field
            .and_then(|field| field.value.as_deref())
            .unwrap_or_default();
        let name = cx.new(|cx| {
            let mut input = Input::new("字段名称", false, cx);
            input.set(name_value, cx);
            input
        });
        let value_input = cx.new(|cx| {
            let mut input = Input::multiline("值", kind == 1, cx);
            if kind != 3 && kind != 2 {
                input.set(value, cx);
            }
            input
        });
        let mut linked_id = field.and_then(|field| field.linked_id);
        if kind == 3
            && linked_id.is_none()
            && let Some((id, _)) = linked_options(item_type).first()
        {
            linked_id = Some(*id);
        }
        Self {
            name,
            value: value_input,
            kind,
            linked_id,
            boolean: value.eq_ignore_ascii_case("true"),
            had_name: field.is_some_and(|field| field.name.is_some()),
            had_value: field.is_some_and(|field| field.value.is_some()),
            original_kind: kind,
        }
    }

    fn set_kind(&mut self, kind: u8, item_type: u8, cx: &mut gpui::App) {
        if !self.committed(cx) || self.kind == kind {
            return;
        }
        let current = if self.kind == 2 {
            Zeroizing::new(if self.boolean { "true" } else { "false" }.to_owned())
        } else if self.kind == 3 {
            Zeroizing::default()
        } else {
            Zeroizing::new(self.value.read(cx).value().to_owned())
        };
        self.kind = kind;
        self.boolean = current.eq_ignore_ascii_case("true");
        if kind == 3 && self.linked_id.is_none() {
            self.linked_id = linked_options(item_type).first().map(|(id, _)| *id);
        }
        self.value.update(cx, |value, cx| {
            value.set_masked(kind == 1, cx);
            value.set(if kind == 2 || kind == 3 { "" } else { &current }, cx);
        });
    }

    fn committed(&self, cx: &gpui::App) -> bool {
        self.name.read(cx).committed()
            && (self.kind == 2 || self.kind == 3 || self.value.read(cx).committed())
    }

    fn collect(&self, cx: &gpui::App) -> DraftField {
        let name = self.name.read(cx).value();
        let value = self.value.read(cx).value();
        custom_field(
            self.kind,
            name,
            value,
            self.boolean,
            self.linked_id,
            self.had_name,
            self.had_value || self.kind != self.original_kind,
        )
    }
}

fn custom_field(
    kind: u8,
    name: &str,
    value: &str,
    boolean: bool,
    linked_id: Option<u32>,
    had_name: bool,
    had_value: bool,
) -> DraftField {
    let name = if name.is_empty() && !had_name {
        None
    } else {
        Some(name.to_owned())
    };
    let value = match kind {
        0 | 1 => {
            if value.is_empty() && !had_value {
                None
            } else {
                Some(value.to_owned())
            }
        }
        2 => Some(boolean.to_string()),
        _ => None,
    };
    DraftField {
        name,
        value,
        r#type: kind,
        linked_id: (kind == 3).then_some(linked_id).flatten(),
    }
}

fn remove_custom<T>(fields: &mut Vec<T>, index: usize) {
    if index < fields.len() {
        fields.remove(index);
    }
}

fn apply_uri_edits(draft: &mut Draft, uris: impl IntoIterator<Item = DraftUri>) {
    if draft.item_type == 1 {
        draft.uris = uris.into_iter().collect();
    } else {
        draft.uris.clear();
    }
}

impl Editor {
    fn remove_uri(&mut self, index: usize, cx: &mut gpui::App) {
        if let Some(uri) = self.uris.get(index) {
            uri.input.update(cx, |input, cx| input.set("", cx));
        }
        remove_uri(&mut self.uris, index);
    }

    fn remove_custom(&mut self, index: usize, cx: &mut gpui::App) {
        if let Some(field) = self.custom_fields.get(index) {
            for input in [&field.name, &field.value] {
                input.update(cx, |input, cx| input.set("", cx));
            }
        }
        remove_custom(&mut self.custom_fields, index);
    }

    fn for_each_input(&self, mut visit: impl FnMut(&Entity<Input>)) {
        visit(&self.name);
        visit(&self.notes.input);
        for (_, field) in self
            .fields
            .iter()
            .chain(self.type_fields.values().flatten())
        {
            visit(&field.input);
        }
        for uri in &self.uris {
            visit(&uri.input);
        }
        for field in &self.custom_fields {
            visit(&field.name);
            visit(&field.value);
        }
    }

    fn set_read_only(&self, read_only: bool, cx: &mut gpui::App) {
        self.for_each_input(|input| {
            input.update(cx, |input, cx| input.set_read_only(read_only, cx));
        });
    }

    pub(crate) fn clear(&mut self, cx: &mut gpui::App) {
        // The platform IME handler and the last hidden frame may own extra
        // Entity references. Wipe their buffers before releasing our handles.
        self.for_each_input(|input| {
            input.update(cx, |input, cx| input.set("", cx));
        });
        self.password.zeroize();
        self.draft = Draft::default();
    }

    pub(crate) fn folder_id(&self) -> Option<&str> {
        self.folder_id.as_deref()
    }

    pub(crate) fn choose_folder(&mut self, id: Option<String>) {
        self.folder_id = id;
        self.show_folders = false;
    }

    fn new(draft: Draft, password: Zeroizing<String>, cx: &mut gpui::App) -> Self {
        let name = cx.new(|cx| {
            let mut field = Input::new("条目名称", false, cx);
            field.set(&draft.name, cx);
            field
        });
        let notes = Editable::new("备注", true, draft.notes.as_deref(), cx);
        let fields = Self::make_fields(&draft, cx);
        let mut uris = draft
            .uris
            .iter()
            .map(|uri| UriEditor::new(Some(uri), cx))
            .collect::<Vec<_>>();
        if draft.item_type == 1 && uris.is_empty() {
            uris.push(UriEditor::new(None, cx));
        }
        let custom_fields = draft
            .custom_fields
            .iter()
            .map(|field| CustomEditor::new(Some(field), draft.item_type, cx))
            .collect();
        let folder_id = draft.folder_id.clone();
        Self {
            draft,
            password,
            name,
            notes,
            fields,
            uris,
            custom_fields,
            type_fields: Default::default(),
            folder_id,
            show_folders: false,
            must_resync: false,
        }
    }

    fn make_fields(draft: &Draft, cx: &mut gpui::App) -> Vec<(&'static str, Editable)> {
        specs(draft.item_type)
            .iter()
            .map(|&(key, label, masked)| {
                (
                    key,
                    Editable::new(label, masked, draft.values.get(key).map(String::as_str), cx),
                )
            })
            .collect()
    }

    fn collect(&mut self, cx: &mut gpui::App) {
        self.draft.name = self.name.read(cx).value().trim().to_owned();
        self.draft.folder_id = self.folder_id.clone();
        let value = self.notes.input.read(cx).value();
        let had_notes = self.draft.notes.is_some();
        self.draft.notes.zeroize();
        self.draft.notes = (had_notes || !value.is_empty()).then(|| value.to_owned());
        for (key, field) in &self.fields {
            let value = field.input.read(cx).value();
            if (!value.is_empty() || self.draft.values.contains_key(*key))
                && let Some(mut old) = self.draft.values.insert((*key).into(), value.into())
            {
                old.zeroize();
            }
        }
        let uris = self.uris.iter().map(|uri| uri.collect(cx));
        apply_uri_edits(&mut self.draft, uris);
        self.draft.custom_fields = self
            .custom_fields
            .iter()
            .map(|field| field.collect(cx))
            .collect();
    }

    fn linked_fields_compatible(&self) -> bool {
        let options = linked_options(self.draft.item_type);
        self.custom_fields.iter().all(|field| {
            field.kind != 3
                || field
                    .linked_id
                    .is_some_and(|id| options.iter().any(|(candidate, _)| *candidate == id))
        })
    }

    fn restore_after_error(&mut self, draft: Draft, code: &str) {
        self.draft = draft;
        self.must_resync |= matches!(code, "conflict" | "notSynced" | "writeUncertain");
    }

    fn switch_kind(&mut self, kind: u8, cx: &mut gpui::App) -> bool {
        if self.draft.id.is_some() || self.draft.item_type == kind || !self.committed(cx) {
            return false;
        }
        let previous = self.draft.item_type;
        self.type_fields
            .insert(previous, std::mem::take(&mut self.fields));
        self.draft.item_type = kind;
        // Types own independent inputs even when their schemas share field names.
        for value in self.draft.values.values_mut() {
            value.zeroize();
        }
        self.draft.values.clear();
        self.fields = self
            .type_fields
            .remove(&kind)
            .unwrap_or_else(|| Self::make_fields(&self.draft, cx));
        true
    }

    fn committed(&self, cx: &gpui::App) -> bool {
        self.name.read(cx).committed()
            && self.notes.committed(cx)
            && self.fields.iter().all(|(_, field)| field.committed(cx))
            && self.uris.iter().all(|uri| uri.input.read(cx).committed())
            && self.custom_fields.iter().all(|field| field.committed(cx))
    }
}

impl VaultPreview {
    pub(crate) fn write_available(&self) -> bool {
        self.client.ready()
            && self.client.form.is_none()
            && self.client.editor.is_none()
            && !self.client.write_guard
            && self
                .client
                .overview
                .as_ref()
                .is_some_and(|overview| !overview.read_only)
    }

    pub fn new_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match self.filter {
            Filter::Kind(kind) => kind,
            _ => 1,
        };
        self.new_item_of_kind(kind, window, cx);
    }

    pub fn new_item_of_kind(&mut self, kind: u8, window: &mut Window, cx: &mut Context<Self>) {
        if !self.write_available() {
            return;
        }
        self.close_detail();
        let mut draft = Draft::default();
        draft.item_type = kind;
        draft.folder_id = self
            .client
            .selected_folder
            .as_ref()
            .filter(|id| !id.is_empty())
            .cloned();
        let editor = Editor::new(draft, Zeroizing::default(), cx);
        editor.name.read(cx).focus(window);
        self.client.editor = Some(editor);
        self.ui.trigger_vault_narrow(true, self.animate);
        cx.notify();
    }

    pub fn edit_item(&mut self, cx: &mut Context<Self>) {
        if self.write_available() {
            self.clone_pending = false;
            self.fetch_editor(Zeroizing::default(), cx);
        }
    }

    pub fn clone_item(&mut self, cx: &mut Context<Self>) {
        if !self.write_available() {
            return;
        }
        let Some(item) = self
            .client
            .overview
            .as_ref()
            .and_then(|overview| overview.items.get(self.selected?))
        else {
            return;
        };
        if item.deleted || item.restricted {
            return;
        }
        self.clone_pending = true;
        self.fetch_editor(Zeroizing::default(), cx);
    }

    fn finish_editor_load(
        &mut self,
        load: EditorLoad,
        result: Result<Draft, bytewarden_core::desktop::Error>,
        password: Zeroizing<String>,
        cx: &mut Context<Self>,
    ) {
        if !load.is_current(
            self.client.form_generation,
            self.selected_item_id.as_deref(),
            self.clone_pending,
            self.lock_queued,
        ) {
            return;
        }
        match result {
            Ok(draft) if draft.can_edit && !draft.is_deleted => {
                self.close_form(cx);
                let (draft, password) = if load.clone_requested {
                    (clone_draft(draft), Zeroizing::default())
                } else {
                    (draft, password)
                };
                self.client.editor = Some(Editor::new(draft, password, cx));
                self.session_notice();
            }
            Ok(_) => {
                self.close_form(cx);
                self.close_detail();
                self.client.notice = "没有编辑权限。".into();
            }
            Err(err) if matches!(err.code(), "passwordRequired" | "wrongPassword") => {
                self.client.form = Some(Form::EditorPassword);
                self.client.notice = status_text(&err);
            }
            Err(err) => {
                self.close_form(cx);
                self.close_detail();
                self.client.notice = status_text(&err);
            }
        }
    }

    pub(crate) fn fetch_editor(&mut self, password: Zeroizing<String>, cx: &mut Context<Self>) {
        let clone_requested = self.clone_pending;
        let Some(id) = self
            .client
            .overview
            .as_ref()
            .and_then(|overview| overview.items.get(self.selected?))
            .map(|item| item.id.clone())
        else {
            return;
        };
        let load = EditorLoad {
            form_generation: self.client.form_generation,
            item_id: id.clone(),
            clone_requested,
        };
        self.client.redact_detail();
        let req = Request::begin();
        self.run(
            "正在载入编辑器",
            Some(req),
            cx,
            move || (req.editor(&id, &password), password),
            move |this, (result, password), cx| {
                this.finish_editor_load(load, result, password, cx);
            },
        );
    }

    pub fn save_editor(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        let Some(editor) = self.client.editor.as_mut() else {
            return;
        };
        if editor.must_resync {
            self.client.notice =
                "草稿已保留。请取消编辑并同步核对后重新编辑，避免重复提交。".into();
            cx.notify();
            return;
        }
        if !editor.committed(cx) {
            self.client.notice = "请先完成输入法文字输入。".into();
            cx.notify();
            return;
        }
        if !editor.linked_fields_compatible() {
            self.client.notice = "关联字段与当前条目类型不兼容，请重新选择或移除后保存。".into();
            cx.notify();
            return;
        }
        editor.collect(cx);
        if editor.draft.name.is_empty() {
            self.client.notice = "请输入条目名称。".into();
            cx.notify();
            return;
        }
        // Keep UI entities on the GUI thread. Send only the owned native draft and
        // a zeroizing copy of the reprompt value to the blocking worker.
        // Preserve only the metadata needed to render this frozen editor while
        // the worker owns the credential draft. Both DTOs zeroize on drop.
        let mut pending_view = Draft::default();
        pending_view.item_type = editor.draft.item_type;
        pending_view.id = editor.draft.id.clone();
        pending_view.name = editor.draft.name.clone();
        let draft = std::mem::replace(&mut editor.draft, pending_view);
        let password = Zeroizing::new(editor.password.to_string());
        // Keep the editor reachable for synchronous lock/dismissal cleanup,
        // while freezing inputs until this draft's response is processed.
        editor.set_read_only(true, cx);
        let form_generation = self.client.form_generation;
        let req = Request::begin();
        self.run(
            "正在保存条目",
            Some(req),
            cx,
            move || (req.save_item(&draft, &password), draft),
            move |this, (result, draft), cx| {
                this.finish_editor_save(form_generation, result, draft, cx);
            },
        );
    }

    fn finish_editor_save(
        &mut self,
        form_generation: u64,
        result: Result<bytewarden_core::desktop::Mutation, bytewarden_core::desktop::Error>,
        draft: Draft,
        cx: &mut Context<Self>,
    ) {
        if self.lock_queued || self.client.form_generation != form_generation {
            return;
        }
        match result {
            Ok(res) => {
                self.close_form(cx);
                self.close_detail();
                self.client.set_overview(res.overview);
                self.invalidate_query();
                self.session_notice();
            }
            Err(err) => {
                let Some(editor) = self.client.editor.as_mut() else {
                    return;
                };
                editor.restore_after_error(draft, err.code());
                editor.set_read_only(false, cx);
                self.client.notice = if editor.must_resync {
                    "保存结果需核对。草稿已保留；请取消编辑并同步后重新编辑，避免重复提交。".into()
                } else {
                    status_text(&err)
                };
            }
        }
    }

    pub fn render_editor(&self, now: std::time::Instant, cx: &mut Context<Self>) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{
                BtnVariant, btn, card_group, glyph_icon, pill_chip, rotating_chevron,
                toggle_switch_animated,
            },
            spring::clamp_opacity,
            tokens::{font, glyph, radius, space},
        };

        let pal = theme::palette();
        let editor = self.client.editor.as_ref().expect("editor is open");
        let folder = editor.folder_id.as_deref().map_or("无文件夹", |id| {
            self.folders.name(id, self.client.overview.as_ref())
        });
        let folder_picker_key = gpui::ElementId::Name("editor-folder-picker".into());
        let folder_picker_p = self.ui.collapsible_progress(
            &folder_picker_key,
            editor.show_folders,
            now,
            self.animate,
        );

        let mut stack = div().flex().flex_col().gap(px(space::M));

        // 1. Item Type (for new items) + Name + Folder card.
        let mut basic_body = div().p(px(space::L)).flex().flex_col().gap(px(space::M));
        if editor.draft.id.is_none() {
            basic_body = basic_body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child("条目类型"),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.)).children(
                            FILTERS
                                .iter()
                                .filter_map(|&(filter, label, _)| match filter {
                                    Filter::Kind(kind) => Some((kind, label)),
                                    _ => None,
                                })
                                .map(|(kind, label)| {
                                    pill_chip(
                                        ("type", usize::from(kind)),
                                        label,
                                        None,
                                        editor.draft.item_type == kind,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if let Some(editor) = &mut this.client.editor {
                                                if !editor.switch_kind(kind, cx)
                                                    && !editor.committed(cx)
                                                {
                                                    this.client.notice =
                                                        "请先完成输入法文字输入。".into();
                                                }
                                                cx.notify();
                                            }
                                        },
                                    ))
                                }),
                        ),
                    ),
            );
        }
        basic_body = basic_body
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(hex(pal.text_secondary))
                            .child("名称"),
                    )
                    .child(editor.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(hex(pal.text_secondary))
                            .child("所属文件夹"),
                    )
                    .child(
                        btn("editor-folder", BtnVariant::Secondary)
                            .justify_between()
                            .w_full()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(glyph_icon(glyph::FOLDER, 14.))
                                    .child(folder.to_owned()),
                            )
                            .child(rotating_chevron(
                                folder_picker_p,
                                12.,
                                hex(pal.text_muted).into(),
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(editor) = &mut this.client.editor {
                                    editor.show_folders = !editor.show_folders;
                                    this.ui.animate_collapsible(
                                        gpui::ElementId::Name("editor-folder-picker".into()),
                                        editor.show_folders,
                                        this.animate,
                                    );
                                    cx.notify();
                                }
                            })),
                    )
                    .when(editor.show_folders || folder_picker_p > 0.02, |el| {
                        el.child(
                            div()
                                .relative()
                                .top(px(-6. * (1. - folder_picker_p)))
                                .opacity(clamp_opacity(folder_picker_p))
                                .child(self.render_folder_picker(true, cx)),
                        )
                    }),
            );
        stack = stack.child(card_group("基本信息", Some(glyph::EDIT)).child(basic_body));

        // 2. Type-specific credential/detail fields card.
        if !editor.fields.is_empty() {
            let (gen_p, _) = self.ui.editor_gen_spring.sample_staggered(
                now,
                std::time::Duration::ZERO,
                self.animate,
            );
            let gen_props = EditorGenProps {
                open: self.ui.editor_gen_open,
                progress: gen_p,
                is_phrase: self.generator.editor_is_phrase(),
                length: self.generator.editor_length(),
                words: self.generator.editor_words(),
                has_special: self.generator.editor_has_special(),
            };
            stack = stack.child(
                card_group("凭据与核心字段", Some(glyph::KEY)).child(
                    div()
                        .p(px(space::L))
                        .flex()
                        .flex_col()
                        .gap(px(space::M))
                        .children(
                            editor
                                .fields
                                .iter()
                                .map(|(_, field)| field.render(gen_props, cx)),
                        ),
                ),
            );
        }

        // 3. Websites / URIs card (for Login items).
        if editor.draft.item_type == 1 {
            stack = stack.child(
                card_group("网站 / URI", Some(glyph::GLOBE)).child(
                    div()
                        .p(px(space::L))
                        .flex()
                        .flex_col()
                        .gap(px(space::M))
                        .children(editor.uris.iter().enumerate().map(|(index, uri)| {
                            div()
                                .p(px(10.))
                                .rounded(px(radius::CONTROL))
                                .bg(hex(pal.layer))
                                .border_1()
                                .border_color(hex(pal.border_subtle))
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(div().flex_1().min_w_0().child(uri.input.clone()))
                                        .child(
                                            btn(("uri-remove", index), BtnVariant::Danger)
                                                .h(px(30.))
                                                .px(px(8.))
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(glyph::DELETE, 12.))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(editor) = &mut this.client.editor {
                                                        editor.remove_uri(index, cx);
                                                        cx.notify();
                                                    }
                                                })),
                                        ),
                                )
                                .child(div().flex().flex_wrap().gap(px(4.)).children(
                                    URI_MATCHES.iter().enumerate().map(
                                        |(choice, (mode, label))| {
                                            let mode = *mode;
                                            pill_chip(
                                                ("uri-match", index * URI_MATCHES.len() + choice),
                                                *label,
                                                None,
                                                uri.match_mode == mode,
                                            )
                                            .h(px(24.))
                                            .px(px(8.))
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if let Some(editor) = &mut this.client.editor
                                                        && let Some(uri) =
                                                            editor.uris.get_mut(index)
                                                    {
                                                        uri.match_mode = mode;
                                                        cx.notify();
                                                    }
                                                }),
                                            )
                                        },
                                    ),
                                ))
                        }))
                        .child(
                            btn("uri-add", BtnVariant::Subtle)
                                .h(px(30.))
                                .justify_start()
                                .text_size(px(font::CAPTION))
                                .child(glyph_icon(glyph::ADD, 12.))
                                .child("添加网站 / URI")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(editor) = &mut this.client.editor {
                                        editor.uris.push(UriEditor::new(None, cx));
                                        cx.notify();
                                    }
                                })),
                        ),
                ),
            );
        }

        // 4. Custom fields card.
        stack = stack.child(
            card_group("自定义字段", Some(glyph::ADD)).child(
                div()
                    .p(px(space::L))
                    .flex()
                    .flex_col()
                    .gap(px(space::M))
                    .children(
                        editor
                            .custom_fields
                            .iter()
                            .enumerate()
                            .map(|(index, field)| {
                                let item_type = editor.draft.item_type;
                                let mut options = linked_options(item_type).to_vec();
                                if let Some(id) = field.linked_id
                                    && !options.iter().any(|(known, _)| *known == id)
                                {
                                    options.push((id, "原关联字段"));
                                }
                                div()
                                    .p(px(10.))
                                    .rounded(px(radius::CONTROL))
                                    .bg(hex(pal.layer))
                                    .border_1()
                                    .border_color(hex(pal.border_subtle))
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                div().flex_1().min_w_0().child(field.name.clone()),
                                            )
                                            .child(
                                                btn(("custom-remove", index), BtnVariant::Danger)
                                                    .h(px(30.))
                                                    .px(px(8.))
                                                    .text_size(px(font::CAPTION))
                                                    .child(glyph_icon(glyph::DELETE, 12.))
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            if let Some(editor) =
                                                                &mut this.client.editor
                                                            {
                                                                editor.remove_custom(index, cx);
                                                                cx.notify();
                                                            }
                                                        },
                                                    )),
                                            ),
                                    )
                                    .child(div().flex().flex_wrap().gap(px(4.)).children(
                                        CUSTOM_TYPES.iter().enumerate().filter_map(
                                            |(choice, (kind, label))| {
                                                if *kind == 3
                                                    && options.is_empty()
                                                    && field.kind != 3
                                                {
                                                    return None;
                                                }
                                                let kind = *kind;
                                                Some(
                                                    pill_chip(
                                                        (
                                                            "custom-type",
                                                            index * CUSTOM_TYPES.len() + choice,
                                                        ),
                                                        *label,
                                                        None,
                                                        field.kind == kind,
                                                    )
                                                    .h(px(24.))
                                                    .px(px(8.))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if let Some(editor) =
                                                            &mut this.client.editor
                                                            && let Some(field) =
                                                                editor.custom_fields.get_mut(index)
                                                        {
                                                            if field.committed(cx) {
                                                                field.set_kind(kind, item_type, cx);
                                                            } else {
                                                                this.client.notice =
                                                                    "请先完成输入法文字输入。"
                                                                        .into();
                                                            }
                                                            cx.notify();
                                                        }
                                                    })),
                                                )
                                            },
                                        ),
                                    ))
                                    .when(field.kind == 1, |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .child(reveal_button(&field.value, cx)),
                                        )
                                    })
                                    .when(field.kind <= 1, |el| el.child(field.value.clone()))
                                    .when(field.kind == 2, |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(px(font::CAPTION))
                                                        .text_color(hex(pal.text_secondary))
                                                        .child(if field.boolean {
                                                            "布尔值：true"
                                                        } else {
                                                            "布尔值：false"
                                                        }),
                                                )
                                                .child(
                                                    toggle_switch_animated(
                                                        ("custom-bool", index),
                                                        field.boolean,
                                                        self.ui.toggle_progress(
                                                            &("custom-bool", index).into(),
                                                            field.boolean,
                                                            now,
                                                            self.animate,
                                                        ),
                                                    )
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if let Some(editor) =
                                                            &mut this.client.editor
                                                            && let Some(field) =
                                                                editor.custom_fields.get_mut(index)
                                                        {
                                                            field.boolean = !field.boolean;
                                                            let next = field.boolean;
                                                            this.ui.animate_toggle(
                                                                ("custom-bool", index),
                                                                next,
                                                                this.animate,
                                                            );
                                                            cx.notify();
                                                        }
                                                    })),
                                                ),
                                        )
                                    })
                                    .when(field.kind == 3, |el| {
                                        el.child(div().flex().flex_wrap().gap(px(4.)).children(
                                            options.iter().enumerate().map(
                                                |(choice, (id, label))| {
                                                    let id = *id;
                                                    pill_chip(
                                                        ("custom-linked", index * 32 + choice),
                                                        format!("{label} ({id})"),
                                                        None,
                                                        field.linked_id == Some(id),
                                                    )
                                                    .h(px(24.))
                                                    .px(px(8.))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if let Some(editor) =
                                                            &mut this.client.editor
                                                            && let Some(field) =
                                                                editor.custom_fields.get_mut(index)
                                                        {
                                                            field.linked_id = Some(id);
                                                            cx.notify();
                                                        }
                                                    }))
                                                },
                                            ),
                                        ))
                                    })
                            }),
                    )
                    .child(
                        btn("custom-add", BtnVariant::Subtle)
                            .h(px(30.))
                            .justify_start()
                            .text_size(px(font::CAPTION))
                            .child(glyph_icon(glyph::ADD, 12.))
                            .child("添加自定义字段")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(editor) = &mut this.client.editor {
                                    editor.custom_fields.push(CustomEditor::new(
                                        None,
                                        editor.draft.item_type,
                                        cx,
                                    ));
                                    cx.notify();
                                }
                            })),
                    ),
            ),
        );

        // 5. Notes & security options card.
        stack = stack.child(
            card_group("备注与安全选项", Some(glyph::SHIELD)).child(
                div()
                    .p(px(space::L))
                    .flex()
                    .flex_col()
                    .gap(px(space::M))
                    .child(editor.notes.render(EditorGenProps::default(), cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .py(px(4.))
                            .child(
                                div()
                                    .text_size(px(font::BODY))
                                    .text_color(hex(pal.text))
                                    .child("收藏此条目"),
                            )
                            .child(
                                toggle_switch_animated(
                                    "editor-favorite",
                                    editor.draft.favorite,
                                    self.ui.toggle_progress(
                                        &gpui::ElementId::Name("editor-favorite".into()),
                                        editor.draft.favorite,
                                        now,
                                        self.animate,
                                    ),
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(editor) = &mut this.client.editor {
                                            editor.draft.favorite = !editor.draft.favorite;
                                            let next = editor.draft.favorite;
                                            this.ui.animate_toggle(
                                                "editor-favorite",
                                                next,
                                                this.animate,
                                            );
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .py(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(font::BODY))
                                            .text_color(hex(pal.text))
                                            .child("要求主密码复验"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(font::MICRO))
                                            .text_color(hex(pal.text_muted))
                                            .child("查看、复制或编辑前须再次验证主密码"),
                                    ),
                            )
                            .child(
                                toggle_switch_animated(
                                    "editor-reprompt",
                                    editor.draft.reprompt,
                                    self.ui.toggle_progress(
                                        &gpui::ElementId::Name("editor-reprompt".into()),
                                        editor.draft.reprompt,
                                        now,
                                        self.animate,
                                    ),
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(editor) = &mut this.client.editor {
                                            editor.draft.reprompt = !editor.draft.reprompt;
                                            let next = editor.draft.reprompt;
                                            this.ui.animate_toggle(
                                                "editor-reprompt",
                                                next,
                                                this.animate,
                                            );
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    )
                    .when(
                        editor
                            .draft
                            .passkeys
                            .as_ref()
                            .is_some_and(|p| !p.is_empty()),
                        |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .py(px(4.))
                                    .child(
                                        div()
                                            .text_size(px(font::BODY))
                                            .text_color(hex(pal.danger))
                                            .child("保存时移除现有通行密钥"),
                                    )
                                    .child(
                                        toggle_switch_animated(
                                            "editor-remove-passkey",
                                            editor.draft.remove_saved_passkey,
                                            self.ui.toggle_progress(
                                                &gpui::ElementId::Name(
                                                    "editor-remove-passkey".into(),
                                                ),
                                                editor.draft.remove_saved_passkey,
                                                now,
                                                self.animate,
                                            ),
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                if let Some(editor) = &mut this.client.editor {
                                                    editor.draft.remove_saved_passkey =
                                                        !editor.draft.remove_saved_passkey;
                                                    let next = editor.draft.remove_saved_passkey;
                                                    this.ui.animate_toggle(
                                                        "editor-remove-passkey",
                                                        next,
                                                        this.animate,
                                                    );
                                                    cx.notify();
                                                }
                                            }),
                                        ),
                                    ),
                            )
                        },
                    ),
            ),
        );

        stack
    }
}

type Spec = (&'static str, &'static str, bool);
fn specs(kind: u8) -> &'static [Spec] {
    match kind {
        1 => &[
            ("username", "用户名", false),
            ("password", "密码", true),
            ("totp", "验证器密钥", true),
        ],
        3 => &[
            ("cardholderName", "持卡人", false),
            ("number", "卡号", true),
            ("brand", "卡组织", false),
            ("expMonth", "到期月份", false),
            ("expYear", "到期年份", false),
            ("code", "安全码", true),
        ],
        4 => &[
            ("title", "称谓", false),
            ("firstName", "名", false),
            ("middleName", "中间名", false),
            ("lastName", "姓", false),
            ("username", "用户名", false),
            ("company", "公司", false),
            ("email", "邮箱", false),
            ("phone", "电话", false),
            ("address1", "地址 1", false),
            ("address2", "地址 2", false),
            ("address3", "地址 3", false),
            ("city", "城市", false),
            ("state", "省／州", false),
            ("postalCode", "邮编", false),
            ("country", "国家", false),
            ("ssn", "社会保障号码", true),
            ("passportNumber", "护照号码", true),
            ("licenseNumber", "驾照号码", true),
        ],
        5 => &[
            ("privateKey", "SSH 私钥", true),
            ("publicKey", "SSH 公钥", false),
            ("fingerprint", "指纹", false),
        ],
        6 => &[
            ("bankName", "银行名称", false),
            ("nameOnAccount", "账户持有人", false),
            ("accountType", "账户类型", false),
            ("accountNumber", "账号", true),
            ("routingNumber", "路由号码", true),
            ("branchNumber", "分行号码", false),
            ("pin", "PIN", true),
            ("swiftCode", "SWIFT", false),
            ("iban", "IBAN", true),
            ("bankContactPhone", "银行联系电话", false),
        ],
        7 => &[
            ("firstName", "名", false),
            ("middleName", "中间名", false),
            ("lastName", "姓", false),
            ("dateOfBirth", "出生日期", false),
            ("licenseNumber", "驾照号码", true),
            ("issuingCountry", "签发国家", false),
            ("issuingState", "签发省／州", false),
            ("issueDate", "签发日期", false),
            ("expirationDate", "到期日期", false),
            ("issuingAuthority", "签发机构", false),
            ("licenseClass", "准驾类型", false),
        ],
        8 => &[
            ("surname", "姓", false),
            ("givenName", "名", false),
            ("dateOfBirth", "出生日期", false),
            ("sex", "性别", false),
            ("birthPlace", "出生地", false),
            ("nationality", "国籍", false),
            ("issuingCountry", "签发国家", false),
            ("passportNumber", "护照号码", true),
            ("passportType", "护照类型", false),
            ("nationalIdentificationNumber", "国民身份号码", true),
            ("issuingAuthority", "签发机构", false),
            ("issueDate", "签发日期", false),
            ("expirationDate", "到期日期", false),
        ],
        _ => &[],
    }
}

#[cfg(test)]
mod uri_tests {
    use super::*;

    #[test]
    fn dismissed_editor_load_cannot_reopen_after_reselecting_the_same_item() {
        for clone_requested in [false, true] {
            let load = EditorLoad {
                form_generation: 7,
                item_id: "protected-item".into(),
                clone_requested,
            };
            assert!(load.is_current(7, Some("protected-item"), clone_requested, false));
            // Close the verification prompt, then reopen it for the same item.
            // Even an already-completed success/error belongs to the old form.
            assert!(!load.is_current(8, Some("protected-item"), clone_requested, false));
        }
    }

    #[test]
    fn editor_load_is_bound_to_selection_clone_intent_and_lock_state() {
        let load = EditorLoad {
            form_generation: 4,
            item_id: "original".into(),
            clone_requested: true,
        };
        assert!(load.is_current(4, Some("original"), true, false));
        assert!(!load.is_current(4, None, true, false));
        assert!(!load.is_current(4, Some("replacement"), true, false));
        assert!(!load.is_current(4, Some("original"), false, false));
        assert!(!load.is_current(4, Some("original"), true, true));
    }
    use bytewarden_core::desktop::{DraftField, Passkey};

    #[test]
    fn linked_field_targets_match_the_existing_desktop_field_ids() {
        assert_eq!(linked_options(1), [(100, "用户名"), (101, "密码")]);
        assert_eq!(
            linked_options(3),
            [
                (300, "持卡人"),
                (301, "到期月份"),
                (302, "到期年份"),
                (303, "安全码"),
                (304, "卡组织"),
                (305, "卡号"),
            ]
        );
        assert_eq!(
            linked_options(4),
            [
                (400, "称谓"),
                (401, "中间名"),
                (402, "地址 1"),
                (403, "地址 2"),
                (404, "地址 3"),
                (405, "城市"),
                (406, "省／州"),
                (407, "邮编"),
                (408, "国家"),
                (409, "公司"),
                (410, "邮箱"),
                (411, "电话"),
                (412, "社会保障号码"),
                (413, "用户名"),
                (414, "护照号码"),
                (415, "驾照号码"),
                (416, "名"),
                (417, "姓"),
                (418, "全名"),
            ]
        );
        assert!(linked_options(2).is_empty());
    }

    #[test]
    fn custom_field_projection_keeps_null_empty_and_type_specific_values() {
        let text = custom_field(0, "", "", false, Some(100), false, false);
        assert_eq!(text.name, None);
        assert_eq!(text.value, None);
        assert_eq!(text.linked_id, None);

        let preserved_empty = custom_field(1, "legacy", "", false, None, true, true);
        assert_eq!(preserved_empty.name.as_deref(), Some("legacy"));
        assert_eq!(preserved_empty.value.as_deref(), Some(""));
        assert_eq!(preserved_empty.r#type, 1);

        let boolean = custom_field(2, "enabled", "ignored", true, Some(100), true, false);
        assert_eq!(boolean.value.as_deref(), Some("true"));
        assert_eq!(boolean.linked_id, None);

        let linked = custom_field(3, "username", "must not leak", false, Some(100), true, true);
        assert_eq!(linked.value, None);
        assert_eq!(linked.linked_id, Some(100));
    }

    #[test]
    fn removing_a_custom_field_keeps_remaining_fields_in_order() {
        let mut fields = vec!["first", "remove", "last"];
        remove_custom(&mut fields, 1);
        assert_eq!(fields, ["first", "last"]);
        remove_custom(&mut fields, usize::MAX);
        assert_eq!(fields, ["first", "last"]);
    }

    #[test]
    fn uri_match_choices_preserve_core_wire_values() {
        assert_eq!(
            URI_MATCHES,
            [
                (None, "默认"),
                (Some(0), "域名"),
                (Some(1), "主机"),
                (Some(2), "开头匹配"),
                (Some(3), "完全匹配"),
                (Some(4), "正则表达式"),
                (Some(5), "从不"),
            ]
        );
    }

    #[test]
    fn uri_value_keeps_null_default_and_distinguishes_cleared_existing_values() {
        assert_eq!(uri_value(true, ""), None);
        assert_eq!(
            uri_value(true, "https://example.com"),
            Some("https://example.com".into())
        );
        assert_eq!(uri_value(false, ""), Some(String::new()));
    }

    #[test]
    fn uri_edits_replace_only_the_uri_projection_of_an_existing_draft() {
        let mut draft = Draft::default();
        draft.item_type = 1;
        draft.name = "kept login".into();
        draft.revision_date = Some("2026-11-25T00:00:00Z".into());
        draft.notes = Some("kept notes".into());
        draft
            .values
            .insert("password".into(), "kept password".into());
        draft.custom_fields.push(DraftField {
            name: Some("kept field".into()),
            value: Some("kept hidden value".into()),
            r#type: 1,
            linked_id: None,
        });
        draft.passkeys = Some(vec![Passkey {
            relying_party: "example.com".into(),
            user_name: Some("kept user".into()),
            created_at: "2026-11-25T00:00:00Z".into(),
        }]);
        let uris = vec![DraftUri {
            uri: Some("https://edited.example.com".into()),
            r#match: Some(3),
        }];

        apply_uri_edits(&mut draft, uris);

        assert_eq!(draft.name, "kept login");
        assert_eq!(draft.revision_date.as_deref(), Some("2026-11-25T00:00:00Z"));
        assert_eq!(draft.notes.as_deref(), Some("kept notes"));
        assert_eq!(
            draft.values.get("password").map(String::as_str),
            Some("kept password")
        );
        assert_eq!(
            draft.custom_fields[0].value.as_deref(),
            Some("kept hidden value")
        );
        assert_eq!(
            draft.passkeys.as_ref().unwrap()[0].relying_party,
            "example.com"
        );
        assert_eq!(draft.uris.len(), 1);
        assert_eq!(
            draft.uris[0].uri.as_deref(),
            Some("https://edited.example.com")
        );
        assert_eq!(draft.uris[0].r#match, Some(3));
    }

    #[test]
    fn changing_to_a_non_login_type_drops_login_uris() {
        let mut draft = Draft::default();
        draft.item_type = 2;
        draft.uris.push(DraftUri {
            uri: Some("https://example.com".into()),
            r#match: Some(0),
        });

        apply_uri_edits(&mut draft, std::iter::empty());

        assert!(draft.uris.is_empty());
    }

    #[test]
    fn uri_rows_round_trip_multiple_values_and_can_be_removed_by_position() {
        let mut rows = vec![
            DraftUri {
                uri: Some("https://example.com".into()),
                r#match: None,
            },
            DraftUri {
                uri: Some("login.example.com".into()),
                r#match: Some(1),
            },
            DraftUri {
                uri: Some("^https://.*$".into()),
                r#match: Some(4),
            },
        ];
        remove_uri(&mut rows, 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].uri.as_deref(), Some("https://example.com"));
        assert_eq!(rows[0].r#match, None);
        assert_eq!(rows[1].uri.as_deref(), Some("^https://.*$"));
        assert_eq!(rows[1].r#match, Some(4));
        remove_uri(&mut rows, usize::MAX);
        assert_eq!(rows.len(), 2);
    }
}
