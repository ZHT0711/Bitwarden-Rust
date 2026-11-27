//! Embedded Bytewarden brand assets reused directly from `apps/Bytewarden.WinUI/Assets`
//! so GPUI shares the exact same logo in titlebar, hero cards, taskbar, and Alt+Tab switcher.

use std::sync::{Arc, OnceLock};

use gpui::{Div, ObjectFit, RenderImage, Window, div, img, prelude::*, px};

use crate::theme::{self, hex};

use super::{
    components::glyph_icon,
    tokens::{glyph, radius},
};

const LOGO_DARK_PNG: &[u8] =
    include_bytes!("../../../Bytewarden.WinUI/Assets/Square150x150Logo.scale-200_theme-dark.png");
const LOGO_LIGHT_PNG: &[u8] =
    include_bytes!("../../../Bytewarden.WinUI/Assets/Square150x150Logo.scale-200_theme-light.png");

#[cfg(windows)]
const APP_ICON_ICO: &[u8] = include_bytes!("../../../Bytewarden.WinUI/Assets/AppIcon.ico");

static DARK_IMAGE: OnceLock<Option<Arc<RenderImage>>> = OnceLock::new();
static LIGHT_IMAGE: OnceLock<Option<Arc<RenderImage>>> = OnceLock::new();

fn decode_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    // GPUI DirectX/Metal render surfaces expect BGRA byte order.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

fn brand_image(dark: bool) -> Option<Arc<RenderImage>> {
    if dark {
        DARK_IMAGE.get_or_init(|| decode_png(LOGO_DARK_PNG)).clone()
    } else {
        LIGHT_IMAGE
            .get_or_init(|| decode_png(LOGO_LIGHT_PNG))
            .clone()
    }
}

/// Render the crisp Bytewarden Rust-gear + Shield brand mark at `logo_size` DIP,
/// automatically adapting to the active dark/light theme.
pub fn brand_logo(logo_size: f32) -> Div {
    let pal = theme::palette();
    if let Some(image) = brand_image(pal.dark) {
        div()
            .size(px(logo_size))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(image)
                    .size(px(logo_size))
                    .object_fit(ObjectFit::Contain),
            )
    } else {
        div()
            .size(px(logo_size))
            .rounded(px(radius::CONTROL))
            .bg(hex(pal.accent_fill))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_color(hex(pal.on_accent))
            .child(glyph_icon(glyph::SHIELD, (logo_size * 0.56).max(12.)))
    }
}

/// Apply the WinUI `AppIcon.ico` to the Windows top-level HWND (`WM_SETICON`)
/// so the taskbar, Alt+Tab switcher, and snap preview display the real Bytewarden icon.
#[cfg(windows)]
pub fn apply_window_icon(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as *mut c_void;
    if hwnd.is_null() {
        return;
    }

    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateIconFromResourceEx(
            bits: *const u8,
            size: u32,
            is_icon: i32,
            version: u32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> *mut c_void;
        fn SendMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    let select_entry = |desired: u16| -> Option<&[u8]> {
        let data = APP_ICON_ICO;
        if data.len() < 6
            || u16::from_le_bytes([data[0], data[1]]) != 0
            || u16::from_le_bytes([data[2], data[3]]) != 1
        {
            return None;
        }
        let count = usize::from(u16::from_le_bytes([data[4], data[5]]));
        let mut best: Option<(u16, &[u8])> = None;
        for i in 0..count {
            let entry_offset = 6 + i * 16;
            let entry = data.get(entry_offset..entry_offset + 16)?;
            let width = if entry[0] == 0 {
                256u16
            } else {
                u16::from(entry[0])
            };
            let size = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
            let slice = data.get(offset..offset.checked_add(size)?)?;
            if size == 0 {
                continue;
            }
            let diff = width.abs_diff(desired);
            if best.is_none_or(|(best_diff, _)| diff < best_diff) {
                best = Some((diff, slice));
            }
        }
        best.map(|(_, slice)| slice)
    };

    let desired_small = ((32.0 * window.scale_factor()).round() as u16).clamp(32, 128);
    unsafe {
        if let Some(small) = select_entry(desired_small) {
            let hicon = CreateIconFromResourceEx(
                small.as_ptr(),
                small.len() as u32,
                1,
                0x0003_0000,
                0,
                0,
                0,
            );
            if !hicon.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL, hicon as isize);
            }
        }
        if let Some(big) = select_entry(256) {
            let hicon =
                CreateIconFromResourceEx(big.as_ptr(), big.len() as u32, 1, 0x0003_0000, 0, 0, 0);
            if !hicon.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG, hicon as isize);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn apply_window_icon(_window: &Window) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_winui_brand_logos_decode_cleanly() {
        assert!(brand_image(true).is_some());
        assert!(brand_image(false).is_some());
    }
}
