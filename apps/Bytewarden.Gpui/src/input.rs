//! Native text editor with single-line and wrapped multiline modes. Sensitive
//! inputs keep owned text in a zeroizing buffer and deny clipboard/surrounding text
//! access even while the user explicitly reveals their display.
use std::{ops::Range, panic::Location};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, InspectorElementId, KeyBinding, LayoutId,
    MouseButton, PaintQuad, Pixels, Point, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;
use zeroize::{Zeroize, Zeroizing};

use crate::{ACCENT, DISABLED, theme::rgb};

actions!(
    desktop_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        Up,
        Down,
        SelectUp,
        SelectDown,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        Newline,
        DocumentHome,
        DocumentEnd,
        SelectDocumentHome,
        SelectDocumentEnd,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("DesktopInput")),
        KeyBinding::new("delete", Delete, Some("DesktopInput")),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, Some("DesktopInput")),
        KeyBinding::new("ctrl-delete", DeleteWordRight, Some("DesktopInput")),
        KeyBinding::new("ctrl-left", WordLeft, Some("DesktopInput")),
        KeyBinding::new("ctrl-right", WordRight, Some("DesktopInput")),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some("DesktopInput")),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, Some("DesktopInput")),
        KeyBinding::new("left", Left, Some("DesktopInput")),
        KeyBinding::new("right", Right, Some("DesktopInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("DesktopInput")),
        KeyBinding::new("shift-right", SelectRight, Some("DesktopInput")),
        KeyBinding::new("up", Up, Some("DesktopInput && mode == multiline")),
        KeyBinding::new("down", Down, Some("DesktopInput && mode == multiline")),
        KeyBinding::new(
            "shift-up",
            SelectUp,
            Some("DesktopInput && mode == multiline"),
        ),
        KeyBinding::new(
            "shift-down",
            SelectDown,
            Some("DesktopInput && mode == multiline"),
        ),
        KeyBinding::new("pageup", PageUp, Some("DesktopInput && mode == multiline")),
        KeyBinding::new(
            "pagedown",
            PageDown,
            Some("DesktopInput && mode == multiline"),
        ),
        KeyBinding::new(
            "shift-pageup",
            SelectPageUp,
            Some("DesktopInput && mode == multiline"),
        ),
        KeyBinding::new(
            "shift-pagedown",
            SelectPageDown,
            Some("DesktopInput && mode == multiline"),
        ),
        KeyBinding::new("enter", Newline, Some("DesktopInput && mode == multiline")),
        KeyBinding::new("ctrl-home", DocumentHome, Some("DesktopInput")),
        KeyBinding::new("ctrl-end", DocumentEnd, Some("DesktopInput")),
        KeyBinding::new("ctrl-shift-home", SelectDocumentHome, Some("DesktopInput")),
        KeyBinding::new("ctrl-shift-end", SelectDocumentEnd, Some("DesktopInput")),
        KeyBinding::new("home", Home, Some("DesktopInput")),
        KeyBinding::new("end", End, Some("DesktopInput")),
        KeyBinding::new("shift-home", SelectHome, Some("DesktopInput")),
        KeyBinding::new("shift-end", SelectEnd, Some("DesktopInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("DesktopInput")),
        KeyBinding::new("ctrl-c", Copy, Some("DesktopInput")),
        KeyBinding::new("ctrl-x", Cut, Some("DesktopInput")),
        KeyBinding::new("ctrl-v", Paste, Some("DesktopInput")),
    ]);
}

pub struct Input {
    focus: FocusHandle,
    placeholder: &'static str,
    value: Zeroizing<String>,
    anchor: usize,
    cursor: usize,
    cursor_at_wrap_end: bool,
    marked: Option<Range<usize>>,
    masked: bool,
    confidential: bool,
    multiline: bool,
    scroll_y: Pixels,
    reveal_cursor: bool,
    preferred_x: Option<Pixels>,
    geometry: Option<Geometry>,
    dragging: bool,
    borderless: bool,
    read_only: bool,
}

struct Geometry {
    bounds: Bounds<Pixels>,
    lines: Vec<TextLine>,
    shift: Point<Pixels>,
    line_height: Pixels,
    height: Pixels,
}

struct TextLine {
    range: Range<usize>,
    line: WrappedLine,
    top: Pixels,
}

impl Geometry {
    fn position(&self, value: &str, index: usize, masked: bool) -> Point<Pixels> {
        self.position_with_affinity(value, index, masked, false)
    }

    fn position_with_affinity(
        &self,
        value: &str,
        index: usize,
        masked: bool,
        at_wrap_end: bool,
    ) -> Point<Pixels> {
        let row = self
            .lines
            .iter()
            .find(|row| index <= row.range.end)
            .or_else(|| self.lines.last())
            .expect("text always has a line");
        let local = index.clamp(row.range.start, row.range.end) - row.range.start;
        let index = display_index(&value[row.range.clone()], local, masked);
        let mut position = row
            .line
            .position_for_index(index, self.line_height)
            .unwrap_or_default();
        // Soft wraps share a byte offset. End keeps the caret on the preceding
        // visual line; ordinary forward movement uses the following line.
        let mut previous = 0;
        for (i, boundary) in row.line.wrap_boundaries.iter().enumerate() {
            let boundary =
                row.line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
            if boundary == index {
                position = if at_wrap_end {
                    point(
                        row.line.unwrapped_layout.x_for_index(index)
                            - row.line.unwrapped_layout.x_for_index(previous),
                        self.line_height * i,
                    )
                } else {
                    point(px(0.), self.line_height * (i + 1))
                };
                break;
            }
            previous = boundary;
        }
        point(position.x, row.top + position.y)
    }

    fn offset(&self, value: &str, position: Point<Pixels>, masked: bool) -> usize {
        self.offset_with_affinity(value, position, masked).0
    }

    fn offset_with_affinity(
        &self,
        value: &str,
        position: Point<Pixels>,
        masked: bool,
    ) -> (usize, bool) {
        let position = position - self.bounds.origin + self.shift;
        let row = self
            .lines
            .iter()
            .rev()
            .find(|row| row.top <= position.y)
            .unwrap_or(&self.lines[0]);
        let height = row.line.size(self.line_height).height;
        let local = point(
            position.x,
            (position.y - row.top).clamp(px(0.), (height - px(1.)).max(px(0.))),
        );
        let shown = row
            .line
            .closest_index_for_position(local, self.line_height)
            .unwrap_or_else(|index| index);
        let source = &value[row.range.clone()];
        let at_wrap_end = row
            .line
            .wrap_boundaries
            .iter()
            .enumerate()
            .any(|(i, boundary)| {
                row.line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
                    == shown
                    && local.y < self.line_height * (i + 1)
            });
        (
            row.range.start + source_index(source, shown, masked),
            at_wrap_end,
        )
    }
}

// Hit testing (including masked bullets) must not place a caret inside a grapheme.
fn source_index(value: &str, shown: usize, masked: bool) -> usize {
    let offset = if masked {
        value
            .char_indices()
            .nth(shown / '•'.len_utf8())
            .map_or(value.len(), |(offset, _)| offset)
    } else {
        shown.min(value.len())
    };
    if offset == value.len() {
        offset
    } else {
        value
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .take_while(|&index| index <= offset)
            .last()
            .unwrap_or(0)
    }
}

fn display_index(value: &str, offset: usize, masked: bool) -> usize {
    if masked {
        value[..offset].chars().count() * '•'.len_utf8()
    } else {
        offset
    }
}

/// Retain original CRLF bytes in the buffer; layout excludes newline controls.
fn line_ranges(value: &str) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\r' || ch == '\n' {
            rows.push(start..index);
            start = index + ch.len_utf8();
            if ch == '\r' && chars.peek().is_some_and(|(_, ch)| *ch == '\n') {
                let (index, _) = chars.next().unwrap();
                start = index + 1;
            }
        }
    }
    rows.push(start..value.len());
    rows
}

