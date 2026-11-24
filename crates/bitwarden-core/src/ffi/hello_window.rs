//! The two window-aware WinRT interfaces are not projected in windows 0.62.2.
//! ABI source: Microsoft windows-rs ab8988360f2d067d00d1f8e62e4d9d76a195bbc5,
//! crates/libs/windows/src/Windows/Security/Credentials/mod.rs. Remove these bindings
//! when upgrading to a released projection that contains both methods.
use std::{ffi::c_void, ptr};

use windows::{
    core::{imp::define_interface, IInspectable_Vtbl, Interface, HRESULT, HSTRING},
    Security::Credentials::{
        KeyCredential, KeyCredentialCreationOption, KeyCredentialManager,
        KeyCredentialOperationResult, KeyCredentialRetrievalResult,
    },
    Storage::Streams::IBuffer,
    Win32::Foundation::E_NOINTERFACE,
    UI::WindowId,
};
use windows_future::IAsyncOperation;

define_interface!(
    IKeyCredentialManagerCreateWithWindowStatics,
    IKeyCredentialManagerCreateWithWindowStatics_Vtbl,
    0x30b1b9c9_61ef_43e8_88ac_cc433b38d1a6
);
#[repr(C)]
pub struct IKeyCredentialManagerCreateWithWindowStatics_Vtbl {
    base: IInspectable_Vtbl,
    create: unsafe extern "system" fn(
        *mut c_void,
        WindowId,
        *mut c_void,
        KeyCredentialCreationOption,
        *mut *mut c_void,
    ) -> HRESULT,
}

define_interface!(
    IKeyCredentialWithWindow,
    IKeyCredentialWithWindow_Vtbl,
    0x050deb3b_b19c_4635_9df6_5650d66c62b1
);
#[repr(C)]
pub struct IKeyCredentialWithWindow_Vtbl {
    base: IInspectable_Vtbl,
    sign:
        unsafe extern "system" fn(*mut c_void, WindowId, *mut c_void, *mut *mut c_void) -> HRESULT,
}

// Only an absent interface (older Windows) permits a fallback. Do not turn cancelled,
// failed or already-displayed prompts into another prompt.
fn optional<T>(interface: windows::core::Result<T>) -> windows::core::Result<Option<T>> {
    match interface {
        Ok(interface) => Ok(Some(interface)),
        Err(error) if error.code() == E_NOINTERFACE => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn create(
    window: u64,
    name: &HSTRING,
    option: KeyCredentialCreationOption,
) -> windows::core::Result<IAsyncOperation<KeyCredentialRetrievalResult>> {
    if window != 0 {
        if let Some(factory) = optional(windows::core::factory::<
            KeyCredentialManager,
            IKeyCredentialManagerCreateWithWindowStatics,
        >())? {
            let mut result = ptr::null_mut();
            // The WinRT activation factory returned the interface with the vtable above.
            unsafe {
                (factory.vtable().create)(
                    factory.as_raw(),
                    WindowId { Value: window },
                    std::mem::transmute_copy(name),
                    option,
                    &mut result,
                )
                .ok()?;
                return windows::core::Type::from_abi(result);
            }
        }
    }
    KeyCredentialManager::RequestCreateAsync(name, option)
}

pub(super) fn sign(
    window: u64,
    credential: &KeyCredential,
    message: &IBuffer,
) -> windows::core::Result<IAsyncOperation<KeyCredentialOperationResult>> {
    if window != 0 {
        if let Some(credential) = optional(credential.cast::<IKeyCredentialWithWindow>())? {
            let mut result = ptr::null_mut();
            // QueryInterface returned the window-aware signing interface; message stays alive.
            unsafe {
                (credential.vtable().sign)(
                    credential.as_raw(),
                    WindowId { Value: window },
                    message.as_raw(),
                    &mut result,
                )
                .ok()?;
                return windows::core::Type::from_abi(result);
            }
        }
    }
    credential.RequestSignAsync(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{E_ABORT, E_ACCESSDENIED};

    #[test]
    fn legacy_fallback_is_only_for_a_missing_interface() {
        assert_eq!(optional(Ok(42)).unwrap(), Some(42));
        assert_eq!(optional::<()>(Err(E_NOINTERFACE.into())).unwrap(), None);
        for code in [E_ABORT, E_ACCESSDENIED] {
            assert_eq!(optional::<()>(Err(code.into())).unwrap_err().code(), code);
        }
    }
}
