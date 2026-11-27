#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseDecision {
    HideToTray,
    ShutDown,
    KeepOpen,
}

pub(crate) fn close_decision(
    keep_in_tray: bool,
    tray_registered: bool,
    exit_requested: bool,
    shutdown_started: bool,
) -> CloseDecision {
    if shutdown_started {
        CloseDecision::KeepOpen
    } else if exit_requested || !keep_in_tray || !tray_registered {
        CloseDecision::ShutDown
    } else {
        CloseDecision::HideToTray
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrayEvent {
    Restore,
    ContextMenu,
    Ignore,
}

fn tray_event(event: u16) -> TrayEvent {
    match event as u32 {
        NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => TrayEvent::Restore,
        WM_CONTEXTMENU => TrayEvent::ContextMenu,
        _ => TrayEvent::Ignore,
    }
}

const NIN_SELECT: u32 = 0x0400;
const NIN_KEYSELECT: u32 = 0x0401;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_CONTEXTMENU: u32 = 0x007b;

#[cfg(windows)]
mod windows {
    use super::{TrayEvent, tray_event};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::Cell,
        ffi::c_void,
        mem::size_of,
        ptr::{null, null_mut},
        sync::OnceLock,
        sync::atomic::{AtomicBool, Ordering},
    };

    type Hwnd = *mut c_void;
    type Hicon = *mut c_void;
    type Hmenu = *mut c_void;
    type Hinstance = *mut c_void;
    type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

    const WM_NCCREATE: u32 = 0x0081;
    const WM_NCDESTROY: u32 = 0x0082;
    const WM_APP: u32 = 0x8000;
    const WM_CLOSE: u32 = 0x0010;
    const WM_NULL: u32 = 0x0000;
    const GWLP_USERDATA: i32 = -21;
    const WS_EX_TOOLWINDOW: u32 = 0x0080;
    const CALLBACK_MESSAGE: u32 = WM_APP + 42;
    const ICON_ID: u32 = 1;
    const RESTORE_COMMAND: usize = 1;
    const EXIT_COMMAND: usize = 2;
    const LOCK_COMMAND: usize = 3;
    const NIM_ADD: u32 = 0;
    const NIM_DELETE: u32 = 2;
    const NIM_SETVERSION: u32 = 4;
    const NOTIFYICON_VERSION_4: u32 = 4;
    const NIF_MESSAGE: u32 = 0x0001;
    const NIF_ICON: u32 = 0x0002;
    const NIF_TIP: u32 = 0x0004;
    const NIF_SHOWTIP: u32 = 0x0080;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const TPM_RETURNCMD: u32 = 0x0100;
    const TPM_NONOTIFY: u32 = 0x0080;
    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;
    const HWND_TOP: Hwnd = std::ptr::null_mut();
    const HWND_TOPMOST: Hwnd = (-1isize) as Hwnd;
    const HWND_NOTOPMOST: Hwnd = (-2isize) as Hwnd;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    const WDA_NONE: u32 = 0x0000_0000;
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
    const WM_ACTIVATE_EXISTING: u32 = WM_APP + 43;
    const ERROR_ALREADY_EXISTS: u32 = 183;
    const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
    const WAIT_OBJECT_0: u32 = 0;
    const IDI_APPLICATION: *const u16 = 32512usize as *const u16;

    const WINDOW_CLASS: &[u16] = &[
        66, 121, 116, 101, 119, 97, 114, 100, 101, 110, 46, 71, 80, 85, 73, 46, 84, 114, 97, 121, 0,
    ];
    const ACTIVATION_CLASS: &[u16] = &[
        66, 121, 116, 101, 119, 97, 114, 100, 101, 110, 46, 71, 80, 85, 73, 46, 65, 99, 116, 105,
        118, 97, 116, 101, 0,
    ];
    static REGISTERED_CLASS: OnceLock<bool> = OnceLock::new();
    static REGISTERED_ACTIVATION_CLASS: OnceLock<bool> = OnceLock::new();

    #[repr(C)]
    struct WindowClassW {
        style: u32,
        window_proc: Option<WndProc>,
        class_extra: i32,
        window_extra: i32,
        instance: Hinstance,
        icon: Hicon,
        cursor: *mut c_void,
        background: *mut c_void,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[repr(C)]
    struct CreateStructW {
        create_params: *mut c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    struct NotifyIconDataW {
        size: u32,
        window: Hwnd,
        id: u32,
        flags: u32,
        callback_message: u32,
        icon: Hicon,
        tip: [u16; 128],
        state: u32,
        state_mask: u32,
        info: [u16; 256],
        timeout_or_version: u32,
        info_title: [u16; 64],
        info_flags: u32,
        guid: Guid,
        balloon_icon: Hicon,
    }

    struct TrayState {
        app_window: Hwnd,
        message_window: Hwnd,
        icon: Hicon,
        owns_icon: bool,
        registered: Cell<bool>,
        hidden: Cell<bool>,
        exit_requested: AtomicBool,
        taskbar_created: u32,
        lock_current: Box<dyn Fn()>,
        #[cfg(test)]
        shell_result: Cell<Option<bool>>,
        #[cfg(test)]
        notification_count: Cell<usize>,
    }

    impl TrayState {
        fn create_receiver(&mut self) -> bool {
            if self.taskbar_created == 0 || !ensure_window_class() {
                return false;
            }
            // TaskbarCreated is broadcast only to top-level windows. A hidden
            // tool window receives it without adding a taskbar/Alt-Tab entry.
            self.message_window = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    WINDOW_CLASS.as_ptr(),
                    WINDOW_CLASS.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    self as *mut Self as *mut c_void,
                )
            };
            !self.message_window.is_null()
        }

        fn notify_shell(&self, message: u32, data: &mut NotifyIconDataW) -> i32 {
            #[cfg(test)]
            if let Some(success) = self.shell_result.get() {
                self.notification_count
                    .set(self.notification_count.get() + 1);
                return i32::from(success);
            }
            unsafe { Shell_NotifyIconW(message, data) }
        }

        fn icon_data(&self) -> NotifyIconDataW {
            let mut tip = [0u16; 128];
            let label: Vec<u16> = "Bytewarden".encode_utf16().collect();
            tip[..label.len()].copy_from_slice(&label);
            NotifyIconDataW {
                size: size_of::<NotifyIconDataW>() as u32,
                window: self.message_window,
                id: ICON_ID,
                flags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
                callback_message: CALLBACK_MESSAGE,
                icon: self.icon,
                tip,
                state: 0,
                state_mask: 0,
                info: [0; 256],
                timeout_or_version: NOTIFYICON_VERSION_4,
                info_title: [0; 64],
                info_flags: 0,
                guid: Guid {
                    data1: 0,
                    data2: 0,
                    data3: 0,
                    data4: [0; 8],
                },
                balloon_icon: null_mut(),
            }
        }

        fn register(&self) -> bool {
            if self.registered.get() {
                return true;
            }
            let mut data = self.icon_data();
            if self.notify_shell(NIM_ADD, &mut data) == 0 {
                return false;
            }
            data.timeout_or_version = NOTIFYICON_VERSION_4;
            if self.notify_shell(NIM_SETVERSION, &mut data) == 0 {
                self.notify_shell(NIM_DELETE, &mut data);
                return false;
            }
            self.registered.set(true);
            true
        }

        fn unregister(&self) {
            if self.registered.get() {
                let mut data = self.icon_data();
                self.notify_shell(NIM_DELETE, &mut data);
                self.registered.set(false);
            }
        }

        fn restore(&self) {
            self.hidden.set(false);
            unsafe { restore_hwnd(self.app_window) };
        }

        fn show_menu(&self, x: i32, y: i32) {
            let menu = unsafe { CreatePopupMenu() };
            if menu.is_null() {
                return;
            }
            let restore_label: Vec<u16> = "打开 Bytewarden\0".encode_utf16().collect();
            let lock_label: Vec<u16> = "锁定当前账户\0".encode_utf16().collect();
            let exit_label: Vec<u16> = "退出 Bytewarden\0".encode_utf16().collect();
            let appended = unsafe {
                AppendMenuW(menu, 0, RESTORE_COMMAND, restore_label.as_ptr()) != 0
                    && AppendMenuW(menu, 0, LOCK_COMMAND, lock_label.as_ptr()) != 0
                    && AppendMenuW(menu, 0, EXIT_COMMAND, exit_label.as_ptr()) != 0
            };
            let selected = if appended {
                unsafe {
                    SetForegroundWindow(self.app_window);
                    let selected = TrackPopupMenuEx(
                        menu,
                        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                        x,
                        y,
                        self.app_window,
                        null(),
                    );
                    PostMessageW(self.message_window, WM_NULL, 0, 0);
                    selected
                }
            } else {
                0
            };
            unsafe { DestroyMenu(menu) };
            match selected as usize {
                RESTORE_COMMAND => self.restore(),
                LOCK_COMMAND => (self.lock_current)(),
                EXIT_COMMAND => {
                    self.exit_requested.store(true, Ordering::Release);
                    unsafe { PostMessageW(self.app_window, WM_CLOSE, 0, 0) };
                }
                _ => {}
            }
        }
    }

    pub(crate) struct Tray {
        state: Box<TrayState>,
    }

    impl Tray {
        pub(crate) fn for_window(
            window: &Window,
            lock_current: impl Fn() + 'static,
        ) -> Option<Self> {
            let handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return None;
            };
            let app_window = handle.hwnd.get() as Hwnd;
            if app_window.is_null() {
                return None;
            }
            let desired_px = ((32.0 * window.scale_factor()).round() as u16).clamp(48, 256);
            let (icon, owns_icon) = unsafe { app_icon(desired_px) }
                .map(|icon| (icon, true))
                .or_else(|| unsafe { Some((LoadIconW(null_mut(), IDI_APPLICATION), false)) })?;
            if icon.is_null() {
                return None;
            }
            let mut tray = Self {
                state: Box::new(TrayState {
                    app_window,
                    message_window: null_mut(),
                    icon,
                    owns_icon,
                    registered: Cell::new(false),
                    hidden: Cell::new(false),
                    exit_requested: AtomicBool::new(false),
                    taskbar_created: unsafe {
                        let taskbar: Vec<u16> = "TaskbarCreated\0".encode_utf16().collect();
                        RegisterWindowMessageW(taskbar.as_ptr())
                    },
                    lock_current: Box::new(lock_current),
                    #[cfg(test)]
                    shell_result: Cell::new(None),
                    #[cfg(test)]
                    notification_count: Cell::new(0),
                }),
            };
            if !tray.state.create_receiver() || !tray.state.register() {
                return None;
            }
            Some(tray)
        }

        pub(crate) fn is_registered(&self) -> bool {
            self.state.registered.get()
        }

        pub(crate) fn hide(&self) {
            unsafe { ShowWindow(self.state.app_window, SW_HIDE) };
            self.state.hidden.set(true);
        }

        pub(crate) fn take_exit_requested(&mut self) -> bool {
            self.state.exit_requested.swap(false, Ordering::AcqRel)
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            if self.state.message_window.is_null() {
                self.state.unregister();
            } else {
                unsafe {
                    SetWindowLongPtrW(self.state.message_window, GWLP_USERDATA, 0);
                }
                self.state.unregister();
                unsafe { DestroyWindow(self.state.message_window) };
                self.state.message_window = null_mut();
            }
            if self.state.owns_icon && !self.state.icon.is_null() {
                unsafe { DestroyIcon(self.state.icon) };
                self.state.icon = null_mut();
            }
        }
    }

    pub(crate) fn window_hwnd_usize(window: &Window) -> Option<usize> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get() as usize;
        (hwnd != 0).then_some(hwnd)
    }

    pub(crate) fn apply_screen_capture_protection(window: &Window, prevent: bool) {
        let Some(hwnd) = window_hwnd_usize(window) else {
            return;
        };
        let affinity = if prevent {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        unsafe {
            let _ = SetWindowDisplayAffinity(hwnd as Hwnd, affinity);
        }
    }

    pub(crate) fn restore_window_to_front(window: &Window) {
        if let Some(hwnd) = window_hwnd_usize(window) {
            unsafe { restore_hwnd(hwnd as Hwnd) };
        }
    }

    unsafe fn restore_hwnd(app_window: Hwnd) {
        if app_window.is_null() {
            return;
        }
        let cmd = if unsafe { IsIconic(app_window) } != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        unsafe {
            ShowWindow(app_window, cmd);
            BringWindowToTop(app_window);
            SetForegroundWindow(app_window);
            if GetForegroundWindow() != app_window {
                // Bypass Windows' foreground lock when activating from tray or a
                // second-instance relaunch while another app owns the foreground.
                let fg = GetForegroundWindow();
                let fg_tid = if fg.is_null() {
                    0
                } else {
                    GetWindowThreadProcessId(fg, null_mut())
                };
                let cur_tid = GetCurrentThreadId();
                let attached =
                    fg_tid != 0 && fg_tid != cur_tid && AttachThreadInput(cur_tid, fg_tid, 1) != 0;
                BringWindowToTop(app_window);
                SetWindowPos(
                    app_window,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                SetForegroundWindow(app_window);
                if attached {
                    AttachThreadInput(cur_tid, fg_tid, 0);
                }
            }
            if GetForegroundWindow() != app_window {
                // Final Z-order pulse guarantees visibility even behind elevated windows.
                SetWindowPos(
                    app_window,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                SetWindowPos(
                    app_window,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                SetForegroundWindow(app_window);
            }
        }
    }

    pub(super) fn profile_scope_tag(local_app_data: Option<&str>) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        let normalized = local_app_data.unwrap_or("default");
        for byte in normalized.as_bytes() {
            let lower = byte.to_ascii_lowercase();
            hash ^= u64::from(lower);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }

    fn current_scope_tag() -> String {
        let local = std::env::var("LOCALAPPDATA").ok();
        profile_scope_tag(local.as_deref())
    }

    fn encode_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn ensure_activation_class() -> bool {
        *REGISTERED_ACTIVATION_CLASS.get_or_init(|| unsafe {
            let instance = GetModuleHandleW(null());
            let class = WindowClassW {
                style: 0,
                window_proc: Some(activation_window_proc),
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: null_mut(),
                cursor: null_mut(),
                background: null_mut(),
                menu_name: null(),
                class_name: ACTIVATION_CLASS.as_ptr(),
            };
            if RegisterClassW(&class) != 0 {
                true
            } else {
                GetLastError() == ERROR_CLASS_ALREADY_EXISTS
            }
        })
    }

    unsafe extern "system" fn activation_window_proc(
        window: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if message == WM_NCCREATE {
            let create = lparam as *const CreateStructW;
            if !create.is_null() {
                let app_hwnd = unsafe { (*create).create_params };
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, app_hwnd as isize) };
            }
            return 1;
        }
        if message == WM_ACTIVATE_EXISTING {
            let app_hwnd = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as Hwnd };
            unsafe { restore_hwnd(app_hwnd) };
            return 0;
        }
        if message == WM_NCDESTROY {
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// Per-profile single-instance guard. When a second GPUI process starts for the same
    /// `LOCALAPPDATA` profile, it signals the existing instance to restore and focus its
    /// window, then exits silently instead of opening a duplicate error window.
    pub(crate) struct SingleInstance {
        mutex: *mut c_void,
        pending_activation: *mut c_void,
        activation_window: Hwnd,
        window_title: Vec<u16>,
    }

    impl SingleInstance {
        pub(crate) fn acquire_or_activate(silent_if_running: bool) -> Option<Self> {
            let tag = current_scope_tag();
            Self::acquire_in_scope(&tag, silent_if_running)
        }

        fn acquire_in_scope(tag: &str, silent_if_running: bool) -> Option<Self> {
            let mutex_name = encode_wide(&format!("Local\\Bytewarden.GPUI.SingleInstance.{tag}"));
            let event_name =
                encode_wide(&format!("Local\\Bytewarden.GPUI.PendingActivation.{tag}"));
            let window_title = encode_wide(&format!("Bytewarden.GPUI.Activate.{tag}"));
            // Hold the event before publishing the mutex. Early relaunches can
            // queue activation even while the primary is opening its window.
            let pending_activation = unsafe { CreateEventW(null(), 0, 0, event_name.as_ptr()) };
            let mutex = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
            if mutex.is_null() {
                // Fail open if mutex creation is denied by environment policy.
                return Some(Self {
                    mutex: null_mut(),
                    pending_activation,
                    activation_window: null_mut(),
                    window_title,
                });
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                if !silent_if_running {
                    unsafe {
                        if !pending_activation.is_null() {
                            SetEvent(pending_activation);
                        }
                        let existing =
                            FindWindowW(ACTIVATION_CLASS.as_ptr(), window_title.as_ptr());
                        if !existing.is_null() {
                            let mut pid = 0;
                            GetWindowThreadProcessId(existing, &mut pid);
                            if pid != 0 {
                                AllowSetForegroundWindow(pid);
                            }
                            let mut delivered = 0usize;
                            let sent = SendMessageTimeoutW(
                                existing,
                                WM_ACTIVATE_EXISTING,
                                0,
                                0,
                                SMTO_ABORTIFHUNG,
                                2000,
                                &mut delivered,
                            ) != 0
                                || PostMessageW(existing, WM_ACTIVATE_EXISTING, 0, 0) != 0;
                            if sent && !pending_activation.is_null() {
                                ResetEvent(pending_activation);
                            }
                        }
                    }
                }
                unsafe {
                    CloseHandle(mutex);
                    if !pending_activation.is_null() {
                        CloseHandle(pending_activation);
                    }
                }
                return None;
            }
            Some(Self {
                mutex,
                pending_activation,
                activation_window: null_mut(),
                window_title,
            })
        }

        pub(crate) fn attach_window(&mut self, window: &Window) {
            if !self.activation_window.is_null() || !ensure_activation_class() {
                return;
            }
            let Ok(handle) = HasWindowHandle::window_handle(window) else {
                return;
            };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return;
            };
            let app_window = handle.hwnd.get() as Hwnd;
            if app_window.is_null() {
                return;
            }
            self.activation_window = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    ACTIVATION_CLASS.as_ptr(),
                    self.window_title.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    app_window,
                )
            };
            if !self.activation_window.is_null()
                && !self.pending_activation.is_null()
                && unsafe { WaitForSingleObject(self.pending_activation, 0) } == WAIT_OBJECT_0
            {
                unsafe { PostMessageW(self.activation_window, WM_ACTIVATE_EXISTING, 0, 0) };
            }
        }
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            if !self.activation_window.is_null() {
                unsafe {
                    SetWindowLongPtrW(self.activation_window, GWLP_USERDATA, 0);
                    DestroyWindow(self.activation_window);
                }
                self.activation_window = null_mut();
            }
            if !self.mutex.is_null() {
                unsafe { CloseHandle(self.mutex) };
                self.mutex = null_mut();
            }
            if !self.pending_activation.is_null() {
                unsafe { CloseHandle(self.pending_activation) };
                self.pending_activation = null_mut();
            }
        }
    }

    fn ensure_window_class() -> bool {
        *REGISTERED_CLASS.get_or_init(|| unsafe {
            let instance = GetModuleHandleW(null());
            let class = WindowClassW {
                style: 0,
                window_proc: Some(tray_window_proc),
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: null_mut(),
                cursor: null_mut(),
                background: null_mut(),
                menu_name: null(),
                class_name: WINDOW_CLASS.as_ptr(),
            };
            if RegisterClassW(&class) != 0 {
                true
            } else {
                GetLastError() == ERROR_CLASS_ALREADY_EXISTS
            }
        })
    }

    unsafe extern "system" fn tray_window_proc(
        window: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if message == WM_NCCREATE {
            let create = lparam as *const CreateStructW;
            if create.is_null() {
                return 0;
            }
            let state = unsafe { (*create).create_params };
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, state as isize) };
            return 1;
        }
        let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *mut TrayState };
        if !state.is_null() {
            let state = unsafe { &*state };
            if message == CALLBACK_MESSAGE {
                let packed = lparam as u32;
                let id = packed >> 16;
                if id == ICON_ID {
                    match tray_event((packed & 0xffff) as u16) {
                        TrayEvent::Restore => state.restore(),
                        TrayEvent::ContextMenu => {
                            let x = (wparam as u16 as i16) as i32;
                            let y = ((wparam >> 16) as u16 as i16) as i32;
                            state.show_menu(x, y);
                        }
                        TrayEvent::Ignore => {}
                    }
                }
                return 0;
            }
            if message == state.taskbar_created {
                state.registered.set(false);
                if !state.register() && state.hidden.get() {
                    state.restore();
                }
                return 0;
            }
            if message == WM_NCDESTROY {
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
            }
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// Pick the crispest ICO entry whose width is at least `desired_px` (falling back
    /// to the largest available entry) so High-DPI Windows 11 system trays and overflow
    /// flyouts never upscale the tiny 16x16 bitmap.
    pub(super) fn select_ico_entry(data: &[u8], desired_px: u16) -> Option<(u16, &[u8])> {
        if data.len() < 6
            || u16::from_le_bytes([data[0], data[1]]) != 0
            || u16::from_le_bytes([data[2], data[3]]) != 1
        {
            return None;
        }
        let count = usize::from(u16::from_le_bytes([data[4], data[5]]));
        let mut best_ge: Option<(u16, &[u8])> = None;
        let mut largest: Option<(u16, &[u8])> = None;
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
            if width >= desired_px && best_ge.is_none_or(|(best_w, _)| width < best_w) {
                best_ge = Some((width, slice));
            }
            if largest.is_none_or(|(max_w, _)| width > max_w) {
                largest = Some((width, slice));
            }
        }
        best_ge.or(largest)
    }

    unsafe fn app_icon(desired_px: u16) -> Option<Hicon> {
        let data = include_bytes!("../../Bytewarden.WinUI/Assets/AppIcon.ico");
        let (_, image) = select_ico_entry(data, desired_px)?;
        let size = u32::try_from(image.len()).ok()?;
        let icon =
            unsafe { CreateIconFromResourceEx(image.as_ptr(), size, 1, 0x0003_0000, 0, 0, 0) };
        (!icon.is_null()).then_some(icon)
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassW(class: *const WindowClassW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Hwnd,
            menu: Hmenu,
            instance: Hinstance,
            parameter: *mut c_void,
        ) -> Hwnd;
        fn DefWindowProcW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
        fn SetWindowLongPtrW(window: Hwnd, index: i32, value: isize) -> isize;
        fn GetWindowLongPtrW(window: Hwnd, index: i32) -> isize;
        fn DestroyWindow(window: Hwnd) -> i32;
        fn ShowWindow(window: Hwnd, command: i32) -> i32;
        fn IsIconic(window: Hwnd) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
        fn SetWindowPos(
            window: Hwnd,
            insert_after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn BringWindowToTop(window: Hwnd) -> i32;
        fn SetForegroundWindow(window: Hwnd) -> i32;
        fn SendMessageTimeoutW(
            window: Hwnd,
            message: u32,
            wparam: usize,
            lparam: isize,
            flags: u32,
            timeout_ms: u32,
            result: *mut usize,
        ) -> isize;
        fn SetWindowDisplayAffinity(window: Hwnd, affinity: u32) -> i32;
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
        fn GetWindowThreadProcessId(window: Hwnd, pid: *mut u32) -> u32;
        fn AllowSetForegroundWindow(pid: u32) -> i32;
        fn RegisterWindowMessageW(name: *const u16) -> u32;
        fn LoadIconW(instance: Hinstance, name: *const u16) -> Hicon;
        fn DestroyIcon(icon: Hicon) -> i32;
        fn CreateIconFromResourceEx(
            data: *const u8,
            size: u32,
            icon: i32,
            version: u32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> Hicon;
        fn CreatePopupMenu() -> Hmenu;
        fn AppendMenuW(menu: Hmenu, flags: u32, id: usize, label: *const u16) -> i32;
        fn DestroyMenu(menu: Hmenu) -> i32;
        fn TrackPopupMenuEx(
            menu: Hmenu,
            flags: u32,
            x: i32,
            y: i32,
            owner: Hwnd,
            parameters: *const c_void,
        ) -> u32;
        fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Hinstance;
        fn GetLastError() -> u32;
        fn CreateMutexW(
            attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn CreateEventW(
            attributes: *const c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn SetEvent(event: *mut c_void) -> i32;
        fn ResetEvent(event: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetCurrentThreadId() -> u32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn Shell_NotifyIconW(message: u32, data: *mut NotifyIconDataW) -> i32;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn early_relaunch_queues_activation_without_a_window() {
            let tag = uuid::Uuid::new_v4().simple().to_string();
            let primary = SingleInstance::acquire_in_scope(&tag, false).unwrap();
            assert!(!primary.pending_activation.is_null());
            assert!(SingleInstance::acquire_in_scope(&tag, true).is_none());
            assert_ne!(
                unsafe { WaitForSingleObject(primary.pending_activation, 0) },
                WAIT_OBJECT_0
            );
            assert!(SingleInstance::acquire_in_scope(&tag, false).is_none());
            assert_eq!(
                unsafe { WaitForSingleObject(primary.pending_activation, 0) },
                WAIT_OBJECT_0
            );
            assert_ne!(
                unsafe { WaitForSingleObject(primary.pending_activation, 0) },
                WAIT_OBJECT_0
            );
        }

        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateDesktopW(
                name: *const u16,
                device: *const u16,
                mode: *const c_void,
                flags: u32,
                access: u32,
                attributes: *const c_void,
            ) -> Hwnd;
            fn GetThreadDesktop(thread: u32) -> Hwnd;
            fn SetThreadDesktop(desktop: Hwnd) -> i32;
            fn CloseDesktop(desktop: Hwnd) -> i32;
            fn SendMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
            fn IsWindowVisible(window: Hwnd) -> i32;
        }

        struct Desktop {
            current: Hwnd,
            previous: Hwnd,
        }
        impl Drop for Desktop {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDesktop(self.previous);
                    CloseDesktop(self.current);
                }
            }
        }
        struct TestWindow(Hwnd);
        impl Drop for TestWindow {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }

        #[test]
        fn taskbar_broadcast_reaches_hidden_receiver_and_recovers_success_failure_and_retry() {
            // No Explorer restart or shell icon writes: only this private desktop
            // receives the real Win32 broadcast, with an injected shell result.
            std::thread::spawn(|| unsafe {
                let previous = GetThreadDesktop(GetCurrentThreadId());
                let name = format!(
                    "BytewardenReview-{}-{}\0",
                    std::process::id(),
                    GetCurrentThreadId()
                )
                .encode_utf16()
                .collect::<Vec<_>>();
                let current = CreateDesktopW(name.as_ptr(), null(), null(), 0, 0x000f01ff, null());
                assert!(
                    !current.is_null(),
                    "CreateDesktopW failed: {}",
                    GetLastError()
                );
                let _desktop = Desktop { current, previous };
                assert_ne!(SetThreadDesktop(current), 0);
                let class = "STATIC\0".encode_utf16().collect::<Vec<_>>();
                let app = TestWindow(CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    0x80000000,
                    0,
                    0,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null_mut(),
                ));
                assert!(!app.0.is_null());
                let taskbar = "TaskbarCreated\0".encode_utf16().collect::<Vec<_>>();
                let mut tray = Tray {
                    state: Box::new(TrayState {
                        app_window: app.0,
                        message_window: null_mut(),
                        icon: null_mut(),
                        owns_icon: false,
                        registered: Cell::new(false),
                        hidden: Cell::new(true),
                        exit_requested: AtomicBool::new(false),
                        taskbar_created: RegisterWindowMessageW(taskbar.as_ptr()),
                        lock_current: Box::new(|| {}),
                        shell_result: Cell::new(Some(true)),
                        notification_count: Cell::new(0),
                    }),
                };
                assert!(tray.state.create_receiver());
                assert!(tray.state.register());
                tray.state.notification_count.set(0);
                SendMessageW(0xffffusize as Hwnd, tray.state.taskbar_created, 0, 0);
                assert_eq!(
                    tray.state.notification_count.get(),
                    2,
                    "broadcast must reach the actual receiver"
                );
                assert!(tray.is_registered());
                assert!(tray.state.hidden.get());
                tray.state.shell_result.set(Some(false));
                SendMessageW(0xffffusize as Hwnd, tray.state.taskbar_created, 0, 0);
                assert!(!tray.is_registered());
                assert!(
                    !tray.state.hidden.get(),
                    "failed registration must restore the window"
                );
                assert_ne!(IsWindowVisible(app.0), 0);
                tray.state.shell_result.set(Some(true));
                SendMessageW(0xffffusize as Hwnd, tray.state.taskbar_created, 0, 0);
                assert!(
                    tray.is_registered(),
                    "a later broadcast must recover after a failed attempt"
                );
            })
            .join()
            .expect("isolated desktop tray regression");
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::{
    SingleInstance, Tray, apply_screen_capture_protection, restore_window_to_front,
    window_hwnd_usize,
};

#[cfg(not(windows))]
pub(crate) struct Tray;

#[cfg(not(windows))]
pub(crate) fn apply_screen_capture_protection(_: &gpui::Window, _: bool) {}

#[cfg(not(windows))]
pub(crate) fn restore_window_to_front(_: &gpui::Window) {}

#[cfg(not(windows))]
pub(crate) fn window_hwnd_usize(_: &gpui::Window) -> Option<usize> {
    None
}

#[cfg(not(windows))]
pub(crate) struct SingleInstance;

#[cfg(not(windows))]
impl SingleInstance {
    pub(crate) fn acquire_or_activate(_: bool) -> Option<Self> {
        Some(Self)
    }

    pub(crate) fn attach_window(&mut self, _: &gpui::Window) {}
}

#[cfg(not(windows))]
impl Tray {
    pub(crate) fn for_window(_: &gpui::Window, _: impl Fn() + 'static) -> Option<Self> {
        None
    }

    pub(crate) fn is_registered(&self) -> bool {
        false
    }

    pub(crate) fn hide(&self) {}

    pub(crate) fn take_exit_requested(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{CloseDecision, TrayEvent, close_decision, tray_event};

    #[test]
    fn close_routes_to_tray_only_when_enabled_and_registered() {
        assert_eq!(
            close_decision(true, true, false, false),
            CloseDecision::HideToTray
        );
        assert_eq!(
            close_decision(true, false, false, false),
            CloseDecision::ShutDown
        );
        assert_eq!(
            close_decision(false, true, false, false),
            CloseDecision::ShutDown
        );
    }

    #[test]
    fn explicit_exit_and_repeated_shutdown_never_hide_the_window() {
        assert_eq!(
            close_decision(true, true, true, false),
            CloseDecision::ShutDown
        );
        assert_eq!(
            close_decision(true, true, false, true),
            CloseDecision::KeepOpen
        );
    }

    #[test]
    fn shell_callback_mapping_ignores_unknown_events() {
        assert_eq!(tray_event(0x0400), TrayEvent::Restore);
        assert_eq!(tray_event(0x0401), TrayEvent::Restore);
        assert_eq!(tray_event(0x0203), TrayEvent::Restore);
        assert_eq!(tray_event(0x007b), TrayEvent::ContextMenu);
        assert_eq!(tray_event(0x0200), TrayEvent::Ignore);
    }

    #[cfg(windows)]
    #[test]
    fn select_ico_entry_prefers_high_dpi_entry_over_tiny_16px() {
        let data = include_bytes!("../../Bytewarden.WinUI/Assets/AppIcon.ico");
        let (w48, bytes48) = super::windows::select_ico_entry(data, 48).expect("48px entry");
        assert!(w48 >= 48);
        assert!(bytes48.len() > 1000);

        let (w64, bytes64) = super::windows::select_ico_entry(data, 64).expect("64px entry");
        assert!(w64 >= 64);
        assert!(bytes64.len() > bytes48.len());
    }

    #[cfg(windows)]
    #[test]
    fn single_instance_scope_tag_is_case_insensitive_and_isolates_profiles() {
        let a = super::windows::profile_scope_tag(Some(r"C:\Users\Alice\AppData\Local"));
        let b = super::windows::profile_scope_tag(Some(r"c:\users\alice\appdata\local"));
        let c = super::windows::profile_scope_tag(Some(r"C:\Temp\IsolatedProfile"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
