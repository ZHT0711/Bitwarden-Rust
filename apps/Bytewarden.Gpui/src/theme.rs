//! Application-wide Fluent 2 semantic palette and Windows 11 Mica backdrop.
//! GPUI renders this single-window application on its UI thread, including child
//! inputs and tooltips. Thread-local values keep shared controls independent of
//! the vault entity, without locks, timers, or per-row allocations.

use std::cell::Cell;

use gpui::{Hsla, Rgba, Window, WindowAppearance};

use crate::{ACCENT, BG, BORDER, DISABLED, MUTED, PANEL, SELECTED, SHELL, TEXT};

#[cfg(windows)]
#[path = "theme_windows.rs"]
mod windows;

/// WinUI's accent text and fill brushes use different system shades. Keep both
/// palettes so an explicit app theme does not inherit the Windows light/dark choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Accents {
    light_text: u32,
    dark_text: u32,
    light_fill: u32,
    dark_fill: u32,
}

impl Accents {
    const FALLBACK: Self = Self {
        light_text: 0x0067c0,
        dark_text: ACCENT,
        light_fill: 0x0067c0,
        dark_fill: ACCENT,
    };

    fn text(self, dark: bool) -> u32 {
        if dark {
            self.dark_text
        } else {
            self.light_text
        }
    }

    fn fill(self, dark: bool) -> u32 {
        if dark {
            self.dark_fill
        } else {
            self.light_fill
        }
    }
}

/// Semantic Fluent 2 palette roles for both light and dark themes.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub mica: bool,
    pub shell: u32,
    pub layer: u32,
    pub card: u32,
    pub card_elevated: u32,
    pub input: u32,
    pub hover: u32,
    pub selected: u32,
    pub border: u32,
    pub border_subtle: u32,
    pub text: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub accent_text: u32,
    pub accent_fill: u32,
    pub on_accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub info: u32,
    pub syntax_digit: u32,
    pub syntax_symbol: u32,
}

impl Palette {
    fn for_mode(dark: bool, mica: bool, accents: Accents) -> Self {
        let (shell, layer, card, selected) = if dark {
            (SHELL, 0x252527, 0x2c2c2f, 0x3c3c42)
        } else {
            (0xf0f2f5, 0xf6f7f9, 0xffffff, 0xe3e7ed)
        };
        let backgrounds = [shell, layer, card, selected];
        let accent_fill = soften_accent(accents.fill(dark), dark, true, &backgrounds);
        let accent_text = soften_accent(accents.text(dark), dark, false, &backgrounds);
        let on_accent = if relative_luminance(accent_fill) > 0.32 {
            0x0c0c0c
        } else {
            0xffffff
        };
        if dark {
            Self {
                dark: true,
                mica,
                shell,
                layer,
                card,
                card_elevated: 0x323236,
                input: 0x222225,
                hover: 0x36363a,
                selected,
                border: 0x3e3e44,
                border_subtle: 0x323237,
                text: TEXT,
                text_secondary: MUTED,
                text_muted: 0xa8a8a8,
                accent_text,
                accent_fill,
                on_accent,
                success: 0x6ccb5f,
                warning: 0xfce100,
                danger: 0xff99a4,
                info: 0x60cdff,
                syntax_digit: 0x60cdff,
                syntax_symbol: 0xffb366,
            }
        } else {
            Self {
                dark: false,
                mica,
                shell,
                layer,
                card,
                card_elevated: 0xffffff,
                input: 0xfbfcfd,
                hover: 0xeceff3,
                selected,
                border: 0xdcdde2,
                border_subtle: 0xe8eaef,
                text: 0x1a1a1a,
                text_secondary: 0x575b63,
                text_muted: 0x626770,
                accent_text,
                accent_fill,
                on_accent,
                success: 0x0c6b0c,
                warning: 0x8a5000,
                danger: 0xbc2618,
                info: 0x005fb8,
                syntax_digit: 0x005fb8,
                syntax_symbol: 0xb24000,
            }
        }
    }