fn accepted(value: &str, multiline: bool) -> bool {
    !value.contains('\0') && (multiline || !value.contains(['\r', '\n']))
}

impl Input {
    pub fn new(placeholder: &'static str, masked: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            placeholder,
            value: Zeroizing::default(),
            anchor: 0,
            cursor: 0,
            cursor_at_wrap_end: false,
            marked: None,
            masked,
            confidential: masked,
            multiline: false,
            scroll_y: px(0.),
            reveal_cursor: true,
            preferred_x: None,
            geometry: None,
            dragging: false,
            borderless: false,
            read_only: false,
        }
    }

    /// Freeze user edits while an owned draft is being submitted. Programmatic
    /// clearing remains available for lock, account switching and shutdown.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    pub fn set_borderless(&mut self, borderless: bool) {
        self.borderless = borderless;
    }

    pub fn multiline(placeholder: &'static str, masked: bool, cx: &mut Context<Self>) -> Self {
        let mut input = Self::new(placeholder, masked, cx);
        input.multiline = true;
        input
    }

    pub fn value(&self) -> &str {
        &self.value
    }
    pub fn committed(&self) -> bool {
        self.marked.is_none()
    }
    pub fn focus(&self, window: &mut Window) {
        self.focus.focus(window);
    }

    pub fn set(&mut self, value: &str, cx: &mut Context<Self>) {
        self.value.zeroize();
        self.value.push_str(value);
        self.anchor = self.value.len();
        self.cursor = self.anchor;
        self.cursor_at_wrap_end = false;
        self.dragging = false;
        self.marked = None;
        self.geometry = None;
        self.scroll_y = px(0.);
        self.reveal_cursor = true;
        self.preferred_x = None;
        cx.notify();
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.confidential = masked;
        self.masked = masked;
        self.geometry = None;
        self.reveal_cursor = true;
        cx.notify();
    }

    pub fn revealed(&self) -> bool {
        self.confidential && !self.masked
    }

    pub fn set_revealed(&mut self, revealed: bool, cx: &mut Context<Self>) {
        self.masked = self.confidential && !revealed;
        self.geometry = None;
        self.reveal_cursor = true;
        cx.notify();
    }

    pub fn take(&mut self, cx: &mut Context<Self>) -> Zeroizing<String> {
        let value = std::mem::take(&mut self.value);
        self.set("", cx);
        value
    }

    fn selected(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }
    fn real_range(&self, range: Range<usize>) -> Range<usize> {
        from_utf16(&self.value, range.start)..from_utf16(&self.value, range.end)
    }
    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.value[..range.start].encode_utf16().count()
            ..self.value[..range.end].encode_utf16().count()
    }
    fn position(&mut self, cursor: usize, extend: bool, cx: &mut Context<Self>) {
        self.position_with_affinity(cursor, false, extend, cx);
    }
    fn position_with_affinity(
        &mut self,
        cursor: usize,
        at_wrap_end: bool,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        // While composing, only the platform's input callbacks may move the
        // selection. Keyboard/mouse actions must not leave a stale marked range.
        if !self.committed() {
            return;
        }
        self.cursor = cursor;
        self.cursor_at_wrap_end = at_wrap_end;
        self.reveal_cursor = true;
        self.preferred_x = None;
        if !extend {
            self.anchor = cursor;
        }
        cx.notify();
    }
    fn adjacent(&self, right: bool) -> usize {
        let mut indices = self.value.grapheme_indices(true).map(|(offset, _)| offset);
        if right {
            indices
                .find(|&offset| offset > self.cursor)
                .unwrap_or(self.value.len())
        } else {
            indices
                .take_while(|&offset| offset < self.cursor)
                .last()
                .unwrap_or(0)
        }
    }
    fn step(&mut self, right: bool, extend: bool, cx: &mut Context<Self>) {
        let range = self.selected();
        let pos = if !extend && !range.is_empty() {
            if right { range.end } else { range.start }
        } else {
            self.adjacent(right)
        };
        self.position(pos, extend, cx);
    }
    fn word_adjacent(&self, right: bool) -> usize {
        if right {
            let mut segments = self
                .value
                .split_word_bound_indices()
                .filter(|(start, segment)| start + segment.len() > self.cursor);
            // Move past the current segment, then all following whitespace.
            // Apostrophes/combining characters follow Unicode word rules.
            segments.next();
            segments
                .find(|(_, segment)| !segment.chars().all(char::is_whitespace))
                .map_or(self.value.len(), |(start, _)| start)
        } else {
            self.value
                .split_word_bound_indices()
                .rev()
                .find(|(start, segment)| {
                    *start < self.cursor && !segment.chars().all(char::is_whitespace)
                })
                .map_or(0, |(start, _)| start)
        }
    }
    fn word_step(&mut self, right: bool, extend: bool, cx: &mut Context<Self>) {
        self.position(self.word_adjacent(right), extend, cx);
    }
    fn erase_word(&mut self, right: bool, cx: &mut Context<Self>) {
        if !self.committed() {
            return;
        }
        let mut range = self.selected();
        if range.is_empty() {
            if right {
                range.end = self.word_adjacent(true);
            } else {
                range.start = self.word_adjacent(false);
            }
        }
        if !range.is_empty() {
            self.replace(range, "", cx);
        }
    }
    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        // Dropping the old full buffer wipes deleted bytes as well as retained bytes.
        let capacity = self.value.len() - range.len() + text.len();
        if capacity > 65_536 || !accepted(text, self.multiline) {
            return;
        }
        let mut next = Zeroizing::new(String::with_capacity(capacity));
        next.push_str(&self.value[..range.start]);
        next.push_str(text);
        next.push_str(&self.value[range.end..]);
        self.value = next;
        self.geometry = None;
        self.marked = None;
        self.position(range.start + text.len(), false, cx);
    }
    fn erase(&mut self, right: bool, cx: &mut Context<Self>) {
        if !self.committed() {
            return;
        }
        let mut range = self.selected();
        if range.is_empty() {
            if right {
                range.end = self.adjacent(true);
            } else {
                range.start = self.adjacent(false);
            }
        }
        if !range.is_empty() {
            self.replace(range, "", cx);
        }
    }
    fn line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        if !self.committed() {
            return;
        }
        if let Some(g) = &self.geometry {
            let row = g
                .lines
                .iter()
                .find(|row| self.cursor <= row.range.end)
                .unwrap_or_else(|| g.lines.last().unwrap());
            let source = &self.value[row.range.clone()];
            let shown = display_index(
                source,
                self.cursor.clamp(row.range.start, row.range.end) - row.range.start,
                self.masked,
            );
            let mut start = 0;
            let mut stop = row.line.len();
            for boundary in &row.line.wrap_boundaries {
                let boundary =
                    row.line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                if boundary < shown || (boundary == shown && !self.cursor_at_wrap_end) {
                    start = boundary;
                } else {
                    stop = boundary;
                    break;
                }
            }
            let cursor =
                row.range.start + source_index(source, if end { stop } else { start }, self.masked);
            let at_wrap_end = end && stop < row.line.len();
            self.position_with_affinity(cursor, at_wrap_end, extend, cx);
            return;
        }
        let rows = line_ranges(&self.value);
        let row = rows
            .iter()
            .find(|row| self.cursor <= row.end)
            .unwrap_or_else(|| rows.last().unwrap());
        self.position(if end { row.end } else { row.start }, extend, cx);
    }
    fn vertical(&mut self, down: bool, extend: bool, page: bool, cx: &mut Context<Self>) {
        if !self.committed() {
            return;
        }
        let Some(g) = &self.geometry else { return };
        let current = g.position_with_affinity(
            &self.value,
            self.cursor,
            self.masked,
            self.cursor_at_wrap_end,
        );
        let x = self.preferred_x.unwrap_or(current.x);
        let distance = if page {
            g.line_height * (g.bounds.size.height / g.line_height).floor().max(1.)
        } else {
            g.line_height
        };
        let y = current.y + if down { distance } else { -distance };
        let (offset, at_wrap_end) = if y < px(0.) {
            (0, false)
        } else if y >= g.height {
            (self.value.len(), false)
        } else {
            g.offset_with_affinity(
                &self.value,
                g.bounds.origin + point(x, y) - g.shift,
                self.masked,
            )
        };
        self.position_with_affinity(offset, at_wrap_end, extend, cx);
        self.preferred_x = Some(x);
    }
    fn offset_at(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        Some(
            self.geometry
                .as_ref()?
                .offset_with_affinity(&self.value, position, self.masked),
        )
    }
}

