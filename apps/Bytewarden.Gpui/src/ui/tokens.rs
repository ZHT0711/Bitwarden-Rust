//! Shared Fluent 2 design tokens for Bytewarden GPUI.
//! Keeping spacing, radii, typography, icon glyphs, and sizes centralized
//! ensures visual consistency and prevents magic numbers across views.

#[allow(dead_code)]
pub mod space {
    pub const XS: f32 = 4.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 24.;
    pub const XXL: f32 = 32.;
}

#[allow(dead_code)]
pub mod radius {
    pub const SM: f32 = 4.;
    pub const CONTROL: f32 = 6.;
    pub const CARD: f32 = 10.;
    pub const OVERLAY: f32 = 12.;
    pub const PILL: f32 = 999.;
}

#[allow(dead_code)]
pub mod font {
    pub const MICRO: f32 = 11.;
    pub const CAPTION: f32 = 12.;
    pub const BODY: f32 = 14.;
    pub const BODY_LG: f32 = 16.;
    pub const SUBTITLE: f32 = 20.;
    pub const TITLE: f32 = 26.;
    pub const TOTP: f32 = 22.;
}

#[allow(dead_code)]
pub mod size {
    pub const TITLEBAR: f32 = 44.;
    pub const CONTROL: f32 = 32.;
    pub const CONTROL_SM: f32 = 26.;
    pub const ICON_BTN: f32 = 30.;
    pub const ROW: f32 = 58.;
    pub const FAVICON: f32 = 32.;
    pub const HERO_ICON: f32 = 48.;
    pub const LIST_DEFAULT: f32 = 340.;
    pub const LIST_MIN: f32 = 260.;
    pub const LIST_MAX: f32 = 520.;
    pub const DETAIL_MAX: f32 = 760.;
    pub const WINDOW_DEFAULT_W: f32 = 1260.;
    pub const WINDOW_DEFAULT_H: f32 = 780.;
}

pub mod family {
    pub const UI: &str = "Segoe UI Variable Text";
    pub const MONO: &str = "Cascadia Mono";
    pub const ICONS: &str = "Segoe Fluent Icons";
}

/// Segoe Fluent Icons glyphs used across Bytewarden GPUI.
#[allow(dead_code)]
pub mod glyph {
    pub const MENU: &str = "\u{e700}";
    pub const BACK: &str = "\u{e72b}";
    pub const CLOSE: &str = "\u{e711}";
    pub const CHROME_MIN: &str = "\u{e921}";
    pub const CHROME_MAX: &str = "\u{e922}";
    pub const CHROME_RESTORE: &str = "\u{e923}";
    pub const CHROME_CLOSE: &str = "\u{e8bb}";
    pub const SEARCH: &str = "\u{e721}";
    pub const ADD: &str = "\u{e710}";
    pub const EDIT: &str = "\u{e70f}";
    pub const DELETE: &str = "\u{e74d}";
    pub const SYNC: &str = "\u{e895}";
    pub const LOCK: &str = "\u{e72e}";
    pub const UNLOCK: &str = "\u{e785}";
    pub const SHIELD: &str = "\u{ea18}";
    pub const CHECK: &str = "\u{e73e}";
    pub const CHECK_BOX: &str = "\u{e73a}";
    pub const COPY: &str = "\u{e8c8}";
    pub const EYE: &str = "\u{e7b3}";
    pub const EYE_OFF: &str = "\u{ed1a}";
    pub const OPEN_EXTERNAL: &str = "\u{e8a7}";
    pub const STAR: &str = "\u{e734}";
    pub const STAR_SOLID: &str = "\u{e735}";
    pub const FOLDER: &str = "\u{e8b7}";
    pub const FOLDER_OPEN: &str = "\u{e838}";
    pub const CHEVRON_RIGHT: &str = "\u{e76c}";
    pub const CHEVRON_DOWN: &str = "\u{e70d}";
    pub const SORT: &str = "\u{e8cb}";
    pub const MORE: &str = "\u{e712}";
    pub const SEND: &str = "\u{e724}";
    pub const KEY: &str = "\u{e8d7}";
    pub const SETTINGS: &str = "\u{e713}";
    pub const INFO: &str = "\u{e946}";
    pub const WARNING: &str = "\u{e7ba}";
    pub const ERROR: &str = "\u{ea39}";
    pub const ARCHIVE: &str = "\u{e7b8}";
    pub const RESTORE: &str = "\u{e777}";
    pub const HOME: &str = "\u{e80f}";
    pub const PERSON: &str = "\u{e77b}";
    pub const GLOBE: &str = "\u{e774}";
    pub const ATTACH: &str = "\u{e723}";
    pub const UPLOAD: &str = "\u{e898}";
    pub const DOWNLOAD: &str = "\u{e896}";
    pub const CLOCK: &str = "\u{e823}";
    pub const HISTORY: &str = "\u{e81c}";
    pub const SPARKLE: &str = "\u{e734}";
    pub const CLOUD_OFF: &str = "\u{e753}";
    pub const KEYBOARD: &str = "\u{e765}";
    pub const PALETTE: &str = "\u{e790}";
    pub const EXPORT: &str = "\u{e8b5}";
    pub const DICE: &str = "\u{e895}";
    pub const PASSKEY: &str = "\u{e928}";
    pub const BUILDING: &str = "\u{e731}";
}

/// Deterministic vibrant hues for monogram avatars and item badges.
const MONOGRAM_COLORS: &[u32] = &[
    0x2b6cb0, // Cobalt blue
    0x2c7a7b, // Deep teal
    0x6b46c1, // Royal violet
    0xb7791f, // Warm amber
    0xc53030, // Crimson
    0x2f855a, // Emerald
    0xb83280, // Magenta rose
    0x4c51bf, // Indigo
];

/// Pick a consistent, pleasant badge color from any non-empty label.
pub fn monogram_color(seed: &str) -> u32 {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ u32::from(byte)
    });
    MONOGRAM_COLORS[(hash as usize) % MONOGRAM_COLORS.len()]
}

/// Extract a clean 1-character uppercase monogram for avatars and items without favicons.
pub fn monogram_letter(label: &str) -> String {
    label
        .trim()
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .or_else(|| label.trim().chars().next())
        .map_or_else(|| "B".into(), |ch| ch.to_uppercase().to_string())
}
