use super::*;
use crate::passkey_provider;

#[no_mangle]
pub extern "C" fn bw_passkey_start() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| match passkey_provider::start() {
        Ok(()) => vault_success(&true),
        Err(e) => vault_error(&e.to_string()),
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}
#[no_mangle]
pub extern "C" fn bw_passkey_register() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| match passkey_provider::register() {
        Ok(()) => vault_success(&true),
        Err(e) => vault_error(&e.to_string()),
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}
#[no_mangle]
pub extern "C" fn bw_passkey_pending() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(request) = passkey_provider::pending() else {
            return vault_success(&serde_json::json!({"id":""}));
        };
        let state = bw_auth_session_state();
        let mut choices = vec![];
        if state == SESSION_UNLOCKED {
            let Ok(guard) = auth_client().try_lock() else {
                return vault_error("busy");
            };
            let Some(session) = guard.as_ref() else {
                return vault_access_error();
            };
            let Some(runtime) = runtime() else {
                return vault_error("internal");
            };
            match runtime.block_on(
                session
                    .vault
                    .passkey_choices(&session.client, &request.operation),
            ) {
                Ok(result) => choices = result,
                Err(e) => return vault_error(&e.to_string()),
            }
        }
        vault_success(
            &serde_json::json!({"id":request.id, "creating":request.operation.creating,
            "relyingParty":request.operation.rp_id, "userName":request.operation.user_name,
            "sessionState":state, "choices":choices}),
        )
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub extern "C" fn bw_passkey_state() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        vault_success(&passkey_provider::state())
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Approval {
    id: String,
    item_id: String,
    revision: String,
    credential_index: usize,
}

#[no_mangle]
pub unsafe extern "C" fn bw_passkey_active(id: *const c_char) -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(id) = (unsafe { read_utf8(id) }) else {
            return 0;
        };
        u8::from(passkey_provider::pending().is_some_and(|p| p.id == id))
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn bw_passkey_cancel(id: *const c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(id) = unsafe { read_utf8(id) } {
            passkey_provider::cancel(&id);
        }
    }));
}

#[no_mangle]
pub unsafe extern "C" fn bw_passkey_respond(
    json: *const c_char,
    password: *const c_char,
    window: usize,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(json) = (unsafe { read_utf8(json) }).filter(|s| s.len() <= 16384) else { return vault_error("validation"); };
        let Ok(approval) = serde_json::from_str::<Approval>(&json) else { return vault_error("validation"); };
        let Some(password) = (unsafe { read_utf8(password) }) else { return vault_error("validation"); };
        let password = Zeroizing::new(password);
        let Some(request) = passkey_provider::pending().filter(|p| p.id == approval.id) else { return vault_error("cancelled"); };
        if let Err(e) = request.begin() { return vault_error(&e.to_string()); }
        let result = (|| -> Result<Vec<u8>, VaultError> {
            request.verify(window, request.operation.user_name.as_deref().unwrap_or(""))?;
            let mut guard = auth_client().lock().map_err(|_| VaultError::Internal)?;
            let session = guard.as_mut().filter(|_| bw_auth_session_state() == SESSION_UNLOCKED).ok_or(VaultError::NotAuthenticated)?;
            if !request.is_current() { return Err(VaultError::NotFound); }
            let runtime = runtime().ok_or(VaultError::Internal)?;
            session.vault.reset_write_attempt();
            let result = catch_unwind(AssertUnwindSafe(|| runtime.block_on(async {
                tokio::select! {
                    result = session.vault.perform_passkey(&session.client, &request.operation, &approval.item_id,
                        &approval.revision, approval.credential_index, &password) => Some(result),
                    _ = async { while request.is_current() { tokio::time::sleep(std::time::Duration::from_millis(50)).await; } } => None,
                }
            })));
            match result {
                Ok(Some(Ok(result))) => {
                    runtime.block_on(passkey_provider::sync_metadata(&session.vault, &session.client));
                    passkey_provider::encode_response(result)
                }
                Ok(Some(Err(e))) => Err(e),
                _ if session.vault.write_in_flight() => { session.vault.mark_write_uncertain(); Err(VaultError::WriteUncertain) }
                _ => Err(VaultError::Internal),
            }
        })();
        match result {
            Ok(bytes) => { request.complete(Ok(bytes)); vault_success(&true) }
            Err(e) => { request.complete(Err(e.to_string())); vault_error(&e.to_string()) }
        }
    })).unwrap_or_else(|_| vault_error("internal"))
}
