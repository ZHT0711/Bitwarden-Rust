use std::{
    ffi::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{atomic::Ordering, MutexGuard, TryLockError},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    auth_client, auth_request, read_utf8, vault_error, vault_success, AuthServer, AuthSession,
    ACCOUNT_ALREADY_SIGNED_IN, ACCOUNT_LIMIT_REACHED, SESSION_LOCKED, SESSION_SIGNED_OUT,
    SESSION_STATE,
};

pub(crate) const MAX_ACCOUNTS: usize = 5;

#[derive(Default)]
pub(super) struct AuthAccounts {
    pub(super) active: Option<AuthSession>,
    pub(super) pending: Option<AuthSession>,
    inactive: Vec<AuthSession>,
    #[cfg(windows)]
    local_cache_loaded: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountSummary<'a> {
    id: Uuid,
    email: &'a str,
    server_mode: u32,
    server_url: &'a str,
    state: u8,
    active: bool,
    needs_sign_in: bool,
    #[cfg(windows)]
    hello_enabled: bool,
}

impl AuthAccounts {
    fn iter(&self) -> impl Iterator<Item = &AuthSession> {
        self.active.iter().chain(self.inactive.iter().rev())
    }

    fn response(&self) -> *mut c_char {
        let active = self.active.as_ref().map(|session| session.account_id);
        vault_success(
            &self
                .iter()
                .map(|session| AccountSummary {
                    id: session.account_id,
                    email: &session.email,
                    server_mode: session.server_mode,
                    server_url: &session.server_url,
                    state: session.state,
                    active: Some(session.account_id) == active,
                    needs_sign_in: session.offline_only,
                    #[cfg(windows)]
                    hello_enabled: session.unlock_data.is_some()
                        && super::hello::is_enabled(session),
                })
                .collect::<Vec<_>>(),
        )
    }

    pub(super) fn can_login(&self, server: &AuthServer, email: &str) -> Result<(), u32> {
        let mut accounts = self.iter();
        let matching = accounts.find(|session| session.server == *server && session.email == email);
        if matching.is_some_and(|session| !session.offline_only) {
            return Err(ACCOUNT_ALREADY_SIGNED_IN);
        }
        if matching.is_none() && self.iter().count() >= MAX_ACCOUNTS {
            return Err(ACCOUNT_LIMIT_REACHED);
        }
        Ok(())
    }

    pub(super) fn activate(&mut self, mut session: AuthSession) {
        let same = |existing: &AuthSession| {
            existing.server == session.server && existing.email == session.email
        };
        let previous = if self.active.as_ref().is_some_and(same) {
            self.active.take()
        } else {
            self.inactive
                .iter()
                .position(same)
                .map(|index| self.inactive.remove(index))
        };
        if let Some(mut previous) = previous {
            session.account_id = previous.account_id;
            previous.clear_vault_keys();
            clear_active_passkeys();
        }
        if let Some(mut previous) = self.active.replace(session) {
            previous.open_detail = None;
            self.inactive.push(previous);
            clear_active_passkeys();
        }
        self.publish_state();
    }

