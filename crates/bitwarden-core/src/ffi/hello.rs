//! Windows Hello signing protects the user key even when the vault is closed. A UI consent
//! result alone is never accepted as proof or as a substitute for the master password.
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
use windows::{
    core::{Interface, HSTRING},
    Security::{
        Credentials::{KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus},
        Cryptography::CryptographicBuffer,
    },
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::WinRT::{IBufferByteAccess, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    },
};
use zeroize::{Zeroize, Zeroizing};

use super::{
    auth_client, auth_request, complete_vault_unlock, local_cache_identity, read_utf8, runtime,
    vault_error, vault_success, AuthSession, PersistedUnlockData, RequestError, SESSION_LOCKED,
    SESSION_UNLOCKED,
};
use crate::local_cache;
use std::{
    ffi::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
};

const NAMESPACE: SecretProtectedKeyEnvelopeNamespace =
    SecretProtectedKeyEnvelopeNamespace::DesktopBiometricUnlock;
const DOMAIN: &[u8] = b"BitwardenRust/HelloVaultUnlock/v1\0";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Enrollment {
    version: u8,
    binding: [u8; 32],
    challenge: [u8; 32],
    envelope: Vec<u8>,
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

fn load(session: &AuthSession) -> Result<Enrollment, &'static str> {
    let identity = local_cache_identity(session);
    let encoded = local_cache::load_hello(&identity).ok_or("helloNotEnabled")?;
    let enrollment: Enrollment = serde_json::from_slice(&encoded).map_err(|_| "helloFailed")?;
    if enrollment.version != 1 || enrollment.binding != binding(session)? {
        return Err("helloFailed");
    }
    Ok(enrollment)
}

pub(super) fn is_enabled(session: &AuthSession) -> bool {
    session.vault.has_cached_snapshot() && load(session).is_ok()
}

struct Apartment(bool);
impl Apartment {
    fn initialize() -> Result<Self, &'static str> {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self(true)),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self(false)),
            Err(_) => Err("helloUnavailable"),
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { RoUninitialize() };
        }
    }
}

fn status(status: KeyCredentialStatus) -> Result<(), &'static str> {
    match status {
        KeyCredentialStatus::Success => Ok(()),
        KeyCredentialStatus::UserCanceled | KeyCredentialStatus::UserPrefersPassword => {
            Err("helloCancelled")
        }
        KeyCredentialStatus::NotFound | KeyCredentialStatus::SecurityDeviceLocked => {
            Err("helloUnavailable")
        }
        _ => Err("helloFailed"),
    }
}

struct SignatureSecret(Zeroizing<[u8; 32]>);
impl HighEntropySecretSource for SignatureSecret {
    fn provide_high_entropy_bytes(&self) -> SensitiveSlice<'_> {
        Sensitive::from(self.0.as_slice())
    }
}

fn challenge_message(binding: &[u8; 32], challenge: &[u8; 32]) -> Vec<u8> {
    [DOMAIN, binding, challenge].concat()
}

struct CancelOnDrop<F: FnOnce()>(Option<F>);
impl<F: FnOnce()> Drop for CancelOnDrop<F> {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            cancel();
        }
    }
}

