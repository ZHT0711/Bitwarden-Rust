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

const MAX_ACCOUNTS: usize = 5;

#[derive(Default)]
pub(super) struct AuthAccounts {
    pub(super) active: Option<AuthSession>,
    pub(super) pending: Option<AuthSession>,
    inactive: Vec<AuthSession>,
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
                })
                .collect::<Vec<_>>(),
        )
    }

    pub(super) fn can_login(&self, server: &AuthServer, email: &str) -> Result<(), u32> {
        if self
            .iter()
            .any(|session| session.server == *server && session.email == email)
        {
            return Err(ACCOUNT_ALREADY_SIGNED_IN);
        }
        if self.inactive.len() + usize::from(self.active.is_some()) >= MAX_ACCOUNTS {
            return Err(ACCOUNT_LIMIT_REACHED);
        }
        Ok(())
    }

    pub(super) fn activate(&mut self, session: AuthSession) {
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

    pub(super) fn remove_active(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.clear_vault_keys();
        }
        self.active = self.inactive.pop();
        clear_active_passkeys();
        self.publish_state();
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

    fn timeout(&mut self, action: AccountTimeout) {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.account_id == action.id)
        {
            if action.log_out {
                self.remove_active();
            } else {
                self.lock_active();
            }
            return;
        }
        if let Some(index) = self
            .inactive
            .iter()
            .position(|session| session.account_id == action.id)
        {
            let session = &mut self.inactive[index];
            if action.log_out || !session.clear_vault_keys() {
                let mut removed = self.inactive.remove(index);
                removed.clear_vault_keys();
            } else {
                session.state = SESSION_LOCKED;
            }
        }
    }
}

fn clear_active_passkeys() {
    #[cfg(windows)]
    let _ = catch_unwind(AssertUnwindSafe(|| {
        crate::passkey_provider::cancel("");
        crate::passkey_provider::clear_metadata();
    }));
}

fn sessions() -> MutexGuard<'static, AuthAccounts> {
    auth_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
pub extern "C" fn bw_auth_logout() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        let mut accounts = sessions();
        accounts.pending = None;
        accounts.remove_active();
    }));
}

#[no_mangle]
pub extern "C" fn bw_auth_logout_all() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        #[cfg(windows)]
        crate::passkey_provider::cancel("");
        let mut accounts = sessions();
        if let Some(mut active) = accounts.active.take() {
            active.clear_vault_keys();
        }
        for mut inactive in accounts.inactive.drain(..) {
            inactive.clear_vault_keys();
        }
        accounts.pending = None;
        clear_active_passkeys();
        accounts.publish_state();
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
        for action in actions {
            accounts.timeout(action);
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