    /// Shell backdrop fill: softly translucent when Windows 11 Mica is active,
    /// opaque Fluent shell otherwise.
    pub fn shell_bg(self) -> Hsla {
        let base: Hsla = gpui::rgb(self.shell).into();
        if self.mica {
            base.opacity(if self.dark { 0.62 } else { 0.72 })
        } else {
            base
        }
    }

    /// Layer backdrop fill (content canvas).
    pub fn layer_bg(self) -> Hsla {
        let base: Hsla = gpui::rgb(self.layer).into();
        if self.mica {
            base.opacity(if self.dark { 0.88 } else { 0.92 })
        } else {
            base
        }
    }

    /// Soft tinted surface from any semantic color (badges, selected pills, InfoBars).
    pub fn tint(self, hex: u32) -> Hsla {
        let base: Hsla = gpui::rgb(hex).into();
        base.opacity(if self.dark { 0.14 } else { 0.09 })
    }

    /// Subtle semantic border tint.
    pub fn tint_border(self, hex: u32) -> Hsla {
        let base: Hsla = gpui::rgb(hex).into();
        base.opacity(if self.dark { 0.32 } else { 0.24 })
    }

    /// Fluent 2 input rest bottom baseline stroke.
    pub fn input_rest_bottom(self) -> Hsla {
        if self.dark {
            gpui::hsla(0., 0., 1., 0.24)
        } else {
            gpui::hsla(0., 0., 0., 0.32)
        }
    }

    /// Soft focus halo tint derived from softened system accent.
    pub fn focus_ring(self) -> Hsla {
        Hsla::from(gpui::rgb(self.accent_fill)).opacity(if self.dark { 0.24 } else { 0.16 })
    }

    /// Dimmed modal / drawer backdrop scrim scaled by animation alpha [0, 1].
    pub fn scrim(self, alpha: f32) -> Hsla {
        let strength = if self.dark { 0.50 } else { 0.22 };
        gpui::hsla(0., 0., 0., (alpha.clamp(0., 1.) * strength).clamp(0., 0.7))
    }
}

/// Soften overly vivid system accent shades in HSL space while preserving hue
/// and guaranteeing WCAG AA (>= 4.5:1) contrast on all theme surfaces.
fn soften_accent(raw: u32, dark: bool, is_fill: bool, backgrounds: &[u32]) -> u32 {
    let (h, s, l) = rgb_to_hsl(raw);
    let max_sat = if dark {
        if is_fill { 0.50 } else { 0.56 }
    } else if is_fill {
        0.54
    } else {
        0.58
    };
    let s = s.min(max_sat);
    let mut l = if dark {
        if is_fill {
            l.clamp(0.60, 0.74)
        } else {
            l.clamp(0.68, 0.82)
        }
    } else if is_fill {
        l.clamp(0.32, 0.44)
    } else {
        l.clamp(0.28, 0.40)
    };

    let passes = |candidate: u32| {
        if is_fill {
            let on = if relative_luminance(candidate) > 0.32 {
                0x0c0c0c
            } else {
                0xffffff
            };
            contrast_ratio(candidate, on) >= 4.5
        } else {
            backgrounds
                .iter()
                .all(|&bg| contrast_ratio(bg, candidate) >= 4.5)
        }
    };

    let mut candidate = hsl_to_rgb(h, s, l);
    if !passes(candidate) {
        for _ in 0..24 {
            l = if dark {
                (l + 0.02).min(0.94)
            } else {
                (l - 0.02).max(0.14)
            };
            candidate = hsl_to_rgb(h, s, l);
            if passes(candidate) {
                break;
            }
        }
    }
    candidate
}

