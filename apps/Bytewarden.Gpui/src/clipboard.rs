//! Windows clipboard transaction for explicit copies of vault and generator output.
//! Text is published only after the History and cloud formats both advertise zero.

#[cfg(windows)]
mod native {
    use std::{mem, ptr, slice};
    use uuid::Uuid;
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HGLOBAL, HWND},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        },
        UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE},
    };
    use zeroize::{Zeroize, Zeroizing};

    const CF_UNICODETEXT: u32 = 13;

    struct Access;
    impl Access {
        fn open(window: HWND) -> Option<Self> {
            // A private message-only window gives the lock a process-owned HWND.
            (!window.is_null() && unsafe { OpenClipboard(window) } != 0).then_some(Self)
        }
    }
    impl Drop for Access {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    struct Block(HGLOBAL, usize);
    impl Block {
        fn from_bytes(bytes: &[u8]) -> Option<Self> {
            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
            if handle.is_null() {
                return None;
            }
            let block = Self(handle, bytes.len());
            let location = unsafe { GlobalLock(handle) };
            if location.is_null() {
                return None;
            }
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), location.cast(), bytes.len());
                GlobalUnlock(handle);
            }
            Some(block)
        }
        fn publish(mut self, format: u32) -> bool {
            if unsafe { SetClipboardData(format, self.0) }.is_null() {
                return false;
            }
            self.0 = ptr::null_mut();
            true
        }
    }
    impl Drop for Block {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let pointer = GlobalLock(self.0);
                    if !pointer.is_null() {
                        slice::from_raw_parts_mut(pointer.cast::<u8>(), self.1).zeroize();
                        GlobalUnlock(self.0);
                    }
                    GlobalFree(self.0);
                }
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([0]).collect()
    }
    fn format(name: &str) -> u32 {
        unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
    }

    #[derive(Clone, Copy)]
    struct Token {
        sequence: u32,
        owner: Uuid,
    }

    #[derive(Default)]
    pub struct Clipboard {
        window: HWND,
        token: Option<Token>,
    }

    impl Clipboard {
        fn ensure_window(&mut self) -> HWND {
            if self.window.is_null() {
                self.window = unsafe {
                    CreateWindowExW(
                        0,
                        wide("STATIC").as_ptr(),
                        ptr::null(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        ptr::null(),
                    )
                };
            }
            self.window
        }

        pub fn copy(&mut self, text: &str) -> bool {
            if text.is_empty() || text.contains('\0') {
                return false;
            }
            let history = format("CanIncludeInClipboardHistory");
            let cloud = format("CanUploadToCloudClipboard");
            let marker = format("Bytewarden.GPUI.ClipboardOwner");
            if history == 0 || cloud == 0 || marker == 0 {
                return false;
            }
            let owner = Uuid::new_v4();
            let wide = Zeroizing::new(wide(text));
            let unicode = unsafe {
                slice::from_raw_parts(wide.as_ptr().cast(), std::mem::size_of_val(wide.as_slice()))
            };
            // Allocate every format before touching existing clipboard contents.
            // A failed allocation or an unavailable clipboard leaves the previous
            // application value intact. Text is always published last.
            let Some(blocks) = [
                (history, &[0u8; 4] as &[u8]),
                (cloud, &[0; 4]),
                (marker, owner.as_bytes()),
                (CF_UNICODETEXT, unicode),
            ]
            .into_iter()
            .map(|(format, bytes)| Block::from_bytes(bytes).map(|block| (format, block)))
            .collect::<Option<Vec<_>>>() else {
                return false;
            };
            let Some(_access) = Access::open(self.ensure_window()) else {
                return false;
            };
            if unsafe { EmptyClipboard() } == 0 {
                return false;
            }
            self.token = None;
            let ok = blocks
                .into_iter()
                .all(|(format, block)| block.publish(format));
            if !ok {
                unsafe { EmptyClipboard() };
                return false;
            }
            self.token = Some(Token {
                sequence: unsafe { GetClipboardSequenceNumber() },
                owner,
            });
            true
        }

        /// Returns true when this app's copy was cleared or replaced. A busy
        /// clipboard leaves the token intact for a later attempt.
        pub fn clear(&mut self) -> bool {
            let Some(token) = self.token else {
                return true;
            };
            let Some(_access) = Access::open(self.window) else {
                return false;
            };
            if unsafe { GetClipboardSequenceNumber() } == token.sequence {
                let Some(owned) = self.marker_matches(token.owner) else {
                    return false;
                };
                if owned && unsafe { EmptyClipboard() } == 0 {
                    return false;
                }
            }
            self.token = None;
            true
        }

        fn marker_matches(&self, owner: Uuid) -> Option<bool> {
            let marker = format("Bytewarden.GPUI.ClipboardOwner");
            if marker == 0 {
                return None;
            }
            let handle = unsafe { GetClipboardData(marker) };
            if handle.is_null() || unsafe { GlobalSize(handle) } < mem::size_of::<Uuid>() {
                return None;
            }
            let pointer = unsafe { GlobalLock(handle) };
            if pointer.is_null() {
                return None;
            }
            let matches =
                unsafe { slice::from_raw_parts(pointer.cast::<u8>(), 16) } == owner.as_bytes();
            unsafe { GlobalUnlock(handle) };
            Some(matches)
        }
    }

    #[cfg(test)]
    include!("clipboard_tests.rs");

    impl Drop for Clipboard {
        fn drop(&mut self) {
            self.clear();
            if !self.window.is_null() {
                unsafe { DestroyWindow(self.window) };
            }
        }
    }
}

#[cfg(windows)]
pub use native::Clipboard;

#[cfg(not(windows))]
#[derive(Default)]
pub struct Clipboard;
#[cfg(not(windows))]
impl Clipboard {
    pub fn copy(&mut self, _: &str) -> bool {
        false
    }
    pub fn clear(&mut self) -> bool {
        true
    }
}
