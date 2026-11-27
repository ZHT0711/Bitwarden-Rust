//! Lightweight on-demand TOTP QR code decoder for Windows clipboard screenshots
//! (`Win+Shift+S`, browser Copy Image) and local image files.
//! All pixel buffers are bounded, processed on a background thread, and dropped immediately.

use std::{io::Cursor, path::Path};

use image::DynamicImage;
use zeroize::Zeroizing;

const MAX_IMAGE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_QR_DIMENSION: u32 = 1600;

/// Decode a TOTP URI from an image file on disk.
pub fn decode_totp_from_file(path: &Path) -> Result<Zeroizing<String>, String> {
    let meta = std::fs::metadata(path).map_err(|_| "无法读取所选图片文件。".to_owned())?;
    if meta.len() > MAX_IMAGE_FILE_BYTES {
        return Err("图片文件过大（上限 16 MB），请裁剪二维码区域后重试。".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|_| "无法读取所选图片文件。".to_owned())?;
    decode_totp_from_image_bytes(&bytes)
}

/// Decode a TOTP URI from a screenshot/image currently stored in the Windows clipboard.
pub fn decode_totp_from_clipboard() -> Result<Zeroizing<String>, String> {
    let bytes = read_clipboard_image_bytes()?;
    decode_totp_from_image_bytes(&bytes)
}

fn decode_totp_from_image_bytes(bytes: &[u8]) -> Result<Zeroizing<String>, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "无法识别图片格式，请使用 PNG / JPG / BMP / WebP 图片。".to_owned())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|_| "图片解码失败，请确认文件未损坏。".to_owned())?;
    decode_totp_from_dynamic_image(img)
}

fn decode_totp_from_dynamic_image(img: DynamicImage) -> Result<Zeroizing<String>, String> {
    let resized = if img.width() > MAX_QR_DIMENSION || img.height() > MAX_QR_DIMENSION {
        img.thumbnail(MAX_QR_DIMENSION, MAX_QR_DIMENSION)
    } else {
        img
    };
    let luma = resized.into_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(luma);
    let grids = prepared.detect_grids();
    if grids.is_empty() {
        return Err("未在图片中检测到二维码，请截取清晰的二维码区域。".to_owned());
    }
    for grid in grids {
        if let Ok((_, content)) = grid.decode()
            && let Some(uri) = extract_totp_uri(&content)
        {
            return Ok(Zeroizing::new(uri));
        }
    }
    Err("识别到了二维码，但不是有效的 TOTP 验证器配置（需为 otpauth://totp/...）。".to_owned())
}

/// Validate and normalize an `otpauth://totp/...` (or `steam://`) QR payload.
pub fn extract_totp_uri(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("otpauth://totp/") {
        let query = trimmed.split_once('?')?.1;
        let has_secret = query.split('&').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(k, v)| k.eq_ignore_ascii_case("secret") && !v.trim().is_empty())
        });
        if has_secret {
            return Some(trimmed.to_owned());
        }
        return None;
    }
    if lower.starts_with("steam://") && trimmed.len() > 8 {
        return Some(trimmed.to_owned());
    }
    None
}

#[cfg(windows)]
fn read_clipboard_image_bytes() -> Result<Vec<u8>, String> {
    use std::{ffi::c_void, ptr::null_mut};

    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(hmem: *mut c_void) -> *const u8;
        fn GlobalUnlock(hmem: *mut c_void) -> i32;
        fn GlobalSize(hmem: *mut c_void) -> usize;
    }

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    if unsafe { OpenClipboard(null_mut()) } == 0 {
        return Err("剪贴板正被其他程序占用，请稍后重试。".to_owned());
    }
    let _guard = ClipboardGuard;

    // 1. Prefer raw PNG clipboard format if present (e.g. modern Snipping Tool / browsers).
    let png_name: Vec<u16> = "PNG\0".encode_utf16().collect();
    let cf_png = unsafe { RegisterClipboardFormatW(png_name.as_ptr()) };
    if cf_png != 0 {
        let handle = unsafe { GetClipboardData(cf_png) };
        if !handle.is_null() {
            let size = unsafe { GlobalSize(handle) };
            if size > 16 && size <= MAX_IMAGE_FILE_BYTES as usize {
                let ptr = unsafe { GlobalLock(handle) };
                if !ptr.is_null() {
                    let bytes = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
                    unsafe { GlobalUnlock(handle) };
                    return Ok(bytes);
                }
            }
        }
    }

    // 2. Fallback to standard Windows DIB / DIBV5 bitmap by prepending a 14-byte BMP file header.
    for format in [CF_DIB, CF_DIBV5] {
        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            continue;
        }
        let dib_size = unsafe { GlobalSize(handle) };
        if dib_size < 40 || dib_size > MAX_IMAGE_FILE_BYTES as usize {
            continue;
        }
        let ptr = unsafe { GlobalLock(handle) };
        if ptr.is_null() {
            continue;
        }
        let dib = unsafe { std::slice::from_raw_parts(ptr, dib_size) };
        let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
        let bpp = u16::from_le_bytes([dib[14], dib[15]]);
        let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
        let colors_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
        let palette_bytes = if bpp <= 8 {
            let count = if colors_used > 0 {
                colors_used
            } else {
                1usize << bpp
            };
            count * 4
        } else if compression == 3 && header_size == 40 {
            12
        } else {
            colors_used * 4
        };
        let pixel_offset = (14 + header_size + palette_bytes) as u32;
        let file_size = (14 + dib_size) as u32;

        let mut bmp = Vec::with_capacity(14 + dib_size);
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&file_size.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&pixel_offset.to_le_bytes());
        bmp.extend_from_slice(dib);
        unsafe { GlobalUnlock(handle) };
        return Ok(bmp);
    }

    Err("剪贴板中没有图片。请先使用 Win+Shift+S 截取二维码后再点击识别。".to_owned())
}

#[cfg(not(windows))]
fn read_clipboard_image_bytes() -> Result<Vec<u8>, String> {
    Err("当前平台暂不支持从剪贴板读取图片。".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_totp_uri_validates_scheme_and_secret() {
        assert_eq!(
            extract_totp_uri(
                "  otpauth://totp/GitHub:alice?secret=JBSWY3DPEHPK3PXP&issuer=GitHub  "
            )
            .as_deref(),
            Some("otpauth://totp/GitHub:alice?secret=JBSWY3DPEHPK3PXP&issuer=GitHub")
        );
        assert!(extract_totp_uri("https://example.com").is_none());
        assert!(extract_totp_uri("otpauth://totp/NoSecret?issuer=Test").is_none());
        assert!(extract_totp_uri("otpauth://totp/EmptySecret?secret=").is_none());
        assert!(extract_totp_uri("steam://JBSWY3DPEHPK3PXP").is_some());
    }
}
