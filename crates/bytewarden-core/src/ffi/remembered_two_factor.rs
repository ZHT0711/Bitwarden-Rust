//! Remembered 2FA is a server-issued credential, never a locally bypassed check.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const LIFETIME: u64 = 30 * 24 * 60 * 60;
static TOKENS: OnceLock<Mutex<HashMap<String, RememberedToken>>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberedToken {
    version: u8,
    identity: String,
    issued_at: u64,
    expires_at: u64,
    token: String,
}

impl Drop for RememberedToken {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

impl RememberedToken {
    fn valid(&self, identity: &str, now: u64) -> bool {
        self.version == 1
            && self.identity == identity
            && !self.token.is_empty()
            && self.token.len() <= 4096
            && self.expires_at.checked_sub(self.issued_at) == Some(LIFETIME)
            && now >= self.issued_at
            && now < self.expires_at
    }
}

fn tokens() -> &'static Mutex<HashMap<String, RememberedToken>> {
    TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|v| v.as_secs())
}

pub(super) fn identity(server: &super::AuthServer, email: &str, device: &str) -> String {
    // JSON framing avoids collisions from untrusted strings containing newlines.
    serde_json::to_string(&(&server.api_url, &server.identity_url, email, device))
        .expect("strings serialize")
}

pub(super) fn load(identity: &str) -> Option<Zeroizing<String>> {
    let now = now()?;
    #[cfg(windows)]
    if crate::local_cache::is_configured() {
        let encoded = crate::local_cache::load_two_factor(identity)?;
        let record = serde_json::from_slice::<RememberedToken>(&encoded).ok();
        if let Some(record) = record.filter(|record| record.valid(identity, now)) {
            return Some(Zeroizing::new(record.token.clone()));
        }
        clear(identity);
        return None;
    }
    let mut tokens = tokens()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if tokens
        .get(identity)
        .is_some_and(|record| !record.valid(identity, now))
    {
        tokens.remove(identity);
    }
    tokens
        .get(identity)
        .map(|record| Zeroizing::new(record.token.clone()))
}

pub(super) fn save(identity: &str, token: &str) {
    let Some(issued_at) = now() else { return };
    let Some(expires_at) = issued_at.checked_add(LIFETIME) else {
        return;
    };
    let record = RememberedToken {
        version: 1,
        identity: identity.to_owned(),
        issued_at,
        expires_at,
        token: token.to_owned(),
    };
    if !record.valid(identity, issued_at) {
        return;
    }
    #[cfg(windows)]
    if crate::local_cache::is_configured() {
        if let Ok(encoded) = serde_json::to_vec(&record).map(Zeroizing::new) {
            crate::local_cache::save_two_factor(identity, &encoded);
        }
        return;
    }
    tokens()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(identity.to_owned(), record);
}

pub(super) fn clear(identity: &str) {
    tokens()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(identity);
    #[cfg(windows)]
    crate::local_cache::remove_two_factor(identity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_login_response_does_not_serialize_or_debug_log_the_remembered_credential() {
        let response = bw_sdk_core::auth::login::PasswordLoginResponse {
            authenticated: true,
            two_factor_token: Some("server-issued-secret-for-synthetic-test".into()),
            reset_master_password: false,
            force_password_reset: false,
            two_factor: None,
        };
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("server-issued-secret"));
        assert!(!encoded.contains("twoFactorToken"));
        assert!(!format!("{response:?}").contains("server-issued-secret"));
    }

    #[test]
    fn expiration_is_exactly_thirty_days_and_clock_rollback_fails_closed() {
        let record = RememberedToken {
            version: 1,
            identity: "account".into(),
            issued_at: 100,
            expires_at: 100 + LIFETIME,
            token: "synthetic-token".into(),
        };
        assert!(record.valid("account", 100));
        assert!(record.valid("account", 100 + LIFETIME - 1));
        assert!(!record.valid("account", 100 + LIFETIME));
        assert!(!record.valid("account", 99));
        assert!(!record.valid("other account", 101));
    }

    #[test]
    fn malformed_records_cannot_extend_the_remember_period() {
        let mut record = RememberedToken {
            version: 1,
            identity: "account".into(),
            issued_at: 100,
            expires_at: 100 + LIFETIME + 1,
            token: "synthetic-token".into(),
        };
        assert!(!record.valid("account", 101));
        record.expires_at -= 1;
        record.token.clear();
        assert!(!record.valid("account", 101));
    }

    #[cfg(windows)]
    #[test]
    fn persisted_expiry_and_binding_are_enforced_on_the_actual_load_path() {
        // Isolate the process-wide Windows storage configuration from other tests.
        const GUARD: &str = "BW_SYNTHETIC_REMEMBER_STORAGE_CHILD";
        if std::env::var_os(GUARD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ffi::remembered_two_factor::tests::persisted_expiry_and_binding_are_enforced_on_the_actual_load_path"])
                .env(GUARD, "1")
                .output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
            return;
        }
        let root = tempfile::tempdir().unwrap();
        assert!(crate::local_cache::set_root(root.path().to_str().unwrap()));
        let identity = "synthetic remembered credential";
        save(identity, "server-issued-synthetic-token");
        assert_eq!(
            load(identity).unwrap().as_str(),
            "server-issued-synthetic-token"
        );
        assert!(load("another account").is_none());
        let encoded = crate::local_cache::load_two_factor(identity).unwrap();
        let mut record: RememberedToken = serde_json::from_slice(&encoded).unwrap();
        record.issued_at = now().unwrap() - LIFETIME;
        record.expires_at = record.issued_at + LIFETIME;
        assert!(crate::local_cache::save_two_factor(
            identity,
            &serde_json::to_vec(&record).unwrap()
        ));
        assert!(load(identity).is_none());
        assert!(crate::local_cache::load_two_factor(identity).is_none());
        save(identity, "fresh-server-token");
        let mut record: RememberedToken =
            serde_json::from_slice(&crate::local_cache::load_two_factor(identity).unwrap())
                .unwrap();
        record.identity = "a different account".into();
        assert!(crate::local_cache::save_two_factor(
            identity,
            &serde_json::to_vec(&record).unwrap()
        ));
        assert!(load(identity).is_none());
    }

    #[test]
    fn remembered_credentials_are_bound_to_both_endpoints_email_and_device() {
        let us = super::super::auth_server(0, "").unwrap();
        let eu = super::super::auth_server(1, "").unwrap();
        let a = identity(&us, "a@example.invalid", "device-a");
        assert_ne!(a, identity(&eu, "a@example.invalid", "device-a"));
        assert_ne!(a, identity(&us, "b@example.invalid", "device-a"));
        assert_ne!(a, identity(&us, "a@example.invalid", "device-b"));
        let mut split = us.clone();
        split.identity_url = eu.identity_url;
        assert_ne!(a, identity(&split, "a@example.invalid", "device-a"));
    }
}
