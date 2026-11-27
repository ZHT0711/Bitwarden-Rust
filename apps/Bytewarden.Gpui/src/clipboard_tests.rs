// Kept inside clipboard::native so the native ownership transaction is exercised.
mod tests {
    use super::*;
    use std::sync::mpsc;
    use windows_sys::Win32::System::{StationsAndDesktops::*, Threading::GetCurrentThreadId};

    struct IsolatedStation {
        original_station: HWINSTA,
        original_desktop: HDESK,
        station: HWINSTA,
        desktop: HDESK,
    }
    impl IsolatedStation {
        fn enter() -> Self {
            // Never touch WinSta0 (the user's clipboard). This test is run alone
            // in its own harness process because the window station is process-wide.
            assert_eq!(
                std::env::var("BYTEWARDEN_ISOLATED_CLIPBOARD_TEST").as_deref(),
                Ok("1")
            );
            unsafe {
                let mut state = Self {
                    original_station: GetProcessWindowStation(),
                    original_desktop: GetThreadDesktop(GetCurrentThreadId()),
                    station: ptr::null_mut(),
                    desktop: ptr::null_mut(),
                };
                state.station = CreateWindowStationW(
                    wide(&format!("BytewardenClipboardTest-{}", Uuid::new_v4())).as_ptr(),
                    0,
                    0x2f,
                    ptr::null(), // Enumerate/read, clipboard, desktop creation and atoms only.
                );
                assert!(
                    !state.station.is_null(),
                    "create isolated station: {}",
                    std::io::Error::last_os_error()
                );
                assert_ne!(state.station, state.original_station);
                assert_ne!(SetProcessWindowStation(state.station), 0);
                state.desktop = CreateDesktopW(
                    wide("test").as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    DESKTOP_CREATEWINDOW | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS,
                    ptr::null(),
                );
                assert!(
                    !state.desktop.is_null(),
                    "create isolated desktop: {}",
                    std::io::Error::last_os_error()
                );
                assert_ne!(SetThreadDesktop(state.desktop), 0);
                state
            }
        }
    }
    impl Drop for IsolatedStation {
        fn drop(&mut self) {
            unsafe {
                SetThreadDesktop(self.original_desktop);
                SetProcessWindowStation(self.original_station);
                if !self.desktop.is_null() {
                    CloseDesktop(self.desktop);
                }
                if !self.station.is_null() {
                    CloseWindowStation(self.station);
                }
            }
        }
    }

    fn bytes(format_id: u32) -> Vec<u8> {
        unsafe {
            let handle = GetClipboardData(format_id);
            assert!(!handle.is_null());
            let length = GlobalSize(handle);
            let pointer = GlobalLock(handle);
            assert!(!pointer.is_null());
            let result = slice::from_raw_parts(pointer.cast::<u8>(), length).to_vec();
            GlobalUnlock(handle);
            result
        }
    }

    fn text(clipboard: &Clipboard) -> String {
        let _access = Access::open(clipboard.window).expect("test clipboard access");
        let value = bytes(CF_UNICODETEXT);
        let utf16 = value
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_ne_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        String::from_utf16(&utf16).unwrap()
    }

    #[test]
    #[ignore = "requires a dedicated process with BYTEWARDEN_ISOLATED_CLIPBOARD_TEST=1; uses a private Windows station"]
    fn isolated_windows_clipboard_transaction() {
        let station = IsolatedStation::enter();
        let mut owner = Clipboard::default();
        assert!(owner.copy("public synthetic secret 甲😀"));
        assert_eq!(text(&owner), "public synthetic secret 甲😀");
        {
            let _access = Access::open(owner.window).unwrap();
            for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
                assert_eq!(&bytes(format(name))[..4], [0; 4]);
            }
            assert_eq!(owner.marker_matches(owner.token.unwrap().owner), Some(true));
        }
        // A second thread really holds the native clipboard lock. Failed clear
        // retains its token; failed copy must leave the existing text untouched.
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let desktop = station.desktop as usize;
        let holder = std::thread::spawn(move || {
            unsafe {
                assert_ne!(SetThreadDesktop(desktop as HDESK), 0);
                assert_ne!(OpenClipboard(ptr::null_mut()), 0);
            }
            let _access = Access;
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv().unwrap();
        assert!(!owner.clear());
        assert!(owner.token.is_some());
        assert!(!owner.copy("unpublished replacement"));
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(text(&owner), "public synthetic secret 甲😀");
        assert!(owner.clear());
        assert!(owner.token.is_none());
        {
            let _access = Access::open(owner.window).unwrap();
            assert!(unsafe { GetClipboardData(CF_UNICODETEXT) }.is_null());
        }
        assert!(owner.copy("public owned secret"));
        let mut replacement = Clipboard::default();
        assert!(replacement.copy("benign external replacement"));
        assert!(owner.clear());
        assert_eq!(text(&replacement), "benign external replacement");
        assert!(!replacement.copy("invalid\0input"));
        assert!(!replacement.copy(""));
        assert_eq!(text(&replacement), "benign external replacement");
        assert!(replacement.clear());
        // Both native owner windows are destroyed before the isolated desktop.
        drop(replacement);
        drop(owner);
        drop(station);
    }
}
