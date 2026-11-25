//! Cryptographic PIN protection for the Bitwarden user key.
//! Both session-only and persistent PIN enrollments wrap the user key inside a
//! `SecretProtectedKeyEnvelope` sealed with an Argon2-derived secret bound to the
//! account, server, and key wrapper material.
use std::{
    ffi::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
};

use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use bitwarden_crypto::safe::{
    HighEntropySecret, HighEntropySecretSource, SecretProtectedKeyEnvelope,
    SecretProtectedKeyEnvelopeNamespace,
};
use bitwarden_sensitive_value::{Sensitive, SensitiveSlice};
use bw_sdk_core::key_management::SymmetricKeySlotId;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use super::{
    auth_client, auth_request, complete_vault_unlock_raw, local_cache_identity, read_utf8,
    vault_error, vault_success, AuthSession, PersistedUnlockData, SESSION_LOCKED, SESSION_UNLOCKED,
};
use crate::local_cache;

const NAMESPACE: SecretProtectedKeyEnvelopeNamespace =
    SecretProtectedKeyEnvelopeNamespace::DesktopBiometricUnlock;
const DOMAIN: &[u8] = b"Bytewarden/PinVaultUnlock/v1\0";
pub(super) const MAX_ATTEMPTS: u8 = 5;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::ffi) struct PinEnrollment {
    version: u8,
    binding: [u8; 32],
    salt: [u8; 32],
    envelope: Vec<u8>,
    persistent: bool,
    failed_attempts: u8,
}

impl Drop for PinEnrollment {
    fn drop(&mut self) {
        self.binding.zeroize();
        self.salt.zeroize();
        self.envelope.zeroize();
    }
}

struct PinSecret(Zeroizing<[u8; 32]>);

impl HighEntropySecretSource for PinSecret {
    fn provide_high_entropy_bytes(&self) -> SensitiveSlice<'_> {
        Sensitive::from(self.0.as_slice())
    }
}

fn binding(session: &AuthSession) -> Result<[u8; 32], &'static str> {
    let unlock = session.unlock_data.as_ref().ok_or("unlockUnavailable")?;
    let encoded = serde_json::to_vec(&PersistedUnlockData::from(unlock)).map_err(|_| "internal")?;
    let mut digest = Sha256::new();
    digest.update(DOMAIN);
    digest.update(local_cache::account_digest(&local_cache_identity(session)));
    digest.update(session.account_id.as_bytes());
    digest.update(encoded);
    Ok(digest.finalize().into())
}

fn derive_secret(
    pin: &str,
    binding: &[u8; 32],
    salt: &[u8; 32],
) -> Result<HighEntropySecret, &'static str> {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(binding);
    hasher.update(salt);
    let raw_salt: [u8; 32] = hasher.finalize().into();
    let salt_string = SaltString::encode_b64(&raw_salt[..16]).map_err(|_| "internal")?;
    let hash = Argon2::default()
        .hash_password(pin.as_bytes(), &salt_string)
        .map_err(|_| "internal")?;
    let output = hash.hash.ok_or("internal")?;
    let bytes = output.as_bytes();
    if bytes.len() < 32 {
        return Err("internal");
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(&bytes[..32]);
    Ok(HighEntropySecret::from(PinSecret(key)))
}

fn load_valid(session: &AuthSession) -> Option<PinEnrollment> {
    let expected = binding(session).ok()?;
    if let Some(enrollment) = session.pin_enrollment.as_ref() {
        return (enrollment.version == 1
            && !enrollment.persistent
            && enrollment.binding == expected
            && enrollment.failed_attempts < MAX_ATTEMPTS)
            .then(|| enrollment.clone());
    }
    if !session.vault.has_cached_snapshot() {
        return None;
    }
    let encoded = local_cache::load_pin(&local_cache_identity(session))?;
    let enrollment: PinEnrollment = serde_json::from_slice(&encoded).ok()?;
    (enrollment.version == 1
        && enrollment.persistent
        && enrollment.binding == expected
        && enrollment.failed_attempts < MAX_ATTEMPTS)
        .then_some(enrollment)
}

pub(in crate::ffi) fn status(session: &AuthSession) -> (bool, bool) {
    match load_valid(session) {
        Some(enrollment) => (true, enrollment.persistent),
        None => (false, false),
    }
}

pub(in crate::ffi) fn clear_session(session: &mut AuthSession) {
    session.pin_enrollment = None;
}

