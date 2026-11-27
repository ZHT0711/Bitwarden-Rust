//! Session-only generation. Owned results, worker snapshots and bounded history
//! use zeroizing buffers; copying always uses the application's protected clipboard.
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use bytewarden_core::desktop;
use gpui::{Context, ElementId, Entity, Task, div, prelude::*, px};
use zeroize::Zeroizing;

use crate::{
    Page, VaultPreview,
    generator_preferences::{self, Scope},
    input::Input,
};

const UPPERCASE: u32 = 1;
const LOWERCASE: u32 = 2;
const NUMBERS: u32 = 4;
const SPECIAL: u32 = 8;
const AVOID_AMBIGUOUS: u32 = 16;
const PASSWORD_ERROR: &str = "当前选项无法在所选长度内满足，请增加长度或减少最少字符数。";
const PASSPHRASE_ERROR: &str = "密码短语需要 3–20 个单词，分隔符最多 16 个 UTF-16 字符。";
const USERNAME_ERROR: &str =
    "请检查基础邮箱、邮箱域名和网站名称；网站名称仅支持字母、数字、点、横线与下划线。";
const MAX_HISTORY: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UsernameKind {
    Word,
    Subaddress,
    Catchall,
}

struct UsernameFields {
    email: Entity<Input>,
    domain: Entity<Input>,
    website: Entity<Input>,
}

struct HistoryEntry {
    value: Zeroizing<String>,
    kind: &'static str,
    time: String,
}

#[derive(Default)]
struct History(VecDeque<HistoryEntry>);

