//! Minimal documented WinRT UISettings ABI. No registry writes, polling, or new
//! Windows projection dependency. ABI/enum/delegate IIDs are from Windows SDK
//! 10.0.26100.0 winrt/windows.ui.viewmanagement.h (IUISettings3).
use std::{
    cell::RefCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicU32, AtomicU64, Ordering, fence},
};

use gpui::WindowAppearance;
use windows_sys::core::{GUID, IInspectable_Vtbl, IUnknown_Vtbl};

use super::{AccentCache, Accents};

const UI_SETTINGS_3: GUID = GUID::from_u128(0x03021be4_5254_4781_8194_5168f7d06d7b);
const COLOR_HANDLER: GUID = GUID::from_u128(0x2dbdba9d_20da_519d_9078_09f835bc5bc7);
const AGILE_OBJECT: GUID = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
static REVISION: AtomicU64 = AtomicU64::new(0);

#[link(name = "runtimeobject")]
unsafe extern "system" {
    fn RoInitialize(init_type: u32) -> i32;
    fn RoUninitialize();
    fn RoActivateInstance(class: *mut c_void, instance: *mut *mut c_void) -> i32;
    fn WindowsCreateString(text: *const u16, len: u32, string: *mut *mut c_void) -> i32;
    fn WindowsDeleteString(string: *mut c_void) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct Color {
    a: u8,
    r: u8,
    g: u8,
    b: u8,
}

impl Color {
    fn rgb(self) -> Option<u32> {
        // System accent resources are opaque; reject an invalid/partial result.
        (self.a == 255).then_some((self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32)
    }
}

#[repr(C)]
struct SettingsVtable {
    base: IInspectable_Vtbl,
    get_color: unsafe extern "system" fn(*mut c_void, i32, *mut Color) -> i32,
    add_changed: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> i32,
    remove_changed: unsafe extern "system" fn(*mut c_void, i64) -> i32,
}

// Confined to this UI thread's TLS, including subscription removal and Release.
// Only the delegate's atomic revision counter is touched by a WinRT worker thread.
struct Settings {
    interface: *mut c_void,
    cookie: Option<i64>,
    initialized: bool,
}

impl Settings {
    fn new() -> Option<Self> {
        // GPUI already calls OleInitialize (STA). Balance our own successful
        // RoInitialize, including S_FALSE. An existing MTA is also usable.
        let initialized = unsafe { RoInitialize(0) };
        if initialized < 0 && initialized != 0x80010106u32 as i32 {
            return None;
        }
        let mut settings = Self {
            interface: ptr::null_mut(),
            cookie: None,
            initialized: initialized >= 0,
        };
        let name = "Windows.UI.ViewManagement.UISettings";
        let mut utf16 = [0u16; 64];
        for (out, byte) in utf16.iter_mut().zip(name.bytes()) {
            *out = byte as u16;
        }
        let mut class = ptr::null_mut();
        // The class name is ASCII, all lengths fit the fixed local UTF-16 buffer.
        if unsafe { WindowsCreateString(utf16.as_ptr(), name.len() as u32, &mut class) } < 0 {
            return None;
        }
        let mut instance = ptr::null_mut();
        let activated = unsafe { RoActivateInstance(class, &mut instance) };
        unsafe { WindowsDeleteString(class) };
        if activated < 0 || instance.is_null() {
            return None;
        }
        // RoActivateInstance returns IInspectable; its first three slots are IUnknown.
        let base = unsafe { &**(instance as *const *const IUnknown_Vtbl) };
        let queried =
            unsafe { (base.QueryInterface)(instance, &UI_SETTINGS_3, &mut settings.interface) };
        unsafe { (base.Release)(instance) };
        if queried < 0 || settings.interface.is_null() {
            return None;
        }
        let handler = Box::into_raw(Box::new(ColorChanged {
            vtable: &HANDLER_VTABLE,
            references: AtomicU32::new(1),
        }));
        let mut cookie = 0;
        // WinRT retains the delegate on success. Release our construction reference
        // on both paths; subscription removal releases WinRT's reference at shutdown.
        let subscribed = unsafe {
            (settings.vtable().add_changed)(settings.interface, handler.cast(), &mut cookie)
        };
        unsafe { release(handler.cast()) };
        if subscribed >= 0 {
            settings.cookie = Some(cookie);
        }
        Some(settings)
    }

    fn vtable(&self) -> &SettingsVtable {
        // The interface is non-null after successful QI and lives until Drop.
        unsafe { &**(self.interface as *const *const SettingsVtable) }
    }

    fn color(&self, kind: i32) -> Option<u32> {
        let mut color = Color::default();
        let result = unsafe { (self.vtable().get_color)(self.interface, kind, &mut color) };
        if result < 0 { None } else { color.rgb() }
    }

    fn accents(&self) -> Option<Accents> {
        // WinUI generic.xaml: AccentTextFillColorPrimaryBrush = Dark2/Light3;
        // AccentFillColorDefaultBrush = Dark1/Light2 (light/dark theme respectively).
        Some(Accents {
            light_text: self.color(3)?, // UIColorType::AccentDark2
            dark_text: self.color(8)?,  // UIColorType::AccentLight3
            light_fill: self.color(4)?, // UIColorType::AccentDark1
            dark_fill: self.color(7)?,  // UIColorType::AccentLight2
        })
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        if !self.interface.is_null() {
            let vtable = self.vtable();
            if let Some(cookie) = self.cookie {
                unsafe { (vtable.remove_changed)(self.interface, cookie) };
            }
            unsafe { (vtable.base.base.Release)(self.interface) };
        }
        if self.initialized {
            unsafe { RoUninitialize() };
        }
    }
}

#[repr(C)]
struct ColorChanged {
    vtable: &'static HandlerVtable,
    references: AtomicU32,
}

#[repr(C)]
struct HandlerVtable {
    base: IUnknown_Vtbl,
    invoke: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
}

static HANDLER_VTABLE: HandlerVtable = HandlerVtable {
    base: IUnknown_Vtbl {
        QueryInterface: query_interface,
        AddRef: add_ref,
        Release: release,
    },
    invoke: changed,
};

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    result: *mut *mut c_void,
) -> i32 {
    if result.is_null() || iid.is_null() {
        return 0x80004003u32 as i32; // E_POINTER
    }
    unsafe { *result = ptr::null_mut() };
    let iid = unsafe { &*iid };
    if [windows_sys::core::IID_IUnknown, COLOR_HANDLER, AGILE_OBJECT]
        .iter()
        .any(|supported| same_guid(iid, supported))
    {
        unsafe {
            *result = this;
            add_ref(this);
        }
        0
    } else {
        0x80004002u32 as i32 // E_NOINTERFACE; delegates do not implement IInspectable.
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    let handler = unsafe { &*this.cast::<ColorChanged>() };
    handler.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let handler = unsafe { &*this.cast::<ColorChanged>() };
    let remaining = handler.references.fetch_sub(1, Ordering::Release) - 1;
    if remaining == 0 {
        fence(Ordering::Acquire);
        unsafe { drop(Box::from_raw(this.cast::<ColorChanged>())) };
    }
    remaining
}

unsafe extern "system" fn changed(
    _this: *mut c_void,
    _sender: *mut c_void,
    _args: *mut c_void,
) -> i32 {
    // UISettings raises this on a worker thread. Never touch GPUI, TLS, or the
    // apartment-owned Settings here. apply() consumes invalidation next render.
    REVISION.fetch_add(1, Ordering::Relaxed);
    0
}

struct SystemAccents {
    settings: Option<Settings>,
    cache: AccentCache,
    appearance: Option<WindowAppearance>,
}

thread_local! {
    static SYSTEM: RefCell<SystemAccents> = const { RefCell::new(SystemAccents {
        settings: None,
        cache: AccentCache { revision: None, colors: None },
        appearance: None,
    }) };
}

pub(super) fn accents(appearance: WindowAppearance) -> Accents {
    SYSTEM.with_borrow_mut(|system| {
        if system.appearance != Some(appearance) {
            system.appearance = Some(appearance);
            system.cache.revision = None;
            if system.settings.is_none() {
                system.settings = Settings::new();
            }
        }
        let revision = REVISION.load(Ordering::Relaxed);
        let settings = &system.settings;
        system
            .cache
            .refresh(revision, || settings.as_ref().and_then(Settings::accents))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_uisettings_api_reads_opaque_colors_and_subscribes() {
        let settings = Settings::new().expect("Windows UISettings activation");
        assert!(settings.cookie.is_some(), "ColorValuesChanged subscription");
        let accents = settings.accents().expect("opaque Windows accent shades");
        eprintln!("Windows UISettings accent shades: {accents:06x?}");
    }

    #[test]
    fn winrt_color_is_argb_bytes_and_must_be_opaque() {
        assert_eq!(
            Color {
                a: 255,
                r: 0xf3,
                g: 0x8c,
                b: 0xd2
            }
            .rgb(),
            Some(0xf38cd2)
        );
        assert_eq!(Color::default().rgb(), None);
        assert_eq!(std::mem::size_of::<Color>(), 4);
    }

    #[test]
    fn delegate_supports_only_its_documented_interfaces_and_balances_references() {
        let handler = Box::into_raw(Box::new(ColorChanged {
            vtable: &HANDLER_VTABLE,
            references: AtomicU32::new(1),
        }))
        .cast::<c_void>();
        for iid in [windows_sys::core::IID_IUnknown, COLOR_HANDLER, AGILE_OBJECT] {
            let mut result = ptr::null_mut();
            assert_eq!(unsafe { query_interface(handler, &iid, &mut result) }, 0);
            assert_eq!(result, handler);
            assert_eq!(unsafe { release(result) }, 1);
        }
        let mut result = handler;
        assert!(unsafe { query_interface(handler, &UI_SETTINGS_3, &mut result) } < 0);
        assert!(result.is_null());
        assert_eq!(unsafe { release(handler) }, 0);
    }
}