unsafe fn account_id(value: *const c_char) -> Result<Uuid, &'static str> {
    let id = unsafe { read_utf8(value) }.ok_or("validation")?;
    Uuid::parse_str(&id).map_err(|_| "validation")
}

fn run_boolean(work: impl FnOnce() -> Result<(), &'static str>) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| match work() {
        Ok(()) => vault_success(&true),
        Err(error) => vault_error(error),
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_pin_enable(
    account: *const c_char,
    password: *const c_char,
    pin: *const c_char,
    require_mp_on_restart: u8,
    request_id: u64,
) -> *mut c_char {
    run_boolean(|| {
        let id = unsafe { account_id(account) }?;
        let password = Zeroizing::new(
            unsafe { read_utf8(password) }
                .filter(|value| !value.is_empty())
                .ok_or("wrongPassword")?,
        );
        let pin = Zeroizing::new(unsafe { read_utf8(pin) }.ok_or("validation")?);
        if pin.chars().count() < 4 || pin.len() > 256 {
            return Err("validation");
        }
        let persistent = require_mp_on_restart == 0;
        let (identity, expected_binding) = {
            let guard = auth_client()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session = guard
                .active
                .as_ref()
                .filter(|s| s.account_id == id && s.state == SESSION_UNLOCKED)
                .ok_or("unlockUnavailable")?;
            if !auth_request::is_current(request_id) {
                return Err("cancelled");
            }
            let keys = session.unlock_data.as_ref().ok_or("unlockUnavailable")?;
            if !session
                .client
                .internal
                .verify_desktop_user_key(&password, &keys.master_password)
            {
                return Err("wrongPassword");
            }
            let identity = local_cache_identity(session);
            if persistent
                && (!local_cache::is_configured()
                    || !local_cache::has_persisted_snapshot(&identity))
            {
                return Err("localStorage");
            }
            (identity, binding(session)?)
        };
        drop(password);

        let mut salt = [0_u8; 32];
        OsRng.try_fill_bytes(&mut salt).map_err(|_| "internal")?;
        let secret = derive_secret(&pin, &expected_binding, &salt)?;
        drop(pin);

        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = guard
            .active
            .as_mut()
            .filter(|s| s.account_id == id && s.state == SESSION_UNLOCKED)
            .ok_or("cancelled")?;
        if !auth_request::is_current(request_id) || binding(session)? != expected_binding {
            return Err("cancelled");
        }
        let envelope = SecretProtectedKeyEnvelope::seal(
            SymmetricKeySlotId::User,
            &secret,
            NAMESPACE,
            &session.client.internal.get_key_store().context(),
        )
        .map_err(|_| "internal")?;
        let enrollment = PinEnrollment {
            version: 1,
            binding: expected_binding,
            salt,
            envelope: (&envelope).into(),
            persistent,
            failed_attempts: 0,
        };
        if persistent {
            let encoded = Zeroizing::new(serde_json::to_vec(&enrollment).map_err(|_| "internal")?);
            if !local_cache::save_pin(&identity, &encoded) {
                return Err("localStorage");
            }
            session.pin_enrollment = None;
        } else {
            if !local_cache::remove_pin(&identity) {
                return Err("localStorage");
            }
            session.pin_enrollment = Some(enrollment);
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn bw_pin_disable(account: *const c_char) -> *mut c_char {
    run_boolean(|| {
        let id = unsafe { account_id(account) }?;
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = guard
            .active
            .as_mut()
            .filter(|s| s.account_id == id)
            .ok_or("accountNotFound")?;
        session.pin_enrollment = None;
        if !local_cache::remove_pin(&local_cache_identity(session)) {
            return Err("localStorage");
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn bw_pin_unlock(
    account: *const c_char,
    pin: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let id = match unsafe { account_id(account) } {
            Ok(id) => id,
            Err(error) => return vault_error(error),
        };
        let pin = Zeroizing::new(match unsafe { read_utf8(pin) } {
            Some(pin) if !pin.is_empty() => pin,
            _ => return vault_error("wrongPin"),
        });

        let (identity, expected_binding, enrollment) = {
            let guard = auth_client()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(session) = guard
                .active
                .as_ref()
                .filter(|s| s.account_id == id && s.state == SESSION_LOCKED)
            else {
                return vault_error("unlockUnavailable");
            };
            let Some(enrollment) = load_valid(session) else {
                return vault_error("pinNotEnabled");
            };
            let Ok(expected_binding) = binding(session) else {
                return vault_error("unlockUnavailable");
            };
            (local_cache_identity(session), expected_binding, enrollment)
        };

        let secret = match derive_secret(&pin, &expected_binding, &enrollment.salt) {
            Ok(secret) => secret,
            Err(error) => return vault_error(error),
        };
        drop(pin);

        let result = complete_vault_unlock_raw(request_id, |session, keys| {
            if session.account_id != id || binding(session).ok() != Some(expected_binding) {
                return Err("cancelled".to_owned());
            }
            let current = load_valid(session).ok_or_else(|| "pinNotEnabled".to_owned())?;
            if current.salt != enrollment.salt || current.envelope != enrollment.envelope {
                return Err("cancelled".to_owned());
            }
            let envelope = SecretProtectedKeyEnvelope::try_from(&enrollment.envelope)
                .map_err(|_| "pinRevoked".to_owned())?;
            session
                .client
                .internal
                .initialize_user_crypto_from_desktop_envelope(
                    &envelope,
                    &secret,
                    keys.account.clone(),
                )
                .map_err(|_| "wrongPin".to_owned())
        });

        match result {
            Ok(()) => {
                if enrollment.failed_attempts > 0 {
                    let mut guard = auth_client()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(session) = guard
                        .active
                        .as_mut()
                        .filter(|s| s.account_id == id && s.state == SESSION_UNLOCKED)
                    {
                        if let Some(mut updated) = load_valid(session) {
                            updated.failed_attempts = 0;
                            if updated.persistent {
                                if let Ok(encoded) =
                                    serde_json::to_vec(&updated).map(Zeroizing::new)
                                {
                                    let _ = local_cache::save_pin(&identity, &encoded);
                                }
                            } else {
                                session.pin_enrollment = Some(updated);
                            }
                        }
                    }
                }
                vault_success(&true)
            }
            Err(code) if code == "wrongPin" => {
                let mut guard = auth_client()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(session) = guard.active.as_mut().filter(|s| s.account_id == id) else {
                    return vault_error("wrongPin");
                };
                let Some(mut updated) = load_valid(session) else {
                    session.pin_enrollment = None;
                    let _ = local_cache::remove_pin(&identity);
                    return vault_error("pinRevoked");
                };
                updated.failed_attempts = updated.failed_attempts.saturating_add(1);
                if updated.failed_attempts >= MAX_ATTEMPTS {
                    session.pin_enrollment = None;
                    let _ = local_cache::remove_pin(&identity);
                    return vault_error("pinRevoked");
                }
                if updated.persistent {
                    if let Ok(encoded) = serde_json::to_vec(&updated).map(Zeroizing::new) {
                        if !local_cache::save_pin(&identity, &encoded) {
                            session.pin_enrollment = None;
                            let _ = local_cache::remove_pin(&identity);
                            return vault_error("pinRevoked");
                        }
                    }
                } else {
                    session.pin_enrollment = Some(updated);
                }
                vault_error("wrongPin")
            }
            Err(code) if code == "pinRevoked" => {
                let mut guard = auth_client()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(session) = guard.active.as_mut().filter(|s| s.account_id == id) {
                    session.pin_enrollment = None;
                }
                let _ = local_cache::remove_pin(&identity);
                vault_error("pinRevoked")
            }
            Err(code) => vault_error(&code),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{CStr, CString},
        sync::{atomic::Ordering, Arc},
    };

    use bitwarden_crypto::{Decryptable, PrimitiveEncryptable};
    use bw_sdk_core::{
        auth::login::PasswordLoginUnlockData,
        client::test_accounts::test_bitwarden_com_account,
        key_management::{
            account_cryptographic_state::WrappedAccountCryptographicState, MasterPasswordUnlockData,
        },
        Client,
    };

    use super::*;
    use crate::{
        ffi::{
            auth_server, bw_auth_session_state, bw_string_free, runtime, DeviceLoginMiddleware,
            PersistedUnlockData, SESSION_STATE,
        },
        vault::VaultCache,
    };

    unsafe fn take_ffi_json(ptr: *mut c_char) -> serde_json::Value {
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("valid UTF-8")
            .to_owned();
        unsafe { bw_string_free(ptr) };
        serde_json::from_str(&text).expect("valid JSON")
    }

    #[test]
    fn pin_enrollment_serde_rejects_unknown_fields_and_roundtrips() {
        let enrollment = PinEnrollment {
            version: 1,
            binding: [7_u8; 32],
            salt: [9_u8; 32],
            envelope: vec![1, 2, 3, 4],
            persistent: true,
            failed_attempts: 2,
        };
        let encoded = serde_json::to_vec(&enrollment).unwrap();
        let decoded: PinEnrollment = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.binding, [7_u8; 32]);
        assert_eq!(decoded.salt, [9_u8; 32]);
        assert_eq!(decoded.envelope, vec![1, 2, 3, 4]);
        assert!(decoded.persistent);
        assert_eq!(decoded.failed_attempts, 2);

        let extra_field = br#"{"version":1,"binding":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"salt":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"envelope":[],"persistent":false,"failed_attempts":0,"extra":1}"#;
        assert!(serde_json::from_slice::<PinEnrollment>(extra_field).is_err());
    }

    #[tokio::test]
    async fn derive_secret_and_envelope_bind_to_pin_binding_and_salt() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let store = client.internal.get_key_store();
        let binding = [0xAA_u8; 32];
        let salt = [0x55_u8; 32];
        let other_binding = [0xAB_u8; 32];
        let other_salt = [0x56_u8; 32];

        let secret = derive_secret("1234", &binding, &salt).unwrap();
        let (account_state, envelope_bytes, cipher) = {
            let mut ctx = store.context();
            let state = WrappedAccountCryptographicState::get_from_key_store(&ctx).unwrap();
            let cipher = "synthetic pin payload"
                .to_owned()
                .encrypt(&mut ctx, SymmetricKeySlotId::User)
                .unwrap();
            let envelope = SecretProtectedKeyEnvelope::seal(
                SymmetricKeySlotId::User,
                &secret,
                NAMESPACE,
                &ctx,
            )
            .unwrap();
            (state, Vec::from(&envelope), cipher)
        };
        store.clear();
        let envelope = SecretProtectedKeyEnvelope::try_from(&envelope_bytes).unwrap();

        for bad in [
            derive_secret("1235", &binding, &salt).unwrap(),
            derive_secret("1234", &other_binding, &salt).unwrap(),
            derive_secret("1234", &binding, &other_salt).unwrap(),
        ] {
            assert!(client
                .internal
                .initialize_user_crypto_from_desktop_envelope(
                    &envelope,
                    &bad,
                    account_state.clone()
                )
                .is_err());
            assert!(!store.context().has_symmetric_key(SymmetricKeySlotId::User));
        }

        let same_secret = derive_secret("1234", &binding, &salt).unwrap();
        client
            .internal
            .initialize_user_crypto_from_desktop_envelope(&envelope, &same_secret, account_state)
            .unwrap();
        let decrypted: String = cipher
            .decrypt(&mut store.context(), SymmetricKeySlotId::User)
            .unwrap();
        assert_eq!(decrypted, "synthetic pin payload");
    }

    #[test]
    fn pin_ffi_validation_and_lockout_revoke_after_five_wrong_attempts() {
        struct SessionReset;
        impl Drop for SessionReset {
            fn drop(&mut self) {
                crate::ffi::accounts::bw_auth_logout_all();
            }
        }

        let _guard = crate::ffi::AUTH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _reset = SessionReset;
        crate::ffi::accounts::bw_auth_logout_all();

        let bad_id = CString::new("not-a-uuid").unwrap();
        let ok_id = uuid::Uuid::new_v4();
        let ok_id_c = CString::new(ok_id.to_string()).unwrap();
        let master = CString::new("wrong-master-password").unwrap();
        let short_pin = CString::new("123").unwrap();
        let good_pin = CString::new("1234").unwrap();
        let wrong_pin = CString::new("9999").unwrap();

        let req = auth_request::begin();
        let invalid_uuid = unsafe {
            take_ffi_json(bw_pin_enable(
                bad_id.as_ptr(),
                master.as_ptr(),
                good_pin.as_ptr(),
                1,
                req,
            ))
        };
        assert_eq!(invalid_uuid["error"], "validation");

        let req = auth_request::begin();
        let too_short = unsafe {
            take_ffi_json(bw_pin_enable(
                ok_id_c.as_ptr(),
                master.as_ptr(),
                short_pin.as_ptr(),
                1,
                req,
            ))
        };
        assert_eq!(too_short["error"], "validation");

        let runtime = runtime().expect("runtime");
        let client = runtime.block_on(Client::init_test_account(test_bitwarden_com_account()));
        let account_state = WrappedAccountCryptographicState::get_from_key_store(
            &client.internal.get_key_store().context(),
        )
        .unwrap();
        let email = "pin-test@example.invalid";
        let master_password = MasterPasswordUnlockData::derive(
            "synthetic-pin-test-password",
            &bitwarden_crypto::Kdf::PBKDF2 {
                iterations: 600_000.try_into().unwrap(),
            },
            email,
            SymmetricKeySlotId::User,
            &client.internal.get_key_store().context(),
        )
        .unwrap();
        let server = auth_server(0, "").unwrap();
        let device_identifier = "11111111-1111-4111-8111-111111111111".to_owned();
        let middleware = Arc::new(DeviceLoginMiddleware::new(device_identifier.clone()));
        let mut session = AuthSession {
            account_id: ok_id,
            email: email.to_owned(),
            server_mode: 0,
            server_url: String::new(),
            state: SESSION_UNLOCKED,
            server,
            device_identifier,
            client,
            device_login_middleware: middleware,
            unlock_data: Some(PasswordLoginUnlockData::from(PersistedUnlockData {
                master_password,
                account: account_state,
            })),
            vault: VaultCache::default(),
            open_detail: None,
            offline_only: false,
            send_write_uncertain: false,
            pin_enrollment: None,
        };
        let session_binding = binding(&session).unwrap();
        let salt = [0x33_u8; 32];
        let secret = derive_secret("1234", &session_binding, &salt).unwrap();
        let envelope = SecretProtectedKeyEnvelope::seal(
            SymmetricKeySlotId::User,
            &secret,
            NAMESPACE,
            &session.client.internal.get_key_store().context(),
        )
        .unwrap();
        let enrollment = PinEnrollment {
            version: 1,
            binding: session_binding,
            salt,
            envelope: (&envelope).into(),
            persistent: false,
            failed_attempts: 0,
        };
        session.pin_enrollment = Some(enrollment.clone());
        {
            let mut accounts = auth_client().lock().unwrap();
            accounts.active = Some(session);
            SESSION_STATE.store(SESSION_UNLOCKED, Ordering::Release);
        }

        let req = auth_request::begin();
        let wrong_mp = unsafe {
            take_ffi_json(bw_pin_enable(
                ok_id_c.as_ptr(),
                master.as_ptr(),
                good_pin.as_ptr(),
                1,
                req,
            ))
        };
        assert_eq!(wrong_mp["error"], "wrongPassword");

        {
            let mut accounts = auth_client().lock().unwrap();
            let active = accounts.active.as_mut().unwrap();
            assert_eq!(status(active), (true, false));
            assert!(active.clear_vault_keys());
            active.state = SESSION_LOCKED;
            SESSION_STATE.store(SESSION_LOCKED, Ordering::Release);
        }

        for attempt in 1..MAX_ATTEMPTS {
            let req = auth_request::begin();
            let res =
                unsafe { take_ffi_json(bw_pin_unlock(ok_id_c.as_ptr(), wrong_pin.as_ptr(), req)) };
            assert_eq!(res["error"], "wrongPin", "attempt {attempt}");
            let accounts = auth_client().lock().unwrap();
            assert_eq!(status(accounts.active.as_ref().unwrap()), (true, false));
        }

        let req = auth_request::begin();
        let lockout =
            unsafe { take_ffi_json(bw_pin_unlock(ok_id_c.as_ptr(), wrong_pin.as_ptr(), req)) };
        assert_eq!(lockout["error"], "pinRevoked");
        {
            let mut accounts = auth_client().lock().unwrap();
            let active = accounts.active.as_mut().unwrap();
            assert_eq!(status(active), (false, false));
            active.pin_enrollment = Some(enrollment);
        }

        let req = auth_request::begin();
        let unlocked =
            unsafe { take_ffi_json(bw_pin_unlock(ok_id_c.as_ptr(), good_pin.as_ptr(), req)) };
        assert_eq!(
            unlocked["ok"], true,
            "unexpected PIN unlock failure: {unlocked}"
        );
        assert_eq!(bw_auth_session_state(), SESSION_UNLOCKED);

        let disabled = unsafe { take_ffi_json(bw_pin_disable(ok_id_c.as_ptr())) };
        assert_eq!(disabled["ok"], true);
        let accounts = auth_client().lock().unwrap();
        assert_eq!(status(accounts.active.as_ref().unwrap()), (false, false));
    }
}