impl History {
    fn record(&mut self, value: &str, kind: &'static str) {
        if value.is_empty()
            || self
                .0
                .front()
                .is_some_and(|entry| entry.value.as_str() == value)
        {
            return;
        }
        if self.0.len() == MAX_HISTORY {
            self.0.pop_back(); // Zeroizing drops the evicted credential.
        }
        self.0.push_front(HistoryEntry {
            value: Zeroizing::new(value.to_owned()),
            kind,
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
        });
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

impl Drop for History {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(super) struct Generator {
    options: Options,
    saved_options: Options,
    preference_scope: Scope,
    separator: Entity<Input>,
    pub(super) result: Option<Zeroizing<String>>,
    error: Option<&'static str>,
    regeneration: Option<Task<()>>,
    worker: Option<Task<()>>,
    request: Option<desktop::Request>,
    revision: u64,
    strength: Option<u8>,
    history: History,
    rendered_secrets: crate::sensitive_text::Registry,
    username: bool,
    username_kind: UsernameKind,
    username_capitalize: bool,
    username_number: bool,
    username_website: bool,
    username_fields: Option<UsernameFields>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Options {
    phrase: bool,
    length: u32,
    flags: u32,
    minimum_numbers: u32,
    minimum_special: u32,
    last_minimum_numbers: u32,
    last_minimum_special: u32,
    words: u32,
    separator: String,
    capitalize: bool,
    include_number: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            phrase: false,
            length: 14,
            flags: UPPERCASE | LOWERCASE | NUMBERS,
            minimum_numbers: 1,
            minimum_special: 0,
            last_minimum_numbers: 1,
            last_minimum_special: 1,
            words: 6,
            separator: "-".into(),
            capitalize: false,
            include_number: false,
        }
    }
}

#[derive(Clone, Copy)]
enum Count {
    Length,
    Words,
    Numbers,
    Special,
}

impl Count {
    fn bounds(self) -> (u32, u32) {
        match self {
            Self::Length => (5, 128),
            Self::Words => (3, 20),
            Self::Numbers | Self::Special => (0, 9),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Length => "长度",
            Self::Words => "单词数量",
            Self::Numbers => "最少数字",
            Self::Special => "最少特殊字符",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Length => "generator-length",
            Self::Words => "generator-words",
            Self::Numbers => "generator-minimum-numbers",
            Self::Special => "generator-minimum-special",
        }
    }
}

impl Options {
    fn normalized(mut self) -> Self {
        self.flags &= UPPERCASE | LOWERCASE | NUMBERS | SPECIAL | AVOID_AMBIGUOUS;
        if self.flags & (UPPERCASE | LOWERCASE | NUMBERS | SPECIAL) == 0 {
            self.flags |= LOWERCASE;
        }
        self.minimum_numbers = if self.flags & NUMBERS != 0 {
            self.minimum_numbers.clamp(1, 9)
        } else {
            0
        };
        self.minimum_special = if self.flags & SPECIAL != 0 {
            self.minimum_special.clamp(1, 9)
        } else {
            0
        };
        let required = u32::from(self.flags & UPPERCASE != 0)
            + u32::from(self.flags & LOWERCASE != 0)
            + self.minimum_numbers
            + self.minimum_special;
        self.length = self.length.clamp(5, 128).max(required);
        self.words = self.words.clamp(3, 20);
        self.separator = normalize_separator(&self.separator);
        self.last_minimum_numbers = if self.flags & NUMBERS != 0 {
            self.minimum_numbers
        } else {
            self.last_minimum_numbers.clamp(1, 9)
        };
        self.last_minimum_special = if self.flags & SPECIAL != 0 {
            self.minimum_special
        } else {
            self.last_minimum_special.clamp(1, 9)
        };
        self
    }

    fn from_preferences(text: &str) -> Option<Self> {
        if text.len() > 4096 {
            return None;
        }
        let mut values = HashMap::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let (key, value) = line.split_once('=')?;
            if !matches!(
                key,
                "version"
                    | "selected_type"
                    | "password_length"
                    | "uppercase"
                    | "lowercase"
                    | "numbers"
                    | "special"
                    | "avoid_ambiguous"
                    | "minimum_numbers"
                    | "minimum_special"
                    | "passphrase_words"
                    | "word_separator_hex"
                    | "capitalize_words"
                    | "include_number"
            ) || values.insert(key, value).is_some()
            {
                return None;
            }
        }
        if values.get("version").copied().unwrap_or("1") != "1" {
            return None;
        }
        let boolean = |key: &str, default: bool| match values.get(key).copied() {
            None => Some(default),
            Some("true") => Some(true),
            Some("false") => Some(false),
            Some(_) => None,
        };
        let number = |key: &str, default: u32| {
            values
                .get(key)
                .map_or(Some(default), |value| value.parse::<u32>().ok())
        };
        let selected_type = values
            .get("selected_type")
            .map_or(Some(0), |value| value.parse::<i32>().ok())?;
        let mut flags = 0;
        for (key, flag, default) in [
            ("uppercase", UPPERCASE, true),
            ("lowercase", LOWERCASE, true),
            ("numbers", NUMBERS, true),
            ("special", SPECIAL, false),
            ("avoid_ambiguous", AVOID_AMBIGUOUS, false),
        ] {
            if boolean(key, default)? {
                flags |= flag;
            }
        }
        let separator = values
            .get("word_separator_hex")
            .map_or_else(|| Some("-".to_owned()), |value| decode_hex(value))?;
        Some(
            Self {
                phrase: selected_type == 1,
                length: number("password_length", 14)?,
                flags,
                minimum_numbers: number("minimum_numbers", 1)?,
                minimum_special: number("minimum_special", 0)?,
                last_minimum_numbers: 1,
                last_minimum_special: 1,
                words: number("passphrase_words", 6)?,
                separator,
                capitalize: boolean("capitalize_words", false)?,
                include_number: boolean("include_number", false)?,
            }
            .normalized(),
        )
    }

    fn to_preferences(&self) -> String {
        let options = self.clone().normalized();
        let mut separator_hex = String::with_capacity(options.separator.len() * 2);
        for byte in options.separator.as_bytes() {
            use std::fmt::Write as _;
            let _ = write!(separator_hex, "{byte:02x}");
        }
        format!(
            "version=1\nselected_type={}\npassword_length={}\nuppercase={}\nlowercase={}\nnumbers={}\nspecial={}\navoid_ambiguous={}\nminimum_numbers={}\nminimum_special={}\npassphrase_words={}\nword_separator_hex={}\ncapitalize_words={}\ninclude_number={}\n",
            u8::from(options.phrase),
            options.length,
            options.flags & UPPERCASE != 0,
            options.flags & LOWERCASE != 0,
            options.flags & NUMBERS != 0,
            options.flags & SPECIAL != 0,
            options.flags & AVOID_AMBIGUOUS != 0,
            options.minimum_numbers,
            options.minimum_special,
            options.words,
            separator_hex,
            options.capitalize,
            options.include_number,
        )
    }

    fn count(&self, count: Count) -> u32 {
        match count {
            Count::Length => self.length,
            Count::Words => self.words,
            Count::Numbers => self.minimum_numbers,
            Count::Special => self.minimum_special,
        }
    }

    fn set_count(&mut self, count: Count, value: u32) {
        let (min, max) = count.bounds();
        let value = value.clamp(min, max);
        match count {
            Count::Length => self.length = value,
            Count::Words => self.words = value,
            Count::Numbers | Count::Special => {
                let (minimum, previous, flag) = match count {
                    Count::Numbers => (
                        &mut self.minimum_numbers,
                        &mut self.last_minimum_numbers,
                        NUMBERS,
                    ),
                    _ => (
                        &mut self.minimum_special,
                        &mut self.last_minimum_special,
                        SPECIAL,
                    ),
                };
                *minimum = value;
                if value > 0 {
                    *previous = value;
                    self.flags |= flag;
                } else {
                    self.flags &= !flag;
                }
            }
        }
    }

    fn toggle_password_flag(&mut self, flag: u32) {
        if flag == NUMBERS || flag == SPECIAL {
            let (count, previous) = if flag == NUMBERS {
                (Count::Numbers, self.last_minimum_numbers)
            } else {
                (Count::Special, self.last_minimum_special)
            };
            self.set_count(
                count,
                if self.flags & flag != 0 {
                    0
                } else {
                    previous.max(1)
                },
            );
        } else {
            self.flags ^= flag;
        }
    }

    fn generate(&self) -> Result<Zeroizing<String>, &'static str> {
        if self.phrase {
            if !(3..=20).contains(&self.words)
                || self.separator.encode_utf16().count() > 16
                || self.separator.chars().any(char::is_control)
            {
                return Err(PASSPHRASE_ERROR);
            }
            desktop::passphrase(
                self.words,
                &self.separator,
                self.capitalize,
                self.include_number,
            )
            .map_err(|_| PASSPHRASE_ERROR)
        } else {
            // The desktop adapter clamps length; reject invalid state instead of
            // silently generating a credential with different requested options.
            if !(5..=128).contains(&self.length)
                || self.flags & (UPPERCASE | LOWERCASE | NUMBERS | SPECIAL) == 0
                || self.minimum_numbers > 9
                || self.minimum_special > 9
                || self.minimum_numbers + self.minimum_special > self.length
            {
                return Err(PASSWORD_ERROR);
            }
            desktop::password(
                self.length,
                self.flags,
                self.minimum_numbers,
                self.minimum_special,
            )
            .map_err(|_| PASSWORD_ERROR)
        }
    }
}

struct UsernameJob {
    kind: UsernameKind,
    capitalize: bool,
    include_number: bool,
    email: Zeroizing<String>,
    domain: Zeroizing<String>,
    website: Option<Zeroizing<String>>,
}

impl UsernameJob {
    fn generate(self, request: desktop::Request) -> Result<Zeroizing<String>, &'static str> {
        let website = self.website.as_ref().map(|value| value.as_str());
        let options = match self.kind {
            UsernameKind::Word => desktop::Username::Word {
                capitalize: self.capitalize,
                include_number: self.include_number,
            },
            UsernameKind::Subaddress => desktop::Username::Subaddress {
                email: &self.email,
                website,
            },
            UsernameKind::Catchall => desktop::Username::Catchall {
                domain: &self.domain,
                website,
            },
        };
        request.username(options).map_err(|_| USERNAME_ERROR)
    }
}