    fn switch(&mut self, id: Uuid) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.account_id == id)
        {
            return true;
        }
        let Some(index) = self
            .inactive
            .iter()
            .position(|session| session.account_id == id)
        else {
            return false;
        };
        self.pending = None;
        let account = self.inactive.remove(index);
        self.activate(account);
        true
    }

    pub(super) fn remove_active(&mut self) -> bool {
        let Some(mut active) = self.active.take() else {
            return true;
        };
        let revoked = revoke_and_clear(&mut active);
        self.active = if revoked {
            self.inactive.pop()
        } else {
            Some(active)
        };
        clear_active_passkeys();
        self.publish_state();
        revoked
    }

    fn clear_all(&mut self, revoke_cache: bool) -> bool {
        self.pending = None;
        let mut cleared = true;
        // Keep failed revocations visible as locked accounts so they can be retried.
        let mut discard = |account: &mut AuthSession| {
            let done = if revoke_cache {
                revoke_and_clear(account)
            } else {
                account.clear_vault_keys();
                true
            };
            cleared &= done;
            done
        };
        self.inactive.retain_mut(|account| !discard(account));
        if self.active.as_mut().is_none_or(discard) {
            self.active = self.inactive.pop();
        }
        clear_active_passkeys();
        self.publish_state();
        cleared
    }

    pub(super) fn lock_active(&mut self) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.state != SESSION_LOCKED {
            if !active.clear_vault_keys() {
                self.remove_active();
                return false;
            }
            active.state = SESSION_LOCKED;
        }
        clear_active_passkeys();
        self.publish_state();
        true
    }

    pub(super) fn publish_state(&self) {
        SESSION_STATE.store(
            self.active
                .as_ref()
                .map_or(SESSION_SIGNED_OUT, |session| session.state),
            Ordering::Release,
        );
    }

    fn timeout(&mut self, action: AccountTimeout) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.account_id == action.id)
        {
            if action.log_out {
                return self.remove_active();
            }
            self.lock_active();
            return true;
        }
        if let Some(index) = self
            .inactive
            .iter()
            .position(|session| session.account_id == action.id)
        {
            let session = &mut self.inactive[index];
            if action.log_out || !session.clear_vault_keys() {
                if !revoke_and_clear(session) {
                    return false;
                }
                self.inactive.remove(index);
            } else {
                session.state = SESSION_LOCKED;
            }
        }
        true
    }
}

fn revoke_and_clear(session: &mut AuthSession) -> bool {
    session.clear_vault_keys();
    session.unlock_data = None;
    session.state = SESSION_LOCKED;
    #[cfg(windows)]
    return super::remove_cached_vault(session);
    #[cfg(not(windows))]
    true
}

fn clear_active_passkeys() {
    #[cfg(windows)]
    let _ = catch_unwind(AssertUnwindSafe(|| {
        crate::passkey_provider::cancel("");
        crate::passkey_provider::clear_metadata();
    }));
}

pub(super) fn sessions() -> MutexGuard<'static, AuthAccounts> {
    #[allow(unused_mut)]
    let mut accounts = auth_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    #[cfg(windows)]
    if !accounts.local_cache_loaded && crate::local_cache::is_configured() {
        accounts.local_cache_loaded = true;
        let mut restored =
            super::restore_persisted_sessions(MAX_ACCOUNTS - accounts.iter().count());
        if accounts.active.is_none() {
            accounts.active = restored.pop();
        }
        accounts.inactive.extend(restored);
        accounts.publish_state();
    }
    accounts
}

#[no_mangle]
pub extern "C" fn bw_auth_accounts() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| sessions().response()))
        .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_auth_switch_account(id: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(id) = (unsafe { read_utf8(id) }).and_then(|id| Uuid::parse_str(&id).ok()) else {
            return vault_error("validation");
        };
        // A slow or unavailable server must not hold up activation of another cached account.
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        let mut accounts = sessions();
        if !accounts.switch(id) {
            return vault_error("accountNotFound");
        }
        accounts.response()
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub extern "C" fn bw_auth_cancel_login() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Ok(mut accounts) = auth_client().try_lock() {
            accounts.pending = None;
        }
    }));
}

#[no_mangle]
pub extern "C" fn bw_auth_logout() -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        let mut accounts = sessions();
        accounts.pending = None;
        u8::from(accounts.remove_active())
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn bw_auth_logout_all() -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        u8::from(sessions().clear_all(true))
    }))
    .unwrap_or(0)
}

/// Release in-memory keys when the desktop process exits while preserving the encrypted cache.
#[no_mangle]
pub extern "C" fn bw_auth_shutdown() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear_all(false);
    }));
}

#[no_mangle]
pub extern "C" fn bw_auth_lock() -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        u8::from(sessions().lock_active())
    }))
    .unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountTimeout {
    id: Uuid,
    log_out: bool,
}