fn from_utf16(text: &str, units: usize) -> usize {
    let mut found = 0;
    for (offset, ch) in text.char_indices() {
        if found + ch.len_utf16() > units {
            return offset;
        }
        found += ch.len_utf16();
    }
    text.len()
}

impl EntityInputHandler for Input {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if self.confidential {
            return None;
        }
        let range = self.real_range(range);
        *actual = Some(self.utf16_range(range.clone()));
        Some(self.value[range].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_range(self.selected()),
            reversed: self.cursor < self.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|range| self.utf16_range(range))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.real_range(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected());
        self.replace(range, text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.real_range(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected());
        if self.read_only
            || self.value.len() - range.len() + text.len() > 65_536
            || !accepted(text, self.multiline)
        {
            return;
        }
        let start = range.start;
        self.replace(range, text, cx);
        self.marked = (!text.is_empty()).then_some(start..start + text.len());
        if let Some(selection) = selection {
            self.anchor = start + from_utf16(text, selection.start);
            self.cursor = start + from_utf16(text, selection.end);
        }
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let g = self.geometry.as_ref()?;
        let range = self.real_range(range);
        let start = g.position(&self.value, range.start, self.masked) - g.shift;
        let end = g.position(&self.value, range.end, self.masked) - g.shift;
        let x = (bounds.left() + start.x).clamp(bounds.left(), bounds.right());
        let y = (bounds.top() + start.y).clamp(
            bounds.top(),
            (bounds.bottom() - g.line_height).max(bounds.top()),
        );
        Some(Bounds::new(
            point(x, y),
            size(
                if start.y == end.y {
                    (end.x - start.x).max(px(1.))
                } else {
                    px(1.)
                },
                g.line_height,
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self
            .geometry
            .as_ref()?
            .offset(&self.value, position, self.masked);
        Some(self.value[..offset].encode_utf16().count())
    }
}

impl Render for Input {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let read_only = self.read_only;
        let pal = crate::theme::palette();
        div()
            .relative()
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .h(px(if self.multiline {
                120.
            } else if self.borderless {
                26.
            } else {
                34.
            }))
            .px(px(if self.borderless { 2. } else { 10. }))
            .when(self.multiline, |el| el.py_2())
            .overflow_hidden()
            .when(read_only, |el| el.opacity(0.62))
            .when(!self.borderless, |el| {
                let hover_bg = if pal.dark {
                    crate::theme::hex(0x28282c)
                } else {
                    crate::theme::hex(0xffffff)
                };
                el.rounded(px(crate::ui::tokens::radius::CONTROL))
                    .bg(crate::theme::hex(if read_only {
                        pal.layer
                    } else if focused {
                        pal.card
                    } else {
                        pal.input
                    }))
                    .border_1()
                    .border_color(if focused && !read_only {
                        gpui::Hsla::from(crate::theme::hex(pal.accent_fill)).opacity(0.65)
                    } else {
                        crate::theme::hex(pal.border).into()
                    })
                    .when(!focused && !read_only, |el| {
                        el.hover(move |s| {
                            s.bg(hover_bg)
                                .border_color(crate::theme::hex(pal.text_muted))
                        })
                    })
                    .when(focused && !read_only, |el| {
                        el.shadow(vec![gpui::BoxShadow {
                            color: pal.focus_ring(),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(2.),
                        }])
                    })
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(if focused && !read_only { 2. } else { 1. }))
                            .bg(if focused && !read_only {
                                gpui::Hsla::from(crate::theme::hex(pal.accent_fill))
                            } else {
                                pal.input_rest_bottom()
                            }),
                    )
            })
            .track_focus(&self.focus)
            .tab_index(0)
            .key_context(if self.multiline {
                "DesktopInput mode=multiline"
            } else {
                "DesktopInput"
            })
            .cursor(if read_only {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.erase(false, cx)))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.erase(true, cx)))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| this.erase_word(false, cx)))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| this.erase_word(true, cx)))
            .on_action(cx.listener(|this, _: &WordLeft, _, cx| this.word_step(false, false, cx)))
            .on_action(cx.listener(|this, _: &WordRight, _, cx| this.word_step(true, false, cx)))
            .on_action(
                cx.listener(|this, _: &SelectWordLeft, _, cx| this.word_step(false, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectWordRight, _, cx| this.word_step(true, true, cx)),
            )
            .on_action(cx.listener(|this, _: &Left, _, cx| this.step(false, false, cx)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.step(true, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.step(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.step(true, true, cx)))
            .on_action(cx.listener(|this, _: &Up, _, cx| this.vertical(false, false, false, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.vertical(true, false, false, cx)))
            .on_action(
                cx.listener(|this, _: &SelectUp, _, cx| this.vertical(false, true, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectDown, _, cx| this.vertical(true, true, false, cx)),
            )
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.vertical(false, false, true, cx)))
            .on_action(
                cx.listener(|this, _: &PageDown, _, cx| this.vertical(true, false, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectPageUp, _, cx| this.vertical(false, true, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectPageDown, _, cx| this.vertical(true, true, true, cx)),
            )
            .on_action(cx.listener(|this, _: &Newline, _, cx| {
                // Enter first commits an active composition; it must not insert a
                // newline or bubble into the enclosing form's save action.
                if this.committed() {
                    this.replace(this.selected(), "\n", cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Home, _, cx| this.line_edge(false, false, cx)))
            .on_action(cx.listener(|this, _: &End, _, cx| this.line_edge(true, false, cx)))
            .on_action(cx.listener(|this, _: &SelectHome, _, cx| this.line_edge(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectEnd, _, cx| this.line_edge(true, true, cx)))
            .on_action(cx.listener(|this, _: &DocumentHome, _, cx| this.position(0, false, cx)))
            .on_action(cx.listener(|this, _: &DocumentEnd, _, cx| {
                this.position(this.value.len(), false, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectDocumentHome, _, cx| this.position(0, true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectDocumentEnd, _, cx| {
                this.position(this.value.len(), true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.committed() {
                    this.anchor = 0;
                    this.position(this.value.len(), true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if !this.confidential && !this.selected().is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        this.value[this.selected()].to_string(),
                    ));
                }
            }))
            .on_action(cx.listener(|this, _: &Cut, _, cx| {
                if !this.read_only
                    && this.committed()
                    && !this.confidential
                    && !this.selected().is_empty()
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        this.value[this.selected()].to_string(),
                    ));
                    this.replace(this.selected(), "", cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, window, cx| {
                if this.read_only {
                    return;
                }
                if let Some(value) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    let value = Zeroizing::new(value);
                    this.replace_text_in_range(None, &value, window, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                if this.multiline
                    && let Some(geometry) = &this.geometry
                {
                    let next = (this.scroll_y - event.delta.pixel_delta(geometry.line_height).y)
                        .clamp(
                            px(0.),
                            (geometry.height - geometry.bounds.size.height).max(px(0.)),
                        );
                    if next != this.scroll_y {
                        this.scroll_y = next;
                        this.reveal_cursor = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                    this.focus(window);
                    this.dragging = true;
                    if let Some((offset, at_wrap_end)) = this.offset_at(e.position) {
                        this.position_with_affinity(offset, at_wrap_end, e.modifiers.shift, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                if this.dragging
                    && let Some((offset, at_wrap_end)) = this.offset_at(e.position)
                {
                    this.position_with_affinity(offset, at_wrap_end, true, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .line_height(px(20.))
            .child(Line(cx.entity()))
    }
}

struct Line(Entity<Input>);
impl IntoElement for Line {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct PaintLine {
    geometry: Option<Geometry>,
    markers: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
}

impl Element for Line {
    type RequestLayoutState = ();
    type PrepaintState = PaintLine;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = if self.0.read(cx).multiline {
            relative(1.).into()
        } else {
            window.line_height().into()
        };
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PaintLine {
        let input = self.0.read(cx);
        let style = window.text_style();
        let line_height = window.line_height();
        let mut lines = Vec::new();
        let mut top = px(0.);
        for range in line_ranges(&input.value) {
            let source = &input.value[range.clone()];
            let mask = input.masked.then(|| "•".repeat(source.chars().count()));
            let display = if input.value.is_empty() {
                input.placeholder
            } else {
                mask.as_deref().unwrap_or(source)
            };
            let run = TextRun {
                len: display.len(),
                font: style.font(),
                color: if input.value.is_empty() {
                    rgb(DISABLED).into()
                } else {
                    style.color
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = if let Some(marked) = input
                .marked
                .as_ref()
                .filter(|marked| marked.start < range.end && marked.end > range.start)
            {
                let from = display_index(
                    source,
                    marked.start.max(range.start) - range.start,
                    input.masked,
                );
                let to = display_index(
                    source,
                    marked.end.min(range.end) - range.start,
                    input.masked,
                );
                vec![
                    TextRun {
                        len: from,
                        ..run.clone()
                    },
                    TextRun {
                        len: to - from,
                        underline: Some(UnderlineStyle {
                            color: Some(rgb(ACCENT).into()),
                            thickness: px(1.),
                            wavy: false,
                        }),
                        ..run.clone()
                    },
                    TextRun {
                        len: display.len() - to,
                        ..run
                    },
                ]
            } else {
                vec![run]
            };
            let line = window
                .text_system()
                .shape_text_uncached(
                    display,
                    style.font_size.to_pixels(window.rem_size()),
                    &runs,
                    input.multiline.then_some(bounds.size.width.max(px(1.))),
                    None,
                )
                .unwrap_or_default()
                .into_iter()
                .next()
                .unwrap_or_default();
            let height = line.size(line_height).height;
            lines.push(TextLine { range, line, top });
            top += height;
        }
        let mut geometry = Geometry {
            bounds,
            lines,
            shift: point(px(0.), input.scroll_y),
            line_height,
            height: top,
        };
        let cursor = geometry.position_with_affinity(
            &input.value,
            input.cursor,
            input.masked,
            input.cursor_at_wrap_end,
        );
        if input.multiline {
            if input.reveal_cursor {
                geometry.shift.y = geometry
                    .shift
                    .y
                    .min(cursor.y)
                    .max(cursor.y + line_height - bounds.size.height);
            }
            geometry.shift.y = geometry
                .shift
                .y
                .clamp(px(0.), (top - bounds.size.height).max(px(0.)));
        } else {
            geometry.shift = point((cursor.x - bounds.size.width + px(6.)).max(px(0.)), px(0.));
        }
        let selected = input.selected();
        let focused = input.focus.is_focused(window);
        let mut markers = Vec::new();
        if focused && !selected.is_empty() {
            for row in &geometry.lines {
                if selected.end < row.range.start || selected.start > row.range.end {
                    continue;
                }
                let source = &input.value[row.range.clone()];
                let from = display_index(
                    source,
                    selected.start.clamp(row.range.start, row.range.end) - row.range.start,
                    input.masked,
                );
                let to = display_index(
                    source,
                    selected.end.clamp(row.range.start, row.range.end) - row.range.start,
                    input.masked,
                );
                let edges = std::iter::once(0)
                    .chain(row.line.wrap_boundaries.iter().map(|boundary| {
                        row.line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
                            .index
                    }))
                    .chain([row.line.len()])
                    .collect::<Vec<_>>();
                for (i, edge) in edges.windows(2).enumerate() {
                    let start = from.max(edge[0]);
                    let end = to.min(edge[1]);
                    let newline = i + 2 == edges.len() && selected.end > row.range.end;
                    if start < end || newline {
                        let x0 = row.line.unwrapped_layout.x_for_index(edge[0]);
                        let x1 = row.line.unwrapped_layout.x_for_index(start.min(edge[1])) - x0;
                        let x2 = row.line.unwrapped_layout.x_for_index(end.max(edge[0])) - x0
                            + if newline { px(4.) } else { px(0.) };
                        let origin =
                            bounds.origin + point(x1, row.top + line_height * i) - geometry.shift;
                        markers.push(fill(
                            Bounds::new(origin, size((x2 - x1).max(px(1.)), line_height)),
                            rgb(0x495e70),
                        ));
                    }
                }
            }
        }
        let caret = (focused && selected.is_empty()).then(|| {
            fill(
                Bounds::new(
                    bounds.origin + cursor - geometry.shift,
                    size(px(1.), line_height),
                ),
                rgb(ACCENT),
            )
        });
        PaintLine {
            geometry: Some(geometry),
            markers,
            caret,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        data: &mut PaintLine,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.0.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.0.clone()), cx);
        let geometry = data.geometry.take().expect("paint follows prepaint");
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for marker in data.markers.drain(..) {
                window.paint_quad(marker);
            }
            for row in &geometry.lines {
                let origin = bounds.origin + point(px(0.), row.top) - geometry.shift;
                if origin.y + row.line.size(geometry.line_height).height >= bounds.top()
                    && origin.y < bounds.bottom()
                {
                    let _ = row.line.paint(
                        origin,
                        geometry.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            if let Some(caret) = data.caret.take() {
                window.paint_quad(caret);
            }
        });
        self.0.update(cx, |input, _| {
            input.scroll_y = geometry.shift.y;
            input.reveal_cursor = false;
            input.geometry = Some(geometry);
        });
    }
}

#[cfg(all(test, feature = "test-support"))]
#[path = "input_tests.rs"]
mod state_tests;

#[cfg(test)]
mod tests {
    use super::{accepted, from_utf16, line_ranges};

    #[test]
    fn newline_layout_keeps_empty_rows_and_original_byte_offsets() {
        let empty = line_ranges("");
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0], 0..0);
        assert_eq!(line_ranges("a\r\n甲\r😀\n"), [0..1, 3..6, 7..11, 12..12]);
        assert!(accepted("甲\r\n😀", true));
        assert!(!accepted("甲\r\n😀", false));
        assert!(!accepted("hidden\0value", true));
    }

    #[test]
    fn native_offsets_do_not_split_surrogate_pairs() {
        assert_eq!(from_utf16("甲😀乙", 1), 3);
        assert_eq!(from_utf16("甲😀乙", 2), 3);
        assert_eq!(from_utf16("甲😀乙", 3), 7);
        assert_eq!(from_utf16("甲😀乙", 99), 10);
    }
}
