use super::*;
use crate::passkey_provider;

fn is_vault_registration(
    server: &AuthServer,
    operation: &crate::vault::passkeys::PasskeyOperation,
) -> bool {
    if !operation.creating {
        return false;
    }
    let Ok(api) = Url::parse(&server.api_url) else {
        return false;
    };
    let Some(host) = api.host_str() else {
        return false;
    };
    let host = match (server.self_hosted, host) {
        (false, "api.bitwarden.com") => "vault.bitwarden.com",
        (false, "api.bitwarden.eu") => "vault.bitwarden.eu",
        _ => host,
    };
    let host = host.trim_end_matches('.');
    let rp = operation.rp_id.trim_end_matches('.').to_ascii_lowercase();
    host == rp
        || host
            .strip_suffix(&rp)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

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
        let mut needs_sync = false;
        if state == SESSION_UNLOCKED {
            let Ok(guard) = auth_client().try_lock() else {
                return vault_error("busy");
            };
            let Some(session) = guard
                .as_ref()
                .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
            else {
                return vault_access_error();
            };
            if is_vault_registration(&session.server, &request.operation) {
                request.reject_before_approval(VaultError::PermissionDenied);
                return vault_success(&serde_json::json!({"id":""}));
            }
            let Some(runtime) = runtime() else {
                return vault_error("internal");
            };
            match runtime.block_on(
                session
                    .vault
                    .passkey_choices(&session.client, &request.operation),
            ) {
                Ok(result) if result.is_empty() && !request.operation.creating => {
                    request.reject_before_approval(VaultError::NotFound);
                    return vault_success(&serde_json::json!({"id":""}));
                }
                Ok(result) => choices = result,
                Err(VaultError::NotSynced) => needs_sync = true,
                Err(e) => return vault_error(&e.to_string()),
            }
        }
        vault_success(
            &serde_json::json!({"id":request.id, "creating":request.operation.creating,
            "relyingParty":request.operation.rp_id, "userName":request.operation.user_name,
            "sessionState":state, "choices":choices, "needsSync":needs_sync}),
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

#[no_mangle]
pub extern "C" fn bw_passkey_sync_metadata() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if bw_auth_session_state() != SESSION_UNLOCKED || passkey_provider::state() != "enabled" {
            return vault_success(&false);
        }
        let Ok(guard) = auth_client().try_lock() else {
            return vault_success(&false);
        };
        let Some(session) = guard
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_success(&false);
        };
        if session.vault.needs_reconciliation() || session.vault.overview().is_err() {
            return vault_success(&false);
        }
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let synced = runtime.block_on(passkey_provider::sync_metadata(
            &session.vault,
            &session.client,
        ));
        vault_success(&synced)
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
            if is_vault_registration(&session.server, &request.operation) { return Err(VaultError::PermissionDenied); }
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
            Ok(bytes) => {
                if request.complete(Ok(bytes)) {
                    vault_success(&true)
                } else {
                    vault_error("cancelled")
                }
            }
            Err(e) => {
                let error = e.to_string();
                if request.complete(Err(error.clone())) {
                    vault_error(&error)
                } else {
                    vault_error("cancelled")
                }
            }
        }
    })).unwrap_or_else(|_| vault_error("internal"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::passkeys::PasskeyOperation;

    #[test]
    fn passkey_registration_cannot_store_the_current_vaults_key_in_itself() {
        let us = auth_server(0, "").unwrap();
        let eu = auth_server(1, "").unwrap();
        let hosted = auth_server(2, "https://vault.example.invalid:8443/bitwarden").unwrap();
        let mut op = PasskeyOperation {
            creating: true,
            rp_id: String::new(),
            user_name: None,
            user_display_name: None,
            user_handle: vec![],
            client_data_hash: vec![],
            credential_ids: vec![],
            algorithms: vec![],
        };
        for (server, rp, blocked) in [
            (&us, "vault.bitwarden.com", true),
            (&eu, "bitwarden.eu", true),
            (&hosted, "VAULT.example.invalid.", true),
            (&hosted, "example.invalid", true),
            (&hosted, "ample.invalid", false),
            (&hosted, "other.example.invalid", false),
        ] {
            op.rp_id = rp.to_owned();
            assert_eq!(is_vault_registration(server, &op), blocked, "{rp}");
        }
        op.creating = false;
        op.rp_id = "vault.bitwarden.com".to_owned();
        assert!(!is_vault_registration(&us, &op));
    }
}
