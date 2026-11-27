//! Revocable, zeroizing text for decrypted values and generated secrets.
//! Layout and painting retain glyph geometry only, never plaintext cache keys.
use std::{
    cell::RefCell,
    hash::{Hash, Hasher},
    panic::Location,
    rc::{Rc, Weak},
};

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, TextRun, TextStyle, Window, WrappedLine,
    point, px, size,
};
use zeroize::{Zeroize, Zeroizing};

type Buffer = RefCell<Zeroizing<String>>;

const MAX_CACHED_SHAPES: usize = 64;

/// Glyph-only layout cache entry (`WrappedLine.text` from `shape_text_uncached` is empty).
/// Stores no plaintext and is wiped synchronously whenever `Registry::clear()` runs.
struct ShapeEntry {
    key_hash: u64,
    byte_len: usize,
    colored: bool,
    font_bits: u32,
    wrap_bits: Option<u32>,
    dark: bool,
    lines: Rc<[WrappedLine]>,
}

struct ShapeCache {
    seed: u64,
    entries: Vec<ShapeEntry>,
}

impl Default for ShapeCache {
    fn default() -> Self {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        seed ^= (&seed as *const u64 as usize) as u64;
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            seed ^= d.as_nanos() as u64;
        }
        Self {
            seed,
            entries: Vec::new(),
        }
    }
}