fn decode_hex(value: &str) -> Option<String> {
    if value.len() > 128 || !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn normalize_separator(value: &str) -> String {
    if value.chars().any(char::is_control) {
        "-".into()
    } else {
        bounded_separator(value).to_owned()
    }
}

fn bounded_separator(value: &str) -> &str {
    let mut units = 0;
    let end = value
        .char_indices()
        .find_map(|(offset, ch)| {
            units += ch.len_utf16();
            (units > 16).then_some(offset)
        })
        .unwrap_or(value.len());
    &value[..end]
}

impl Generator {
    pub(super) fn new(cx: &mut Context<VaultPreview>) -> Self {
        let preference_scope = Scope::Anonymous;
        let options = generator_preferences::load(&preference_scope)
            .and_then(|text| Options::from_preferences(&text))
            .unwrap_or_default()
            .normalized();
        let saved_options = options.clone();
        let separator_value = options.separator.clone();
        Self {
            options,
            saved_options,
            preference_scope,
            separator: cx.new(|cx| {
                let mut input = Input::new("单词分隔符（最多 16 个字符）", false, cx);
                input.set(&separator_value, cx);
                input
            }),
            result: None,
            error: None,
            regeneration: None,
            worker: None,
            request: None,
            revision: 0,
            strength: None,
            history: History::default(),
            rendered_secrets: Default::default(),
            username: false,
            username_kind: UsernameKind::Word,
            username_capitalize: true,
            username_number: true,
            username_website: false,
            username_fields: None,
        }
    }

    pub(super) fn generate_for_editor(&self) -> Result<Zeroizing<String>, &'static str> {
        self.options.generate()
    }

    pub(super) fn editor_is_phrase(&self) -> bool {
        self.options.phrase
    }

    pub(super) fn editor_length(&self) -> u32 {
        self.options.length
    }

    pub(super) fn editor_words(&self) -> u32 {
        self.options.words
    }

    pub(super) fn editor_has_special(&self) -> bool {
        self.options.flags & SPECIAL != 0
    }

    pub(super) fn set_editor_phrase(&mut self, phrase: bool) {
        self.options.phrase = phrase;
        self.persist_preferences();
    }

    pub(super) fn set_editor_length(&mut self, length: u32) {
        self.options.phrase = false;
        self.options.length = length.clamp(5, 128);
        self.persist_preferences();
    }

    pub(super) fn set_editor_words(&mut self, words: u32) {
        self.options.phrase = true;
        self.options.words = words.clamp(3, 20);
        self.persist_preferences();
    }

    pub(super) fn toggle_editor_special(&mut self) {
        self.options.phrase = false;
        if self.options.flags & SPECIAL != 0 {
            self.options.flags &= !SPECIAL;
            self.options.minimum_special = 0;
        } else {
            self.options.flags |= SPECIAL;
            self.options.minimum_special = self.options.last_minimum_special.clamp(1, 9);
        }
        self.persist_preferences();
    }

    pub(super) fn editor_generation_message(&self) -> &'static str {
        if self.options.phrase {
            "已按当前设置生成密码短语并填入"
        } else {
            "已按当前设置生成密码并填入"
        }
    }

    fn persist_preferences(&mut self) {
        self.options = self.options.clone().normalized();
        if self.options == self.saved_options {
            return;
        }
        let content = self.options.to_preferences();
        if generator_preferences::save(&self.preference_scope, &content).is_ok() {
            self.saved_options = self.options.clone();
        }
    }

    pub(super) fn refresh_preference_scope(
        &mut self,
        scope: Scope,
        cx: &mut Context<VaultPreview>,
    ) {
        if self.preference_scope == scope {
            return;
        }
        self.persist_preferences();
        self.clear_inputs(cx);
        self.clear();
        self.preference_scope = scope;
        self.options = generator_preferences::load(&self.preference_scope)
            .and_then(|text| Options::from_preferences(&text))
            .unwrap_or_default()
            .normalized();
        self.saved_options = self.options.clone();
        let separator = self.options.separator.clone();
        self.separator
            .update(cx, |input, cx| input.set(&separator, cx));
    }

    pub(super) fn reset_saved_selection(&mut self, scope: &str) {
        let scope = Scope::Account(scope.to_owned());
        if self.preference_scope == scope {
            self.options.phrase = false;
            self.persist_preferences();
            return;
        }
        let Some(content) = generator_preferences::load(&scope) else {
            return;
        };
        let mut options = Options::from_preferences(&content).unwrap_or_default();
        options.phrase = false;
        let _ = generator_preferences::save(&scope, &options.normalized().to_preferences());
    }

    fn clear_history(&mut self) {
        self.rendered_secrets.clear();
        self.history.clear();
    }

    fn cancel_generation(&mut self) {
        self.rendered_secrets.clear();
        self.revision = self.revision.wrapping_add(1);
        self.regeneration = None;
        if let Some(request) = self.request.take() {
            request.cancel();
        }
        self.worker = None;
        self.result = None;
        self.strength = None;
        self.error = None;
    }

    pub(super) fn clear_inputs(&self, cx: &mut gpui::App) {
        if let Some(fields) = &self.username_fields {
            for input in [&fields.email, &fields.domain, &fields.website] {
                input.update(cx, |input, cx| input.set("", cx));
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.persist_preferences();
        self.cancel_generation();
        self.history.clear();
        // Live UI callers also invoke clear_inputs before dropping entities.
        self.username_fields = None;
        self.username = false;
        self.username_kind = UsernameKind::Word;
        self.username_capitalize = true;
        self.username_number = true;
        self.username_website = false;
    }
}

impl Drop for Generator {
    fn drop(&mut self) {
        self.clear();
    }
}

impl VaultPreview {
    pub(super) fn flush_generator_preferences(&mut self, cx: &mut Context<Self>) {
        let value = {
            let field = self.generator.separator.read(cx);
            field.committed().then(|| field.value().to_owned())
        };
        if let Some(value) = value {
            let normalized = normalize_separator(&value);
            self.generator.options.separator = normalized.clone();
            if normalized != value {
                self.generator
                    .separator
                    .update(cx, |field, cx| field.set(&normalized, cx));
            }
        }
        self.generator.persist_preferences();
    }

    pub(super) fn observe_generator(&mut self, cx: &mut Context<Self>) {
        cx.observe(&self.generator.separator, |this, input, cx| {
            let field = input.read(cx);
            if !field.committed() {
                return;
            }
            let separator = normalize_separator(field.value());
            let changed = separator != field.value();
            if changed {
                input.update(cx, |field, cx| field.set(&separator, cx));
            }
            if this.generator.options.separator != separator {
                this.generator.options.separator = separator;
                if this.page == Page::Generator {
                    this.schedule_generation(cx);
                }
            }
        })
        .detach();
    }

    fn ensure_username_fields(&mut self, cx: &mut Context<Self>) {
        if self.generator.username_fields.is_some() {
            return;
        }
        let fields = UsernameFields {
            email: cx.new(|cx| Input::new("基础邮箱：name@example.com", false, cx)),
            domain: cx.new(|cx| Input::new("邮箱域名：example.com", false, cx)),
            website: cx.new(|cx| Input::new("网站名称：example.com", false, cx)),
        };
        for input in [&fields.email, &fields.domain, &fields.website] {
            cx.observe(input, |this, input, cx| {
                if input.read(cx).committed()
                    && this.generator.username
                    && this.page == Page::Generator
                {
                    this.schedule_generation(cx);
                }
            })
            .detach();
        }
        self.generator.username_fields = Some(fields);
    }

    fn schedule_generation(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready() || self.lock_queued || self.closing || self.page != Page::Generator
        {
            return;
        }
        self.generator.cancel_generation();
        let clock = cx.background_executor().clone();
        let generation = self.client.overview_generation;
        let revision = self.generator.revision;
        self.generator.regeneration = Some(cx.spawn(async move |view, app| {
            clock.timer(Duration::from_millis(150)).await;
            view.update(app, |this, cx| {
                // Account/session clearing invalidates this epoch, so a delayed
                // edit cannot recreate a secret after lock, logout or switching.
                if this.page == Page::Generator
                    && this.client.overview_generation == generation
                    && this.generator.revision == revision
                    && !this.lock_queued
                    && !this.closing
                {
                    this.generate(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn generate(&mut self, cx: &mut Context<Self>) {
        self.generator.cancel_generation();
        if !self.client.ready() || self.lock_queued || self.closing || self.page != Page::Generator
        {
            cx.notify();
            return;
        }
        self.generator.persist_preferences();
        let username = if self.generator.username {
            self.ensure_username_fields(cx);
            let fields = self
                .generator
                .username_fields
                .as_ref()
                .expect("initialized fields");
            let job = UsernameJob {
                kind: self.generator.username_kind,
                capitalize: self.generator.username_capitalize,
                include_number: self.generator.username_number,
                email: Zeroizing::new(fields.email.read(cx).value().trim().to_owned()),
                domain: Zeroizing::new(fields.domain.read(cx).value().trim().to_owned()),
                website: self
                    .generator
                    .username_website
                    .then(|| Zeroizing::new(fields.website.read(cx).value().trim().to_owned())),
            };
            let request = desktop::Request::begin();
            self.generator.request = Some(request);
            Some((request, job))
        } else {
            None
        };
        let options = self.generator.options.clone();
        let kind = if self.generator.username {
            "用户名"
        } else if options.phrase {
            "密码短语"
        } else {
            "密码"
        };
        let revision = self.generator.revision;
        let generation = self.client.overview_generation;
        let request = self.generator.request;
        // SDK generation and strength estimation both run away from the GPUI thread.
        let work = cx.background_executor().spawn(async move {
            if let Some((request, job)) = username {
                job.generate(request).map(|value| (value, None))
            } else {
                options.generate().map(|value| {
                    let strength = desktop::password_strength(&value).ok().flatten();
                    (value, strength)
                })
            }
        });
        self.generator.worker = Some(cx.spawn(async move |view, app| {
            let result = work.await;
            view.update(app, |this, cx| {
                if this.generator.revision != revision {
                    return;
                }
                this.generator.worker = None;
                this.generator.request = None;
                if this.page != Page::Generator
                    || this.client.overview_generation != generation
                    || this.lock_queued
                    || this.closing
                    || request.is_some_and(|request| !request.is_current())
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok((value, strength)) => {
                        this.generator.history.record(&value, kind);
                        this.generator.result = Some(value);
                        this.generator.strength = strength;
                        this.generator.error = None;
                        this.ui.trigger_generator_refresh(this.animate);
                    }
                    Err(message) => {
                        this.generator.error = Some(message);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn generator_count(&self, count: Count, cx: &Context<Self>) -> gpui::Div {
        let pal = crate::theme::palette();
        let value = self.generator.options.count(count);
        let (min, max) = count.bounds();
        let show_slider = matches!(count, Count::Length | Count::Words);
        let presets: &[u32] = match count {
            Count::Length => &[8, 12, 16, 20, 24, 32, 48, 64, 96, 128],
            Count::Words => &[3, 4, 5, 6, 8, 10, 12, 16, 20],
            _ => &[],
        };
        let fill_ratio = if max > min {
            (value.saturating_sub(min) as f32 / (max - min) as f32).clamp(0., 1.)
        } else {
            0.
        };
        div()
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(crate::ui::tokens::font::BODY))
                            .text_color(crate::theme::hex(pal.text))
                            .child(count.label()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                crate::ui::components::btn(
                                    (count.id(), 0usize),
                                    crate::ui::components::BtnVariant::Secondary,
                                )
                                .size(px(28.))
                                .px_0()
                                .opacity(if value > min { 1. } else { 0.4 })
                                .child("−")
                                .when(value > min, |b| {
                                    b.on_click(cx.listener(move |this, _, _, cx| {
                                        let v =
                                            this.generator.options.count(count).saturating_sub(1);
                                        this.generator.options.set_count(count, v);
                                        this.schedule_generation(cx);
                                    }))
                                }),
                            )
                            .child(
                                div()
                                    .w(px(36.))
                                    .text_center()
                                    .font_family(crate::ui::tokens::family::MONO)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(crate::theme::hex(pal.accent_text))
                                    .child(value.to_string()),
                            )
                            .child(
                                crate::ui::components::btn(
                                    (count.id(), 1usize),
                                    crate::ui::components::BtnVariant::Secondary,
                                )
                                .size(px(28.))
                                .px_0()
                                .opacity(if value < max { 1. } else { 0.4 })
                                .child("+")
                                .when(value < max, |b| {
                                    b.on_click(cx.listener(move |this, _, _, cx| {
                                        let v =
                                            this.generator.options.count(count).saturating_add(1);
                                        this.generator.options.set_count(count, v);
                                        this.schedule_generation(cx);
                                    }))
                                }),
                            ),
                    ),
            )
            .when(show_slider, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .w_full()
                                .h(px(6.))
                                .rounded(px(crate::ui::tokens::radius::PILL))
                                .bg(crate::theme::hex(pal.layer))
                                .border_1()
                                .border_color(crate::theme::hex(pal.border_subtle))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(fill_ratio))
                                        .rounded(px(crate::ui::tokens::radius::PILL))
                                        .bg(crate::theme::hex(pal.accent_fill)),
                                ),
                        )
                        .child(div().flex().flex_wrap().gap(px(4.)).children(
                            presets.iter().copied().enumerate().map(|(i, target)| {
                                let active = value == target;
                                div()
                                    .id((count.id(), i + 10))
                                    .cursor_pointer()
                                    .h(px(22.))
                                    .px(px(7.))
                                    .rounded(px(crate::ui::tokens::radius::PILL))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_family(crate::ui::tokens::family::MONO)
                                    .text_size(px(crate::ui::tokens::font::MICRO))
                                    .when(active, |c| {
                                        c.bg(pal.tint(pal.accent_fill))
                                            .border_1()
                                            .border_color(pal.tint_border(pal.accent_fill))
                                            .text_color(crate::theme::hex(pal.accent_text))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                    })
                                    .when(!active, |c| {
                                        c.bg(crate::theme::hex(pal.layer))
                                            .text_color(crate::theme::hex(pal.text_muted))
                                            .hover(move |s| {
                                                s.bg(crate::theme::hex(pal.hover))
                                                    .text_color(crate::theme::hex(pal.text))
                                            })
                                    })
                                    .child(target.to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.generator.options.set_count(count, target);
                                        this.schedule_generation(cx);
                                    }))
                            }),
                        )),
                )
            })
    }

    fn username_options(&self, now: Instant, cx: &Context<Self>) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{pill_chip, toggle_switch_animated},
            tokens::{font, space},
        };
        let pal = theme::palette();
        let generator = &self.generator;
        let cap_p = self.ui.toggle_progress(
            &ElementId::Name("username-capitalize".into()),
            generator.username_capitalize,
            now,
            self.animate,
        );
        let num_p = self.ui.toggle_progress(
            &ElementId::Name("username-number".into()),
            generator.username_number,
            now,
            self.animate,
        );
        let web_p = self.ui.toggle_progress(
            &ElementId::Name("username-website-toggle".into()),
            generator.username_website,
            now,
            self.animate,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(hex(pal.text_secondary))
                    .child("用户名类型"),
            )
            .child(
                div().flex().flex_wrap().gap(px(6.)).children(
                    [
                        (UsernameKind::Word, "随机词"),
                        (UsernameKind::Subaddress, "加号邮箱别名"),
                        (UsernameKind::Catchall, "Catch-all 邮箱"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(i, (kind, label))| {
                        pill_chip(
                            ("username-kind", i),
                            label,
                            None,
                            generator.username_kind == kind,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.generator.username_kind = kind;
                            this.schedule_generation(cx);
                        }))
                    }),
                ),
            )
            .when(generator.username_kind == UsernameKind::Word, |el| {
                el.child(
                    div()
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(font::BODY))
                                .text_color(hex(pal.text))
                                .child("首字母大写"),
                        )
                        .child(
                            toggle_switch_animated(
                                "username-capitalize",
                                generator.username_capitalize,
                                cap_p,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.generator.username_capitalize =
                                    !this.generator.username_capitalize;
                                let next = this.generator.username_capitalize;
                                this.ui
                                    .animate_toggle("username-capitalize", next, this.animate);
                                this.schedule_generation(cx);
                            })),
                        ),
                )
                .child(
                    div()
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(font::BODY))
                                .text_color(hex(pal.text))
                                .child("附加随机数字"),
                        )
                        .child(
                            toggle_switch_animated(
                                "username-number",
                                generator.username_number,
                                num_p,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.generator.username_number = !this.generator.username_number;
                                let next = this.generator.username_number;
                                this.ui
                                    .animate_toggle("username-number", next, this.animate);
                                this.schedule_generation(cx);
                            })),
                        ),
                )
            })
            .when_some(generator.username_fields.as_ref(), |el, fields| {
                el.when(generator.username_kind == UsernameKind::Subaddress, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("基础邮箱"),
                            )
                            .child(fields.email.clone()),
                    )
                })
                .when(generator.username_kind == UsernameKind::Catchall, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("邮箱域名"),
                            )
                            .child(fields.domain.clone()),
                    )
                })
                .when(generator.username_kind != UsernameKind::Word, |el| {
                    el.child(
                        div()
                            .py(px(4.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(font::BODY))
                                    .text_color(hex(pal.text))
                                    .child("使用网站名称而非随机字符"),
                            )
                            .child(
                                toggle_switch_animated(
                                    "username-website-toggle",
                                    generator.username_website,
                                    web_p,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.generator.username_website =
                                            !this.generator.username_website;
                                        let next = this.generator.username_website;
                                        this.ui.animate_toggle(
                                            "username-website-toggle",
                                            next,
                                            this.animate,
                                        );
                                        this.schedule_generation(cx);
                                    },
                                )),
                            ),
                    )
                    .when(generator.username_website, |el| {
                        el.child(fields.website.clone())
                    })
                })
            })
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(hex(pal.text_muted))
                    .child("转发邮箱服务尚未接入。用户名选项仅在本次会话有效。"),
            )
    }

    fn generator_history(&self, now: Instant, cx: &Context<Self>) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{BtnVariant, badge, btn, card_group, glyph_icon, password_colored},
            tokens::{font, glyph, space},
        };
        let pal = theme::palette();
        card_group("生成历史记录（仅本次会话）", Some(glyph::HISTORY))
            .child(
                div()
                    .px(px(space::L))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(hex(pal.border_subtle))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_muted))
                            .child(format!(
                                "已记录 {} / {MAX_HISTORY} 条",
                                self.generator.history.0.len()
                            )),
                    )
                    .child(
                        btn("clear-generator-history", BtnVariant::Subtle)
                            .h(px(26.))
                            .px(px(8.))
                            .text_size(px(font::CAPTION))
                            .child(glyph_icon(glyph::DELETE, 11.))
                            .child("清空历史")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.generator.clear_history();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("generator-history")
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .children(
                        self.generator
                            .history
                            .0
                            .iter()
                            .enumerate()
                            .map(|(i, entry)| {
                                let tag = format!("gen-hist-{i}");
                                let copied = self.ui.is_recently_copied(&tag, now);
                                let kind_label = entry.kind;
                                let is_last = i + 1 == self.generator.history.0.len();
                                div()
                                    .px(px(space::L))
                                    .py(px(9.))
                                    .when(!is_last, |el| {
                                        el.border_b_1().border_color(hex(pal.border_subtle))
                                    })
                                    .hover(move |s| s.bg(hex(pal.hover)))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(space::M))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(3.))
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .id(("generator-history-value", i))
                                                    .overflow_x_scroll()
                                                    .child(password_colored(
                                                        &self.generator.rendered_secrets,
                                                        &entry.value,
                                                        13.5,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.))
                                                    .child(badge(entry.kind, pal.info))
                                                    .child(
                                                        div()
                                                            .text_size(px(font::MICRO))
                                                            .text_color(hex(pal.text_muted))
                                                            .child(entry.time.clone()),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        btn(
                                            ("copy-generator-history", i),
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
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                if let Some(entry) = this.generator.history.0.get(i)
                                                {
                                                    let value =
                                                        Zeroizing::new(entry.value.to_string());
                                                    this.copy_protected_labeled(
                                                        value, &tag, kind_label, cx,
                                                    );
                                                }
                                            }),
                                        ),
                                    )
                            }),
                    ),
            )
    }

    pub(super) fn generator(&self, now: Instant, cx: &Context<Self>) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{
                BtnVariant, btn, card_group, glyph_icon, info_bar, password_colored,
                rotating_refresh_icon, segmented_track, strength_meter, surface_card,
                toggle_switch_animated,
            },
            spring::clamp_opacity,
            toast::ToastKind,
            tokens::{font, glyph, radius, space},
        };

        let pal = theme::palette();
        let settings = &self.generator.options;
        let kind = if self.generator.username {
            "用户名"
        } else if settings.phrase {
            "密码短语"
        } else {
            "密码"
        };
        let mode_index = if self.generator.username {
            2
        } else if settings.phrase {
            1
        } else {
            0
        };
        let mode_pos = self.ui.segmented_position(
            &ElementId::Name("generator-mode-seg".into()),
            mode_index,
            now,
            self.animate,
        );
        let (spin_angle, _) =
            self.ui
                .generator_spin_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let (result_p, _) =
            self.ui
                .generator_result_spring
                .sample_staggered(now, Duration::ZERO, self.animate);

        div()
            .flex()
            .flex_col()
            .gap(px(space::L))
            .w_full()
            .max_w(px(760.))
            .mx_auto()
            .p(px(space::XL))
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
                            .child("密码生成器"),
                    )
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child("调用官方 Rust SDK 安全生成高强度密码、密码短语与用户名。"),
                    ),
            )
            // Hero Output Card with syntax coloring and strength bar.
            .child(
                surface_card()
                    .p(px(space::L))
                    .flex()
                    .flex_col()
                    .gap(px(space::M))
                    .child(
                        div()
                            .id("generator-result")
                            .overflow_x_scroll()
                            .p(px(space::L))
                            .w_full()
                            .min_w_0()
                            .rounded(px(radius::CONTROL))
                            .bg(hex(pal.layer))
                            .border_1()
                            .border_color(hex(pal.border_subtle))
                            .child(
                                div()
                                    .relative()
                                    .top(px(5. * (1. - result_p)))
                                    .opacity(clamp_opacity(0.4 + 0.6 * result_p))
                                    .child(match &self.generator.result {
                                        Some(value) => password_colored(
                                            &self.generator.rendered_secrets,
                                            value,
                                            20.,
                                        ),
                                        None => div()
                                            .text_size(px(font::BODY))
                                            .text_color(hex(pal.text_muted))
                                            .child(format!("点击下方「生成{kind}」")),
                                    }),
                            ),
                    )
                    .when_some(self.generator.strength, |el, score| {
                        el.child(strength_meter(score))
                    })
                    .when(
                        self.generator.worker.is_some() || self.generator.regeneration.is_some(),
                        |el| {
                            el.child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.accent_text))
                                    .child("正在生成…"),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .when(self.generator.result.is_some(), |el| {
                                let copied = self.ui.is_recently_copied("generator-result", now);
                                el.child(
                                    btn("copy-generated", BtnVariant::Primary)
                                        .child(glyph_icon(
                                            if copied { glyph::CHECK } else { glyph::COPY },
                                            14.,
                                        ))
                                        .child(if copied {
                                            "已复制".to_owned()
                                        } else {
                                            format!("复制{kind}")
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(value) = &this.generator.result {
                                                let secret = Zeroizing::new(value.to_string());
                                                this.copy_protected_labeled(
                                                    secret,
                                                    "generator-result",
                                                    kind,
                                                    cx,
                                                );
                                            }
                                        })),
                                )
                            })
                            .child(
                                btn(
                                    "generate",
                                    if self.generator.result.is_some() {
                                        BtnVariant::Secondary
                                    } else {
                                        BtnVariant::Primary
                                    },
                                )
                                .child(rotating_refresh_icon(
                                    spin_angle,
                                    14.,
                                    hex(if self.generator.result.is_some() {
                                        pal.text
                                    } else {
                                        pal.on_accent
                                    })
                                    .into(),
                                ))
                                .child(format!("重新生成{kind}"))
                                .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
                            )
                            .child(
                                btn("clear-generated", BtnVariant::Subtle)
                                    .child("清空")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.generator.cancel_generation();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // Options Card.
            .child(
                card_group("生成选项", Some(glyph::KEY)).child(
                    div()
                        .p(px(space::L))
                        .flex()
                        .flex_col()
                        .gap(px(space::M))
                        .child(
                            segmented_track(3, mode_pos).children(
                                [
                                    (false, false, "密码"),
                                    (true, false, "密码短语"),
                                    (false, true, "用户名"),
                                ]
                                .into_iter()
                                .enumerate()
                                .map(
                                    |(i, (phrase, username, label))| {
                                        let selected = mode_index == i;
                                        div()
                                            .id(("generator-mode", i))
                                            .relative()
                                            .cursor_pointer()
                                            .flex_1()
                                            .h(px(30.))
                                            .rounded(px(radius::CONTROL))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(font::CAPTION))
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
                                            .child(label)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this.generator.options.phrase != phrase
                                                    || this.generator.username != username
                                                {
                                                    let prev = if this.generator.username {
                                                        2
                                                    } else if this.generator.options.phrase {
                                                        1
                                                    } else {
                                                        0
                                                    };
                                                    this.generator.options.phrase = phrase;
                                                    this.generator.username = username;
                                                    this.ui.animate_segmented(
                                                        ElementId::Name(
                                                            "generator-mode-seg".into(),
                                                        ),
                                                        prev,
                                                        i,
                                                        this.animate,
                                                    );
                                                    if username {
                                                        this.ensure_username_fields(cx);
                                                    }
                                                    this.schedule_generation(cx);
                                                }
                                            }))
                                    },
                                ),
                            ),
                        )
                        .when(!settings.phrase && !self.generator.username, |el| {
                            el.child(self.generator_count(Count::Length, cx))
                                .child(
                                    div().flex().flex_col().gap(px(6.)).children(
                                        [
                                            (UPPERCASE, "大写字母 (A-Z)"),
                                            (LOWERCASE, "小写字母 (a-z)"),
                                            (NUMBERS, "数字 (0-9)"),
                                            (SPECIAL, "特殊字符 (!@#$%^&*)"),
                                            (AVOID_AMBIGUOUS, "避免易混淆字符 (0/O, 1/l/I)"),
                                        ]
                                        .into_iter()
                                        .enumerate()
                                        .map(
                                            |(i, (flag, label))| {
                                                let on = settings.flags & flag != 0;
                                                let prog = self.ui.toggle_progress(
                                                    &("generator-flag", i).into(),
                                                    on,
                                                    now,
                                                    self.animate,
                                                );
                                                div()
                                                    .py(px(4.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .text_size(px(font::BODY))
                                                            .text_color(hex(pal.text))
                                                            .child(label),
                                                    )
                                                    .child(
                                                        toggle_switch_animated(
                                                            ("generator-flag", i),
                                                            on,
                                                            prog,
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.generator
                                                                    .options
                                                                    .toggle_password_flag(flag);
                                                                let next =
                                                                    this.generator.options.flags
                                                                        & flag
                                                                        != 0;
                                                                this.ui.animate_toggle(
                                                                    ("generator-flag", i),
                                                                    next,
                                                                    this.animate,
                                                                );
                                                                this.schedule_generation(cx);
                                                            },
                                                        )),
                                                    )
                                            },
                                        ),
                                    ),
                                )
                                .child(self.generator_count(Count::Numbers, cx))
                                .child(self.generator_count(Count::Special, cx))
                        })
                        .when(settings.phrase && !self.generator.username, |el| {
                            let cap_p = self.ui.toggle_progress(
                                &ElementId::Name("generator-capitalize".into()),
                                settings.capitalize,
                                now,
                                self.animate,
                            );
                            let num_p = self.ui.toggle_progress(
                                &ElementId::Name("generator-number".into()),
                                settings.include_number,
                                now,
                                self.animate,
                            );
                            el.child(self.generator_count(Count::Words, cx))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .text_size(px(font::CAPTION))
                                                .text_color(hex(pal.text_secondary))
                                                .child("单词分隔符（最多 16 个字符）"),
                                        )
                                        .child(self.generator.separator.clone()),
                                )
                                .child(
                                    div()
                                        .py(px(4.))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child("每个单词首字母大写")
                                        .child(
                                            toggle_switch_animated(
                                                "generator-capitalize",
                                                settings.capitalize,
                                                cap_p,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.generator.options.capitalize =
                                                        !this.generator.options.capitalize;
                                                    let next = this.generator.options.capitalize;
                                                    this.ui.animate_toggle(
                                                        "generator-capitalize",
                                                        next,
                                                        this.animate,
                                                    );
                                                    this.schedule_generation(cx);
                                                }),
                                            ),
                                        ),
                                )
                                .child(
                                    div()
                                        .py(px(4.))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child("包含一个数字")
                                        .child(
                                            toggle_switch_animated(
                                                "generator-number",
                                                settings.include_number,
                                                num_p,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.generator.options.include_number =
                                                        !this.generator.options.include_number;
                                                    let next =
                                                        this.generator.options.include_number;
                                                    this.ui.animate_toggle(
                                                        "generator-number",
                                                        next,
                                                        this.animate,
                                                    );
                                                    this.schedule_generation(cx);
                                                }),
                                            ),
                                        ),
                                )
                        })
                        .when(self.generator.username, |el| {
                            el.child(self.username_options(now, cx))
                        })
                        .when_some(self.generator.error, |el, message| {
                            el.child(info_bar(ToastKind::Warning, message))
                        }),
                ),
            )
            .when(!self.generator.history.0.is_empty(), |el| {
                el.child(self.generator_history(now, cx))
            })
    }
}