fn rgb_to_hsl(rgb: u32) -> (f64, f64, f64) {
    let r = ((rgb >> 16) & 0xff) as f64 / 255.;
    let g = ((rgb >> 8) & 0xff) as f64 / 255.;
    let b = (rgb & 0xff) as f64 / 255.;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let delta = max - min;
    if delta <= 1e-6 {
        return (0., 0., l);
    }
    let s = if l > 0.5 {
        delta / (2. - max - min)
    } else {
        delta / (max + min)
    };
    let mut h = if (max - r).abs() <= 1e-6 {
        (g - b) / delta + if g < b { 6. } else { 0. }
    } else if (max - g).abs() <= 1e-6 {
        (b - r) / delta + 2.
    } else {
        (r - g) / delta + 4.
    };
    h /= 6.;
    (h, s, l)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> u32 {
    let hue_to_rgb = |p: f64, q: f64, mut t: f64| {
        if t < 0. {
            t += 1.;
        }
        if t > 1. {
            t -= 1.;
        }
        if t < 1. / 6. {
            p + (q - p) * 6. * t
        } else if t < 0.5 {
            q
        } else if t < 2. / 3. {
            p + (q - p) * (2. / 3. - t) * 6.
        } else {
            p
        }
    };
    let (r, g, b) = if s <= 1e-6 {
        (l, l, l)
    } else {
        let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
        let p = 2. * l - q;
        (
            hue_to_rgb(p, q, h + 1. / 3.),
            hue_to_rgb(p, q, h),
            hue_to_rgb(p, q, h - 1. / 3.),
        )
    };
    let to_u8 = |v: f64| (v.clamp(0., 1.) * 255.).round() as u32;
    (to_u8(r) << 16) | (to_u8(g) << 8) | to_u8(b)
}

fn contrast_ratio(a: u32, b: u32) -> f64 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A failed refresh retains the last usable palette. A revision is attempted only
/// once, including failure, so a virtual list cannot repeatedly call Windows.
#[cfg(any(windows, test))]
#[derive(Default)]
struct AccentCache {
    revision: Option<u64>,
    colors: Option<Accents>,
}

#[cfg(any(windows, test))]
impl AccentCache {
    fn refresh(&mut self, revision: u64, read: impl FnOnce() -> Option<Accents>) -> Accents {
        if self.revision != Some(revision) {
            self.revision = Some(revision);
            if let Some(colors) = read() {
                self.colors = Some(colors);
            }
        }
        self.colors.unwrap_or(Accents::FALLBACK)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const CHOICES: [(Self, &'static str); 3] = [
        (Self::System, "跟随 Windows"),
        (Self::Light, "浅色"),
        (Self::Dark, "深色"),
    ];

    pub fn dark(self, appearance: WindowAppearance) -> bool {
        match self {
            Self::System => matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            Self::Light => false,
            Self::Dark => true,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

thread_local! {
    static DARK: Cell<bool> = const { Cell::new(true) };
    static MICA: Cell<bool> = const { Cell::new(false) };
    static ACCENTS: Cell<Accents> = const { Cell::new(Accents::FALLBACK) };
}

pub fn apply(theme: Theme, window: &Window) {
    let dark = theme.dark(window.appearance());
    DARK.set(dark);
    #[cfg(windows)]
    {
        ACCENTS.set(windows::accents(window.appearance()));
        MICA.set(update_window_chrome(window, dark));
    }
}

pub fn palette() -> Palette {
    Palette::for_mode(DARK.get(), MICA.get(), ACCENTS.get())
}

pub fn is_dark() -> bool {
    DARK.get()
}

#[cfg(windows)]
fn update_window_chrome(window: &Window, dark: bool) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    thread_local! {
        static CHROME: Cell<Option<(isize, bool, bool)>> = const { Cell::new(None) };
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = handle.hwnd.get();
    if let Some((cached_hwnd, cached_dark, cached_mica)) = CHROME.get()
        && cached_hwnd == hwnd
        && cached_dark == dark
    {
        return cached_mica;
    }

    #[repr(C)]
    struct Margins {
        left: i32,
        right: i32,
        top: i32,
        bottom: i32,
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut std::ffi::c_void,
            attribute: u32,
            value: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn DwmExtendFrameIntoClientArea(
            hwnd: *mut std::ffi::c_void,
            margins: *const Margins,
        ) -> i32;
    }

    let hwnd_ptr = hwnd as *mut std::ffi::c_void;
    let enabled = i32::from(dark);
    // DWMWA_USE_IMMERSIVE_DARK_MODE (20) sets dark titlebar/Mica tinting.
    // DWMWA_WINDOW_CORNER_PREFERENCE (33) = DWMWCP_ROUND (2) ensures Windows 11 rounded corners.
    // DWMWA_SYSTEMBACKDROP_TYPE (38) = DWMSBT_MAINWINDOW (2) enables native Windows 11 Mica.
    let corner_round = 2i32;
    let backdrop_mica = 2i32;
    let margins = Margins {
        left: -1,
        right: -1,
        top: -1,
        bottom: -1,
    };
    let mica = unsafe {
        DwmSetWindowAttribute(hwnd_ptr, 20, (&enabled as *const i32).cast(), 4);
        DwmSetWindowAttribute(hwnd_ptr, 33, (&corner_round as *const i32).cast(), 4);
        let extend_ok = DwmExtendFrameIntoClientArea(hwnd_ptr, &margins) >= 0;
        let backdrop_ok =
            DwmSetWindowAttribute(hwnd_ptr, 38, (&backdrop_mica as *const i32).cast(), 4) >= 0;
        extend_ok && backdrop_ok
    };
    CHROME.set(Some((hwnd, dark, mica)));
    mica
}

fn resolve(value: u32, dark: bool, accents: Accents) -> u32 {
    if value == ACCENT {
        return accents.text(dark);
    }
    if dark {
        return value;
    }
    match value {
        SHELL => 0xf3f3f3,
        BG => 0xf8f8f8,
        PANEL => 0xffffff,
        SELECTED => 0xe6e6e6,
        BORDER => 0xe0e0e0,
        TEXT => 0x1a1a1a,
        MUTED => 0x595959,
        DISABLED => 0x757575,
        // Selection fill in the native input painter.
        0x495e70 => 0xcce4f7,
        _ => value,
    }
}

pub fn rgb(value: u32) -> Rgba {
    gpui::rgb(resolve(value, DARK.get(), ACCENTS.get()))
}

/// Direct raw RGB conversion without dark->light remapping (for Palette fields).
pub fn hex(value: u32) -> Rgba {
    gpui::rgb(value)
}

fn relative_luminance(value: u32) -> f64 {
    let linear = |shift: u32| {
        let channel = ((value >> shift) & 255u32) as f64 / 255.;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_theme_overrides_system_and_system_follows_changes() {
        assert!(!Theme::Light.dark(WindowAppearance::Dark));
        assert!(Theme::Dark.dark(WindowAppearance::Light));
        assert!(!Theme::System.dark(WindowAppearance::Light));
        assert!(Theme::System.dark(WindowAppearance::VibrantDark));
        for (choice, _) in Theme::CHOICES {
            assert!(Theme::parse(choice.key()) == choice);
        }
        assert!(Theme::parse("future-setting") == Theme::System);
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let la = relative_luminance(a);
        let lb = relative_luminance(b);
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn system_shades_keep_text_and_fill_roles_for_both_explicit_themes() {
        let pink = Accents {
            light_text: 0x8a0061,
            dark_text: 0xf9c0ed,
            light_fill: 0xb4007e,
            dark_fill: 0xf38cd2,
        };
        assert_eq!(resolve(ACCENT, false, pink), pink.light_text);
        assert_eq!(resolve(ACCENT, true, pink), pink.dark_text);
        assert_eq!(pink.fill(false), pink.light_fill);
        assert_eq!(pink.fill(true), pink.dark_fill);
        for dark in [false, true] {
            for background in [BG, PANEL, SHELL, SELECTED] {
                assert!(
                    contrast(resolve(background, dark, pink), resolve(ACCENT, dark, pink)) >= 4.5
                );
            }
            let pal = Palette::for_mode(dark, false, pink);
            assert!(contrast(pal.accent_fill, pal.on_accent) >= 4.5);
        }
    }

    #[test]
    fn accent_cache_reads_once_per_change_and_retains_fallback_or_last_success() {
        let mut cache = AccentCache::default();
        let reads = Cell::new(0);
        assert_eq!(
            cache.refresh(0, || {
                reads.set(reads.get() + 1);
                None
            }),
            Accents::FALLBACK
        );
        for _ in 0..10_000 {
            assert_eq!(
                cache.refresh(0, || panic!("unchanged palette must stay cached")),
                Accents::FALLBACK
            );
        }
        let pink = Accents {
            dark_text: 0xf9c0ed,
            ..Accents::FALLBACK
        };
        assert_eq!(
            cache.refresh(1, || {
                reads.set(reads.get() + 1);
                Some(pink)
            }),
            pink
        );
        assert_eq!(
            cache.refresh(2, || {
                reads.set(reads.get() + 1);
                None
            }),
            pink
        );
        assert_eq!(
            cache.refresh(2, || panic!("a failure must stay cached too")),
            pink
        );
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn both_palettes_keep_body_secondary_and_accent_text_readable() {
        for dark in [false, true] {
            for background in [BG, PANEL, SHELL, SELECTED] {
                for foreground in [TEXT, MUTED, ACCENT] {
                    let ratio = contrast(
                        resolve(background, dark, Accents::FALLBACK),
                        resolve(foreground, dark, Accents::FALLBACK),
                    );
                    assert!(
                        ratio >= 4.5,
                        "insufficient text contrast ({ratio:.2}): dark={dark}, {foreground:x}/{background:x}"
                    );
                }
            }
            let pal = Palette::for_mode(dark, false, Accents::FALLBACK);
            for bg in [pal.shell, pal.layer, pal.card, pal.selected] {
                for fg in [
                    pal.text,
                    pal.text_secondary,
                    pal.text_muted,
                    pal.accent_text,
                    pal.success,
                    pal.warning,
                    pal.danger,
                    pal.info,
                    pal.syntax_digit,
                    pal.syntax_symbol,
                ] {
                    let ratio = contrast(bg, fg);
                    assert!(
                        ratio >= 4.5,
                        "semantic contrast ({ratio:.2}) < 4.5: dark={dark}, fg={fg:06x} on bg={bg:06x}"
                    );
                }
            }
            assert!(contrast(pal.accent_fill, pal.on_accent) >= 4.5);
        }
    }

    #[test]
    fn soften_accent_tames_vivid_colors_and_preserves_contrast() {
        let vivid_cases = [
            Accents {
                light_text: 0xb0006d,
                dark_text: 0xff66cc,
                light_fill: 0xff00aa,
                dark_fill: 0xff33bb,
            },
            Accents {
                light_text: 0x006677,
                dark_text: 0x00ffff,
                light_fill: 0x00e5ff,
                dark_fill: 0x00ffff,
            },
            Accents {
                light_text: 0x007722,
                dark_text: 0x00ff44,
                light_fill: 0x00e639,
                dark_fill: 0x33ff66,
            },
        ];
        for accents in vivid_cases {
            for dark in [false, true] {
                let pal = Palette::for_mode(dark, false, accents);
                let (_, raw_s, _) = rgb_to_hsl(accents.fill(dark));
                let (_, soft_s, _) = rgb_to_hsl(pal.accent_fill);
                assert!(
                    soft_s < raw_s,
                    "expected softened fill saturation ({soft_s:.2}) < raw ({raw_s:.2})"
                );
                assert!(
                    contrast(pal.accent_fill, pal.on_accent) >= 4.5,
                    "softened fill contrast failed: dark={dark}, fill={:06x}, on={:06x}",
                    pal.accent_fill,
                    pal.on_accent
                );
                for bg in [pal.shell, pal.layer, pal.card, pal.selected] {
                    assert!(
                        contrast(bg, pal.accent_text) >= 4.5,
                        "softened text contrast failed: dark={dark}, text={:06x} on bg={bg:06x}",
                        pal.accent_text
                    );
                }
            }
        }
    }
}