impl ShapeCache {
    fn hash_value(&self, value: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.seed.hash(&mut hasher);
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn get_or_shape(
        &mut self,
        value: &str,
        colored: bool,
        style: &TextStyle,
        font_size: Pixels,
        wrap_width: Option<Pixels>,
        window: &mut Window,
    ) -> Rc<[WrappedLine]> {
        if value.is_empty() {
            return Rc::from([]);
        }
        let key_hash = self.hash_value(value);
        let byte_len = value.len();
        let font_bits = f32::from(font_size).to_bits();
        let wrap_bits = wrap_width.map(|w| f32::from(w).round().to_bits());
        let dark = crate::theme::palette().dark;

        if let Some(hit) = self.entries.iter().find(|e| {
            e.key_hash == key_hash
                && e.byte_len == byte_len
                && e.colored == colored
                && e.font_bits == font_bits
                && e.wrap_bits == wrap_bits
                && e.dark == dark
        }) {
            return hit.lines.clone();
        }

        let shaped: Rc<[WrappedLine]> = window
            .text_system()
            .shape_text_uncached(
                value,
                font_size,
                &runs(value, style, colored),
                wrap_width,
                None,
            )
            .unwrap_or_default()
            .into_vec()
            .into();

        if self.entries.len() >= MAX_CACHED_SHAPES {
            self.entries.drain(0..MAX_CACHED_SHAPES / 2);
        }
        self.entries.push(ShapeEntry {
            key_hash,
            byte_len,
            colored,
            font_bits,
            wrap_bits,
            dark,
            lines: shaped.clone(),
        });
        shaped
    }
}

#[derive(Default)]
pub(crate) struct Registry {
    buffers: RefCell<Vec<Weak<Buffer>>>,
    shapes: Rc<RefCell<ShapeCache>>,
}

impl Registry {
    pub(crate) fn text(&self, value: &str, colored: bool) -> SensitiveText {
        let value = Rc::new(RefCell::new(Zeroizing::new(value.to_owned())));
        let mut buffers = self.buffers.borrow_mut();
        if buffers.len() >= 64 {
            buffers.retain(|buffer| buffer.strong_count() != 0);
        }
        buffers.push(Rc::downgrade(&value));
        SensitiveText {
            value,
            shapes: self.shapes.clone(),
            colored,
        }
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn retained_buffers_for_test(&self) -> Vec<Rc<Buffer>> {
        self.buffers
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect()
    }

    pub(crate) fn clear(&self) {
        self.shapes.borrow_mut().entries.clear();
        for buffer in self
            .buffers
            .borrow_mut()
            .drain(..)
            .filter_map(|b| b.upgrade())
        {
            buffer.borrow_mut().zeroize();
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(crate) struct SensitiveText {
    value: Rc<Buffer>,
    shapes: Rc<RefCell<ShapeCache>>,
    colored: bool,
}

fn runs(value: &str, style: &TextStyle, colored: bool) -> Vec<TextRun> {
    if !colored {
        return vec![style.to_run(value.len())];
    }
    let palette = crate::theme::palette();
    let mut runs: Vec<TextRun> = Vec::new();
    for ch in value.chars() {
        let color = crate::theme::hex(if ch.is_ascii_digit() {
            palette.syntax_digit
        } else if ch.is_alphabetic() || ch.is_whitespace() {
            palette.text
        } else {
            palette.syntax_symbol
        });
        if let Some(last) = runs.last_mut().filter(|last| last.color == color.into()) {
            last.len += ch.len_utf8();
        } else {
            let mut run = style.to_run(ch.len_utf8());
            run.color = color.into();
            runs.push(run);
        }
    }
    runs
}

impl IntoElement for SensitiveText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SensitiveText {
    type RequestLayoutState = ();
    type PrepaintState = Rc<[WrappedLine]>;
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
        _: &mut App,
    ) -> (LayoutId, ()) {
        let value = self.value.clone();
        let shapes = self.shapes.clone();
        let colored = self.colored;
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        (
            window.request_measured_layout(
                Default::default(),
                move |known, available, window, _| {
                    let wrap_width = known.width.or(match available.width {
                        AvailableSpace::Definite(width) => Some(width.max(px(1.))),
                        _ => None,
                    });
                    let value = value.borrow();
                    let lines = shapes
                        .borrow_mut()
                        .get_or_shape(&value, colored, &style, font_size, wrap_width, window);
                    let width = lines
                        .iter()
                        .map(|line| line.size(line_height).width)
                        .fold(px(0.), Pixels::max);
                    let height = lines
                        .iter()
                        .map(|line| line.size(line_height).height)
                        .fold(px(0.), |a, b| a + b);
                    size(known.width.unwrap_or(width), known.height.unwrap_or(height))
                },
            ),
            (),
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Rc<[WrappedLine]> {
        let value = self.value.borrow();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        self.shapes.borrow_mut().get_or_shape(
            &value,
            self.colored,
            &style,
            font_size,
            Some(bounds.size.width.max(px(1.))),
            window,
        )
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        lines: &mut Rc<[WrappedLine]>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.value.borrow().is_empty() {
            return;
        }
        let line_height = window.line_height();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let mut top = px(0.);
            for line in lines.iter() {
                let _ = line.paint(
                    bounds.origin + point(px(0.), top),
                    line_height,
                    Default::default(),
                    Some(bounds),
                    window,
                    cx,
                );
                top += line.size(line_height).height;
            }
        });
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use gpui::{TestAppContext, div, prelude::*};

    #[gpui::test]
    fn retained_elements_are_revoked_without_a_subsequent_frame(cx: &mut TestAppContext) {
        let registry = Rc::new(Registry::default());
        let retained = registry.text("synthetic-secret-92!", true);
        let old_value = retained.value.clone();
        let registry_for_view = registry.clone();
        let window = cx.add_window(move |_, _| SecretView(registry_for_view));
        // add_window returns a real test window; draw exercises measurement and painting.
        window
            .update(cx, |_, window, cx| {
                window.refresh();
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| {
                assert!(
                    !window
                        .text_system()
                        .contains_cached_text_for_test("synthetic-secret-92!")
                );
            })
            .unwrap();
        registry.clear();
        assert!(old_value.borrow().is_empty());
        assert!(retained.value.borrow().is_empty());
    }

    #[gpui::test]
    fn uncached_multiline_unicode_layout_matches_normal_shaping_without_retaining_text(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, _| SecretView(Rc::new(Registry::default())));
        window
            .update(cx, |_, window, _| {
                for text in [
                    "synthetic-92! 中文 😀 abcdefghijklmnopqrstuvwxyz",
                    "first-secret\nsecond-secret\n",
                    "",
                    "\n",
                ] {
                    let style = window.text_style();
                    let font_size = style.font_size.to_pixels(window.rem_size());
                    for wrap in [None, Some(px(45.)), Some(px(240.))] {
                        let runs = runs(text, &style, true);
                        // The uncached production path must work before a cached control is created.
                        let uncached = window
                            .text_system()
                            .shape_text_uncached(text, font_size, &runs, wrap, None)
                            .unwrap();
                        assert!(uncached.iter().all(|line| line.text.is_empty()));
                        let cached = window
                            .text_system()
                            .shape_text(text.to_owned().into(), font_size, &runs, wrap, None)
                            .unwrap();
                        assert_eq!(uncached.len(), cached.len());
                        for (uncached, cached) in uncached.iter().zip(&cached) {
                            assert_eq!(uncached.len(), cached.len());
                            assert_eq!(uncached.size(px(24.)), cached.size(px(24.)));
                            assert_eq!(uncached.wrap_boundaries, cached.wrap_boundaries);
                            assert_eq!(
                                uncached
                                    .unwrapped_layout
                                    .runs
                                    .iter()
                                    .map(|run| run.glyphs.len())
                                    .sum::<usize>(),
                                cached
                                    .unwrapped_layout
                                    .runs
                                    .iter()
                                    .map(|run| run.glyphs.len())
                                    .sum::<usize>()
                            );
                        }
                    }
                }
            })
            .unwrap();
    }

    struct SecretView(Rc<Registry>);
    impl gpui::Render for SecretView {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().child(self.0.text("synthetic-secret-92!", true))
        }
    }
}