async fn sign(
    identity: &str,
    binding: &[u8; 32],
    challenge: &[u8; 32],
    enroll: bool,
) -> Result<HighEntropySecret, &'static str> {
    let digest = local_cache::account_digest(identity);
    let name = HSTRING::from(format!(
        "BitwardenRust.VaultUnlock.v1.{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    let mut opened = KeyCredentialManager::OpenAsync(&name)
        .map_err(|_| "helloUnavailable")?
        .await
        .map_err(|_| "helloUnavailable")?;
    if opened.Status().map_err(|_| "helloUnavailable")? == KeyCredentialStatus::NotFound && enroll {
        if !KeyCredentialManager::IsSupportedAsync()
            .map_err(|_| "helloUnavailable")?
            .await
            .map_err(|_| "helloUnavailable")?
        {
            return Err("helloUnavailable");
        }
        let op = KeyCredentialManager::RequestCreateAsync(
            &name,
            KeyCredentialCreationOption::FailIfExists,
        )
        .map_err(|_| "helloFailed")?;
        let mut cancel = CancelOnDrop(Some(|| {
            let _ = op.Cancel();
        }));
        opened = op.clone().await.map_err(|_| "helloFailed")?;
        cancel.0 = None;
        if opened.Status().map_err(|_| "helloFailed")?
            == KeyCredentialStatus::CredentialAlreadyExists
        {
            opened = KeyCredentialManager::OpenAsync(&name)
                .map_err(|_| "helloUnavailable")?
                .await
                .map_err(|_| "helloUnavailable")?;
        }
    }
    status(opened.Status().map_err(|_| "helloUnavailable")?)?;
    let credential = opened.Credential().map_err(|_| "helloFailed")?;
    let message = CryptographicBuffer::CreateFromByteArray(&challenge_message(binding, challenge))
        .map_err(|_| "helloFailed")?;
    let op = credential
        .RequestSignAsync(&message)
        .map_err(|_| "helloFailed")?;
    let mut cancel = CancelOnDrop(Some(|| {
        let _ = op.Cancel();
    }));
    let signed = op.clone().await.map_err(|_| "helloFailed")?;
    cancel.0 = None;
    status(signed.Status().map_err(|_| "helloFailed")?)?;
    let buffer = signed.Result().map_err(|_| "helloFailed")?;
    let len = buffer.Length().map_err(|_| "helloFailed")? as usize;
    if !(128..=1024).contains(&len) {
        return Err("helloFailed");
    }
    let access: IBufferByteAccess = buffer.cast().map_err(|_| "helloFailed")?;
    let pointer = unsafe { access.Buffer() }.map_err(|_| "helloFailed")?;
    if pointer.is_null() {
        return Err("helloFailed");
    }
    // The buffer is owned by the local Windows result and remains alive while we use its bytes.
    let bytes = unsafe { std::slice::from_raw_parts_mut(pointer, len) };
    let prf = Zeroizing::new(Sha256::digest(&*bytes).into());
    bytes.zeroize();
    Ok(HighEntropySecret::from(SignatureSecret(prf)))
}

fn prompt(
    identity: &str,
    binding: &[u8; 32],
    challenge: &[u8; 32],
    enroll: bool,
    request_id: u64,
) -> Result<HighEntropySecret, &'static str> {
    let _apartment = Apartment::initialize()?;
    let runtime = runtime().ok_or("internal")?;
    match runtime.block_on(auth_request::run(
        request_id,
        sign(identity, binding, challenge, enroll),
    )) {
        Ok(result) => result,
        Err(RequestError::Cancelled) => Err("cancelled"),
        Err(RequestError::TimedOut) => Err("helloTimedOut"),
    }
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
pub unsafe extern "C" fn bw_hello_enable(
    account: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    run_boolean(|| {
        let id = unsafe { account_id(account) }?;
        let password = Zeroizing::new(
            unsafe { read_utf8(password) }
                .filter(|password| !password.is_empty())
                .ok_or("wrongPassword")?,
        );
        let (identity, expected) = {
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
            if !local_cache::is_configured() {
                return Err("localStorage");
            }
            if !session.vault.has_cached_snapshot() {
                return Err("notSynced");
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
            if !local_cache::has_persisted_snapshot(&identity) {
                return Err("localStorage");
            }
            (identity, binding(session)?)
        };
        drop(password);
        let mut challenge = [0_u8; 32];
        OsRng
            .try_fill_bytes(&mut challenge)
            .map_err(|_| "internal")?;
        let secret = prompt(&identity, &expected, &challenge, true, request_id)?;
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = guard
            .active
            .as_ref()
            .filter(|s| s.account_id == id && s.state == SESSION_UNLOCKED)
            .ok_or("cancelled")?;
        if !auth_request::is_current(request_id) || binding(session)? != expected {
            return Err("cancelled");
        }
        let envelope = SecretProtectedKeyEnvelope::seal(
            SymmetricKeySlotId::User,
            &secret,
            NAMESPACE,
            &session.client.internal.get_key_store().context(),
        )
        .map_err(|_| "helloFailed")?;
        let encoded = serde_json::to_vec(&Enrollment {
            version: 1,
            binding: expected,
            challenge,
            envelope: (&envelope).into(),
        })
        .map_err(|_| "internal")?;
        if !local_cache::save_hello(&identity, &encoded) {
            return Err("localStorage");
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn bw_hello_disable(account: *const c_char) -> *mut c_char {
    run_boolean(|| {
        let id = unsafe { account_id(account) }?;
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = guard
            .active
            .as_ref()
            .filter(|s| s.account_id == id)
            .ok_or("accountNotFound")?;
        // Invalidate an in-flight prompt before revoking; it must not save a stale enrollment.
        auth_request::begin();
        if !local_cache::remove_hello(&local_cache_identity(session)) {
            return Err("localStorage");
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn bw_hello_unlock(account: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let prepare = || {
            let id = unsafe { account_id(account) }?;
            let (identity, enrollment) = {
                let guard = auth_client()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let session = guard
                    .active
                    .as_ref()
                    .filter(|s| s.account_id == id && s.state == SESSION_LOCKED)
                    .ok_or("unlockUnavailable")?;
                (local_cache_identity(session), load(session)?)
            };
            let secret = prompt(
                &identity,
                &enrollment.binding,
                &enrollment.challenge,
                false,
                request_id,
            )?;
            Ok((id, enrollment, secret))
        };
        let (id, enrollment, secret) = match prepare() {
            Ok(value) => value,
            Err(error) => return vault_error(error),
        };
        complete_vault_unlock(request_id, |session, keys| {
            if session.account_id != id || binding(session)? != enrollment.binding {
                return Err("cancelled".to_owned());
            }
            let current = load(session)?;
            if current.challenge != enrollment.challenge || current.envelope != enrollment.envelope
            {
                return Err("cancelled".to_owned());
            }
            let envelope = SecretProtectedKeyEnvelope::try_from(&enrollment.envelope)
                .map_err(|_| "helloFailed".to_owned())?;
            session
                .client
                .internal
                .initialize_user_crypto_from_desktop_envelope(
                    &envelope,
                    &secret,
                    keys.account.clone(),
                )
                .map_err(|_| "helloFailed".to_owned())
        })
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitwarden_crypto::{Decryptable, PrimitiveEncryptable};
    use bw_sdk_core::{
        client::test_accounts::test_bitwarden_com_account,
        key_management::account_cryptographic_state::WrappedAccountCryptographicState, Client,
    };

    #[tokio::test]
    async fn correct_secret_restores_account_keys_without_releasing_raw_key_material() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let store = client.internal.get_key_store();
        let correct = HighEntropySecret::make(32).unwrap();
        let wrong = HighEntropySecret::make(32).unwrap();
        let (account_state, envelope, data) = {
            let mut ctx = store.context();
            let state = WrappedAccountCryptographicState::get_from_key_store(&ctx).unwrap();
            let data = "synthetic vault field"
                .to_owned()
                .encrypt(&mut ctx, SymmetricKeySlotId::User)
                .unwrap();
            let envelope = SecretProtectedKeyEnvelope::seal(
                SymmetricKeySlotId::User,
                &correct,
                NAMESPACE,
                &ctx,
            )
            .unwrap();
            (state, Vec::from(&envelope), data)
        };
        store.clear();
        let envelope = SecretProtectedKeyEnvelope::try_from(&envelope).unwrap();
        assert!(client
            .internal
            .initialize_user_crypto_from_desktop_envelope(&envelope, &wrong, account_state.clone())
            .is_err());
        assert!(!store.context().has_symmetric_key(SymmetricKeySlotId::User));
        client
            .internal
            .initialize_user_crypto_from_desktop_envelope(&envelope, &correct, account_state)
            .unwrap();
        let plaintext: String = data
            .decrypt(&mut store.context(), SymmetricKeySlotId::User)
            .unwrap();
        assert_eq!(plaintext, "synthetic vault field");
    }

    #[test]
    fn signed_challenge_is_versioned_and_bound_to_enrollment_and_account() {
        let message = challenge_message(&[1; 32], &[2; 32]);
        assert!(message.starts_with(DOMAIN));
        assert_ne!(message, challenge_message(&[3; 32], &[2; 32]));
        assert_ne!(message, challenge_message(&[1; 32], &[3; 32]));
        assert_eq!(
            status(KeyCredentialStatus::UserCanceled),
            Err("helloCancelled")
        );
        assert_eq!(
            status(KeyCredentialStatus::UserPrefersPassword),
            Err("helloCancelled")
        );
        assert_eq!(
            status(KeyCredentialStatus::NotFound),
            Err("helloUnavailable")
        );
    }
}