#[no_mangle]
pub unsafe extern "C" fn bw_auth_apply_timeouts(
    actions: *const c_char,
    interrupt: u8,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(actions) = (unsafe { read_utf8(actions) }).filter(|json| json.len() <= 2048)
        else {
            return vault_error("validation");
        };
        let Ok(actions) = serde_json::from_str::<Vec<AccountTimeout>>(&actions) else {
            return vault_error("validation");
        };
        if actions.len() > MAX_ACCOUNTS {
            return vault_error("validation");
        }
        let mut accounts = if interrupt != 0 {
            auth_request::begin();
            #[cfg(windows)]
            crate::passkey_provider::cancel("");
            sessions()
        } else {
            match auth_client().try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => return vault_error("busy"),
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
            }
        };
        let mut cleared = true;
        for action in actions {
            cleared &= accounts.timeout(action);
        }
        if !cleared {
            return vault_error("cacheRevocationFailed");
        }
        accounts.response()
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::{auth_client_for_server, auth_server};

    #[test]
    fn offline_account_can_be_reauthenticated_without_consuming_an_account_slot() {
        let mut accounts = AuthAccounts::default();
        let us = auth_server(0, "").unwrap();
        let device_id = "11111111-1111-4111-8111-111111111111";
        let email = "offline@example.invalid";
        auth_client_for_server(&mut accounts.pending, &us, (email, 0, ""), device_id);
        let mut locked = accounts.pending.take().unwrap();
        locked.state = SESSION_LOCKED;
        assert!(!locked.offline_only);
        accounts.active = Some(locked);
        assert_eq!(
            accounts.can_login(&us, email),
            Err(ACCOUNT_ALREADY_SIGNED_IN)
        );
        accounts.active.as_mut().unwrap().offline_only = true;
        for index in 0..MAX_ACCOUNTS - 1 {
            let other_email = format!("other{index}@example.invalid");
            auth_client_for_server(&mut accounts.pending, &us, (&other_email, 0, ""), device_id);
            accounts.inactive.push(accounts.pending.take().unwrap());
        }

        assert!(accounts.can_login(&us, email).is_ok());
        assert_eq!(
            accounts.can_login(&us, "another@example.invalid"),
            Err(ACCOUNT_LIMIT_REACHED)
        );

        auth_client_for_server(&mut accounts.pending, &us, (email, 0, ""), device_id);
        let replacement = accounts.pending.take().unwrap();
        accounts.activate(replacement);
        assert_eq!(accounts.iter().count(), MAX_ACCOUNTS);
        assert_eq!(accounts.active.as_ref().unwrap().email, email);
        assert_eq!(accounts.active.as_ref().unwrap().state, SESSION_SIGNED_OUT);
    }

    #[test]
    fn account_limit_counts_sessions_and_identifies_duplicates_by_server_and_email() {
        let mut accounts = AuthAccounts::default();
        let us = auth_server(0, "").unwrap();
        let eu = auth_server(1, "").unwrap();
        let device_id = "11111111-1111-4111-8111-111111111111";
        for index in 0..MAX_ACCOUNTS {
            let email = format!("account{index}@example.invalid");
            assert!(accounts.can_login(&us, &email).is_ok());
            auth_client_for_server(&mut accounts.pending, &us, (&email, 0, ""), device_id);
            let account = accounts.pending.take().unwrap();
            if index == 0 {
                accounts.active = Some(account);
                assert_eq!(
                    accounts.can_login(&us, &email),
                    Err(ACCOUNT_ALREADY_SIGNED_IN)
                );
                assert!(accounts.can_login(&eu, &email).is_ok());
            } else {
                accounts.inactive.push(account);
            }
        }
        assert_eq!(
            accounts.can_login(&eu, "extra@example.invalid"),
            Err(ACCOUNT_LIMIT_REACHED)
        );
    }
}