#[cfg(all(test, feature = "test-support"))]
#[path = "generator_security_tests.rs"]
mod security_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_history_is_bounded_newest_first_deduplicated_and_clearable() {
        let mut history = History::default();
        history.record("", "密码");
        assert!(history.0.is_empty());
        for index in 0..MAX_HISTORY + 12 {
            history.record(&format!("synthetic-{index}"), "密码");
        }
        assert_eq!(history.0.len(), MAX_HISTORY);
        assert_eq!(history.0.front().unwrap().value.as_str(), "synthetic-61");
        assert_eq!(history.0.back().unwrap().value.as_str(), "synthetic-12");
        history.record("synthetic-61", "密码");
        assert_eq!(history.0.len(), MAX_HISTORY);
        assert_eq!(history.0.back().unwrap().value.as_str(), "synthetic-12");
        history.clear();
        assert!(history.0.is_empty());
    }

    #[test]
    fn generator_defaults_match_winui_password_and_passphrase_options() {
        let options = Options::default();
        assert!(!options.phrase);
        assert_eq!((options.length, options.flags), (14, 7));
        assert_eq!((options.minimum_numbers, options.minimum_special), (1, 0));
        assert_eq!(options.words, 6);
        assert_eq!(options.separator, "-");
        assert!(!options.capitalize && !options.include_number);
    }

    #[test]
    fn generator_preferences_round_trip_and_normalize_winui_bounds() {
        let options = Options {
            phrase: true,
            length: 200,
            flags: UPPERCASE | NUMBERS | SPECIAL | AVOID_AMBIGUOUS,
            minimum_numbers: 10,
            minimum_special: 2,
            last_minimum_numbers: 10,
            last_minimum_special: 2,
            words: 2,
            separator: "😀::".into(),
            capitalize: true,
            include_number: true,
        };
        let serialized = options.to_preferences();
        let restored = Options::from_preferences(&serialized).expect("valid persisted settings");
        assert_eq!(restored, options.normalized());
        assert_eq!(restored.length, 128);
        assert_eq!(restored.minimum_numbers, 9);
        assert_eq!(restored.words, 3);

        let settings = Options::from_preferences(
            "version=1\nuppercase=false\nlowercase=false\nnumbers=false\nspecial=false\nword_separator_hex=09\n",
        )
        .expect("legacy defaults with a control separator");
        assert_eq!(settings.flags & LOWERCASE, LOWERCASE);
        assert_eq!(settings.separator, "-");
        assert_eq!(settings.length, 14);
    }

    #[test]
    fn generator_preferences_reject_unknown_duplicate_and_unsupported_values() {
        assert!(Options::from_preferences("version=2").is_none());
        assert!(Options::from_preferences("version=1\nunknown=value").is_none());
        assert!(Options::from_preferences("version=1\nversion=1").is_none());
        assert!(Options::from_preferences("version=1\nuppercase=perhaps").is_none());
        assert!(Options::from_preferences("version=1\nword_separator_hex=xy").is_none());
        assert!(Options::from_preferences(&"x".repeat(4097)).is_none());
    }

    #[test]
    fn generator_minimum_counts_toggle_classes_and_restore_previous_values() {
        let mut options = Options::default();
        for (count, flag) in [(Count::Numbers, NUMBERS), (Count::Special, SPECIAL)] {
            options.set_count(count, 5);
            assert_ne!(options.flags & flag, 0);
            options.toggle_password_flag(flag);
            assert_eq!(options.count(count), 0);
            assert_eq!(options.flags & flag, 0);
            // Automatic generation persists normalized preferences while the
            // category is disabled; it must retain the user's previous count.
            options = options.normalized();
            options.toggle_password_flag(flag);
            assert_eq!(options.count(count), 5);
            options.set_count(count, 0);
            assert_eq!(options.flags & flag, 0);
            options = options.normalized();
            options.toggle_password_flag(flag);
            assert_eq!(options.count(count), 5);
        }
        options.set_count(Count::Special, 0);
        options.last_minimum_special = 1;
        options.toggle_password_flag(SPECIAL);
        assert_eq!(options.minimum_special, 1);
    }

    #[test]
    fn generator_count_controls_enforce_winui_ranges() {
        let mut options = Options::default();
        for (count, min, max) in [
            (Count::Length, 5, 128),
            (Count::Words, 3, 20),
            (Count::Numbers, 0, 9),
            (Count::Special, 0, 9),
        ] {
            options.set_count(count, 0);
            assert_eq!(options.count(count), min);
            options.set_count(count, u32::MAX);
            assert_eq!(options.count(count), max);
        }
    }

    #[test]
    fn generator_password_honors_classes_and_minimums() {
        let mut options = Options {
            length: 24,
            flags: 0,
            ..Options::default()
        };
        options.set_count(Count::Numbers, 4);
        options.set_count(Count::Special, 3);
        let value = options.generate().expect("valid password options");
        assert_eq!(value.len(), 24);
        assert!(value.chars().all(|ch| !ch.is_ascii_alphabetic()));
        assert!(value.chars().filter(char::is_ascii_digit).count() >= 4);
        assert!(
            value
                .chars()
                .filter(|ch| !ch.is_ascii_alphanumeric())
                .count()
                >= 3
        );
    }

    #[test]
    fn generator_password_honors_uppercase_only_and_ambiguous_exclusion() {
        let options = Options {
            length: 128,
            flags: UPPERCASE | AVOID_AMBIGUOUS,
            minimum_numbers: 0,
            ..Options::default()
        };
        let value = options.generate().expect("valid uppercase-only options");
        assert_eq!(value.len(), 128);
        assert!(value.chars().all(|ch| ch.is_ascii_uppercase()));
        assert!(!value.contains(['I', 'O', 'l', '0', '1']));
    }

    #[test]
    fn generator_passphrase_honors_separator_capitalization_and_number() {
        let options = Options {
            phrase: true,
            words: 4,
            separator: "::".into(),
            capitalize: true,
            include_number: true,
            ..Options::default()
        };
        let value = options.generate().expect("valid passphrase options");
        let words: Vec<_> = value.split("::").collect();
        assert_eq!(words.len(), 4);
        assert!(words.iter().all(|word| {
            word.chars()
                .find(char::is_ascii_alphabetic)
                .is_some_and(|ch| ch.is_ascii_uppercase())
        }));
        assert_eq!(value.chars().filter(char::is_ascii_digit).count(), 1);
    }

    #[test]
    fn generator_separator_input_matches_winui_utf16_limit_without_splitting_scalars() {
        assert_eq!(bounded_separator("12345678901234567"), "1234567890123456");
        assert_eq!(bounded_separator("😀😀😀😀😀😀😀😀😀"), "😀😀😀😀😀😀😀😀");
        assert_eq!(bounded_separator("123456789012345😀"), "123456789012345");
        assert_eq!(bounded_separator(""), "");
    }

    #[test]
    fn generator_rejects_invalid_options_instead_of_silently_clamping() {
        let mut options = Options {
            flags: AVOID_AMBIGUOUS,
            ..Options::default()
        };
        assert!(options.generate().is_err());
        options.flags = NUMBERS | SPECIAL;
        options.length = 5;
        options.minimum_numbers = 4;
        options.minimum_special = 4;
        assert!(options.generate().is_err());
        options.length = 129;
        assert!(options.generate().is_err());
        options.phrase = true;
        options.words = 2;
        assert!(options.generate().is_err());
        options.words = 6;
        for separator in [
            "\0",
            "\r",
            "\n",
            "\t",
            "12345678901234567",
            "😀😀😀😀😀😀😀😀😀",
        ] {
            options.separator = separator.into();
            assert!(options.generate().is_err());
        }
        options.separator = String::new();
        assert!(options.generate().is_ok());
        options.separator = "😀😀😀😀😀😀😀😀".into();
        assert!(options.generate().is_ok());
    }
}
