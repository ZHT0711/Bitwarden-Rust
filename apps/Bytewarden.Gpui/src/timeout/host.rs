//! Windows subscriptions are scoped to the GPUI window. They only enqueue work;
//! GPUI is never entered synchronously from a native window procedure.
use super::{Capabilities, Event};

#[cfg(windows)]
mod native {
    use super::{Capabilities, Event};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::Cell,
        ffi::c_void,
        panic::{AssertUnwindSafe, catch_unwind},
        ptr,
        rc::Rc,
    };

    type Hwnd = *mut c_void;
    type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;
    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn SetWindowSubclass(
            window: Hwnd,
            proc: Option<SubclassProc>,
            id: usize,
            reference: usize,
        ) -> i32;
        fn RemoveWindowSubclass(window: Hwnd, proc: Option<SubclassProc>, id: usize) -> i32;
        fn DefSubclassProc(window: Hwnd, message: u32, parameter: usize, data: isize) -> isize;
    }
    #[link(name = "wtsapi32")]
    unsafe extern "system" {
        fn WTSRegisterSessionNotification(window: Hwnd, flags: u32) -> i32;
        fn WTSUnRegisterSessionNotification(window: Hwnd) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterSuspendResumeNotification(window: Hwnd, flags: u32) -> *mut c_void;
        fn UnregisterSuspendResumeNotification(registration: *mut c_void) -> i32;
        fn GetLastInputInfo(info: *mut LastInput) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTickCount64() -> u64;
        fn GetTickCount() -> u32;
    }
    #[repr(C)]
    struct LastInput {
        size: u32,
        time: u32,
    }
    pub fn now() -> u64 {
        unsafe { GetTickCount64() }
    }
    pub fn idle_millis() -> Option<u64> {
        let mut input = LastInput {
            size: std::mem::size_of::<LastInput>() as u32,
            time: 0,
        };
        (unsafe { GetLastInputInfo(&mut input) } != 0)
            .then(|| u64::from(unsafe { GetTickCount() }.wrapping_sub(input.time)))
    }
    struct State {
        attached: Cell<bool>,
        callback: Rc<dyn Fn(Event)>,
    }
    pub struct Host {
        window: Hwnd,
        state: Option<Box<State>>,
        lock: bool,
        power: *mut c_void,
    }
    fn subclass_id() -> usize {
        window_proc as *const () as usize
    }
    unsafe extern "system" fn window_proc(
        window: Hwnd,
        message: u32,
        parameter: usize,
        data: isize,
        _: usize,
        reference: usize,
    ) -> isize {
        // No panic may unwind across the OS boundary, including executor shutdown.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let state = unsafe { &*(reference as *const State) };
            let event = match (message, parameter) {
                (0x02B1, 0x07) => Some(Event::Lock), // WM_WTSSESSION_CHANGE / WTS_SESSION_LOCK
                (0x0218, 0x04 | 0x12) => Some(Event::Sleep), // suspend and automatic resume
                _ => None,
            };
            if let Some(event) = event {
                (state.callback)(event);
            }
            if message == 0x0082 {
                // WM_NCDESTROY: invalidate before the handle can be reused.
                state.attached.set(false);
                unsafe {
                    RemoveWindowSubclass(window, Some(window_proc), subclass_id());
                }
            }
        }));
        // comctl32 retains and invokes the existing subclass/window-procedure chain.
        unsafe { DefSubclassProc(window, message, parameter, data) }
    }
    impl Host {
        pub fn new(window: &Window, callback: impl Fn(Event) + 'static) -> Option<Self> {
            let RawWindowHandle::Win32(handle) =
                HasWindowHandle::window_handle(window).ok()?.as_raw()
            else {
                return None;
            };
            let window = handle.hwnd.get() as Hwnd;
            let state = Box::new(State {
                attached: Cell::new(false),
                callback: Rc::new(callback),
            });
            let reference = (&*state as *const State) as usize;
            if unsafe { SetWindowSubclass(window, Some(window_proc), subclass_id(), reference) }
                == 0
            {
                return None;
            }
            state.attached.set(true);
            Some(Self {
                window,
                state: Some(state),
                lock: unsafe { WTSRegisterSessionNotification(window, 0) } != 0,
                power: unsafe { RegisterSuspendResumeNotification(window, 0) },
            })
        }
        pub fn capabilities(&self) -> Capabilities {
            Capabilities {
                lock: self.lock,
                sleep: !self.power.is_null(),
                idle: true,
            }
        }
    }
    impl Drop for Host {
        fn drop(&mut self) {
            if !self.power.is_null() {
                unsafe {
                    UnregisterSuspendResumeNotification(self.power);
                }
                self.power = ptr::null_mut();
            }
            if let Some(state) = self.state.take()
                && state.attached.replace(false)
            {
                if self.lock {
                    unsafe {
                        WTSUnRegisterSessionNotification(self.window);
                    }
                }
                if unsafe { RemoveWindowSubclass(self.window, Some(window_proc), subclass_id()) }
                    == 0
                {
                    // A retained OS callback must never point to freed state.
                    // Fail safely on unexpected removal failure (normally only
                    // possible if another owner changed the subclass chain).
                    Box::leak(state);
                }
            }
        }
    }
}
#[cfg(windows)]
pub use native::{Host, idle_millis, now};

#[cfg(not(windows))]
pub struct Host;
#[cfg(not(windows))]
impl Host {
    pub fn new(_: &gpui::Window, _: impl Fn(Event) + 'static) -> Option<Self> {
        None
    }
    pub fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}
#[cfg(not(windows))]
pub fn now() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
#[cfg(not(windows))]
pub fn idle_millis() -> Option<u64> {
    None
}
