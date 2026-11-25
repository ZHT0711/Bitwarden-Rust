//! Windows invokes this in-process COM server. Private keys never enter the UI ABI.
use crate::vault::{
    passkeys::{PasskeyOperation, PasskeyResult},
    VaultError,
};
use std::{
    collections::HashSet,
    error::Error,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};
use win_webauthn::{
    plugin::{
        Clsid, PluginAddAuthenticatorOptions, PluginAuthenticator, PluginCancelOperationRequest,
        PluginGetAssertionRequest, PluginLockStatus, PluginMakeCredentialRequest,
        PluginUserVerificationRequest, WebAuthnPlugin,
    },
    AuthenticatorInfo, CborWriter, CtapVersion, PublicKeyCredentialParameters, Uuid,
};
use windows::{core::GUID, Win32::Foundation::HWND};

pub(crate) const CLSID: &str = "{68bf13a5-a81e-4514-a87e-e282b75baf26}";
const AAGUID: &str = "bb905b68-1c5e-4939-b25a-7a24b83f40d6";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
static SERVER: OnceLock<Mutex<Option<WebAuthnPlugin>>> = OnceLock::new();
static PENDING: Mutex<Option<Arc<Pending>>> = Mutex::new(None);

pub(crate) struct Pending {
    pub id: String,
    pub operation: PasskeyOperation,
    transaction: GUID,
    hash: Vec<u8>,
    created: Instant,
    state: Mutex<PendingState>,
    ready: Condvar,
}
#[derive(Default)]
struct PendingState {
    processing: bool,
    cancelled: bool,
    result: Option<Result<Vec<u8>, String>>,
}

impl Pending {
    pub fn is_current(&self) -> bool {
        self.state.lock().is_ok_and(|s| self.is_active(&s))
    }

    fn is_active(&self, state: &PendingState) -> bool {
        !state.cancelled && state.result.is_none() && self.created.elapsed() < REQUEST_TIMEOUT
    }
    pub fn begin(&self) -> Result<(), VaultError> {
        let mut state = self.state.lock().map_err(|_| VaultError::Internal)?;
        if !self.is_active(&state) || state.processing {
            return Err(VaultError::NotFound);
        }
        state.processing = true;
        Ok(())
    }
    #[must_use]
    pub fn complete(&self, result: Result<Vec<u8>, String>) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if !self.is_active(&state) {
            return false;
        }
        state.result = Some(result);
        self.ready.notify_all();
        true
    }

    pub fn reject_before_approval(&self, error: VaultError) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if !self.is_active(&state) || state.processing {
            return false;
        }
        state.result = Some(Err(error.to_string()));
        self.ready.notify_all();
        true
    }

    fn cancel(&self) {
        if let Ok(mut state) = self.state.lock() {
            if state.result.is_none() {
                state.cancelled = true;
                self.ready.notify_all();
            }
        }
    }
    pub fn verify(&self, window: usize, user_name: &str) -> Result<(), VaultError> {
        if !self.is_current() || window == 0 {
            return Err(VaultError::NotFound);
        }
        let plugin = WebAuthnPlugin::new(Clsid::try_from(CLSID).map_err(|_| VaultError::Internal)?);
        plugin
            .perform_user_verification(
                PluginUserVerificationRequest {
                    window_handle: HWND(window as *mut _),
                    transaction_id: self.transaction,
                    user_name: user_name.to_owned(),
                    display_hint: Some(self.operation.rp_id.clone()),
                },
                &self.hash,
            )
            .map_err(|error| {
                if error.is_user_cancelled() {
                    VaultError::Cancelled
                } else {
                    VaultError::PermissionDenied
                }
            })
    }
}

pub(crate) fn pending() -> Option<Arc<Pending>> {
    PENDING
        .lock()
        .ok()?
        .as_ref()
        .filter(|p| p.is_current())
        .cloned()
}
pub(crate) fn cancel(id: &str) {
    if let Ok(pending) = PENDING.lock() {
        if let Some(request) = pending.as_ref().filter(|p| id.is_empty() || p.id == id) {
            request.cancel();
        }
    }
}
pub(crate) fn start() -> Result<(), VaultError> {
    let mut server = SERVER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| VaultError::Internal)?;
    if server.is_none() {
        let mut plugin =
            WebAuthnPlugin::new(Clsid::try_from(CLSID).map_err(|_| VaultError::Internal)?);
        plugin
            .register_server(Handler)
            .map_err(|e| VaultError::PasskeyProvider(e.to_string()))?;
        *server = Some(plugin);
    }
    Ok(())
}
pub(crate) fn register() -> Result<(), VaultError> {
    start()?;
    let options = PluginAddAuthenticatorOptions {
        authenticator_name: "Bytewarden (Preview)".to_owned(),
        clsid: Clsid::try_from(CLSID).map_err(|_| VaultError::Internal)?,
        rp_id: Some("zht0711.github.io".to_owned()),
        light_theme_logo_svg: None,
        dark_theme_logo_svg: None,
        authenticator_info: AuthenticatorInfo {
            versions: HashSet::from([CtapVersion::Fido2_0, CtapVersion::Fido2_1]),
            aaguid: Uuid::try_from(AAGUID).map_err(|_| VaultError::Internal)?,
            options: Some(HashSet::from([
                "rk".to_owned(),
                "up".to_owned(),
                "uv".to_owned(),
            ])),
            transports: Some(HashSet::from(["internal".to_owned()])),
            algorithms: Some(vec![PublicKeyCredentialParameters {
                alg: -7,
                typ: "public-key".to_owned(),
            }]),
        },
        supported_rp_ids: None,
    };
    if WebAuthnPlugin::add_authenticator(&options)
        .map_err(|e| VaultError::PasskeyProvider(e.to_string()))?
        .is_none()
    {
        WebAuthnPlugin::update_authenticator_details(&options.into())
            .map_err(|e| VaultError::PasskeyProvider(e.to_string()))?;
    }
    Ok(())
}

pub(crate) fn state() -> &'static str {
    let Ok(clsid) = Clsid::try_from(CLSID) else {
        return "unavailable";
    };
    match WebAuthnPlugin::new(clsid).get_authenticator_state() {
        Ok(win_webauthn::plugin::AuthenticatorState::Enabled) => "enabled",
        Ok(win_webauthn::plugin::AuthenticatorState::Disabled) => "disabled",
        Err(_) => "unregistered",
    }
}

pub(crate) async fn sync_metadata(
    vault: &crate::vault::VaultCache,
    client: &bw_sdk_core::Client,
) -> bool {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use win_webauthn::{
        plugin::{AuthenticatorState, PluginCredentialDetails},
        CredentialId, UserId,
    };
    if SERVER.get().is_none() {
        return false;
    }
    let Ok(clsid) = Clsid::try_from(CLSID) else {
        return false;
    };
    let plugin = WebAuthnPlugin::new(clsid);
    if plugin.get_authenticator_state().ok() != Some(AuthenticatorState::Enabled) {
        return false;
    }
    let Ok(views) = vault.passkey_views(client).await else {
        return false;
    };
    let mut metadata = Vec::new();
    for view in views {
        for credential in view.get_fido2_credentials() {
            if !crate::vault::passkeys::is_discoverable(&credential) {
                continue;
            }
            let Ok(id) = bitwarden_fido::string_to_guid_bytes(&credential.credential_id) else {
                continue;
            };
            let Some(handle) = credential
                .user_handle
                .as_ref()
                .and_then(|h| URL_SAFE_NO_PAD.decode(h).ok())
            else {
                continue;
            };
            let (Ok(credential_id), Ok(user_id)) =
                (CredentialId::try_from(id), UserId::try_from(handle))
            else {
                continue;
            };
            metadata.push(PluginCredentialDetails {
                credential_id,
                user_id,
                rp_id: credential.rp_id,
                rp_friendly_name: credential.rp_name,
                user_name: credential.user_name.unwrap_or_default(),
                user_display_name: credential.user_display_name.unwrap_or_default(),
            });
        }
    }
    // Windows receives only public discovery metadata. The vault remains authoritative.
    plugin.sync_credentials(metadata).is_ok()
}

pub(crate) fn clear_metadata() {
    if SERVER.get().is_none() {
        return;
    }
    if let Ok(clsid) = Clsid::try_from(CLSID) {
        let _ = WebAuthnPlugin::new(clsid).sync_credentials(vec![]);
    }
}

fn request(
    operation: PasskeyOperation,
    transaction: GUID,
    hash: &[u8],
) -> Result<Vec<u8>, Box<dyn Error>> {
    operation.validate()?;
    let request = Arc::new(Pending {
        id: uuid::Uuid::new_v4().to_string(),
        operation,
        transaction,
        hash: hash.to_vec(),
        created: Instant::now(),
        state: Mutex::new(PendingState::default()),
        ready: Condvar::new(),
    });
    {
        let mut slot = PENDING.lock().map_err(|_| "provider unavailable")?;
        if slot.as_ref().is_some_and(|p| p.is_current()) {
            return Err("provider busy".into());
        }
        *slot = Some(request.clone());
    }
    let state = request.state.lock().map_err(|_| "provider unavailable")?;
    let (mut state, _) = request
        .ready
        .wait_timeout_while(
            state,
            REQUEST_TIMEOUT.saturating_sub(request.created.elapsed()),
            |s| !s.cancelled && s.result.is_none(),
        )
        .map_err(|_| "provider unavailable")?;
    let result = state
        .result
        .take()
        .unwrap_or_else(|| Err("cancelled".to_owned()));
    state.cancelled = true;
    drop(state);
    if let Ok(mut slot) = PENDING.lock() {
        if slot.as_ref().is_some_and(|p| p.id == request.id) {
            *slot = None;
        }
    }
    result.map_err(|code| -> Box<dyn Error> {
        match code.as_str() {
            "credentialExcluded" => {
                Box::new(win_webauthn::plugin::PluginError::ExcludedCredentialMatched)
            }
            "notFound" => Box::new(win_webauthn::plugin::PluginError::CredentialNotFound),
            "cancelled" => Box::new(win_webauthn::plugin::PluginError::UserCancelled),
            _ => code.into(),
        }
    })
}
struct Handler;
impl PluginAuthenticator for Handler {
    fn make_credential(
        &self,
        value: PluginMakeCredentialRequest,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let user = value.user_information();
        request(
            PasskeyOperation {
                creating: true,
                rp_id: value.rp_information().id(),
                user_name: Some(user.name()),
                user_display_name: Some(user.display_name()),
                user_handle: user.id().to_vec(),
                client_data_hash: value.client_data_hash().to_vec(),
                credential_ids: value
                    .exclude_credentials()
                    .filter_map(|c| c.credential_id().map(Vec::from))
                    .collect(),
                algorithms: value.pub_key_cred_params().map(|p| p.alg()).collect(),
            },
            value.transaction_id,
            value.operation_request_hash(),
        )
    }
    fn get_assertion(&self, value: PluginGetAssertionRequest) -> Result<Vec<u8>, Box<dyn Error>> {
        request(
            PasskeyOperation {
                creating: false,
                rp_id: value.rp_id().to_owned(),
                user_name: None,
                user_display_name: None,
                user_handle: vec![],
                client_data_hash: value.client_data_hash().to_vec(),
                credential_ids: value
                    .allow_credentials()
                    .filter_map(|c| c.credential_id().map(Vec::from))
                    .collect(),
                algorithms: vec![],
            },
            value.transaction_id,
            value.operation_request_hash(),
        )
    }
    fn cancel_operation(&self, value: PluginCancelOperationRequest) -> Result<(), Box<dyn Error>> {
        if let Some(pending) = pending().filter(|p| {
            p.transaction == value.transaction_id() && p.hash == value.operation_request_hash()
        }) {
            cancel(&pending.id);
        }
        Ok(())
    }
    fn lock_status(&self) -> Result<PluginLockStatus, Box<dyn Error>> {
        Ok(if crate::ffi::bw_auth_is_authenticated() != 0 {
            PluginLockStatus::PluginUnlocked
        } else {
            PluginLockStatus::PluginLocked
        })
    }
}

pub(crate) fn encode_response(result: PasskeyResult) -> Result<Vec<u8>, VaultError> {
    let mut data = Vec::new();
    let mut writer = CborWriter::new(&mut data);
    let encoded = (|| -> Result<(), Box<dyn Error>> {
        match result {
            PasskeyResult::Created {
                mut authenticator_data,
                ..
            } => {
                // The SDK emits none attestation. Identify this provider with its own AAGUID.
                if authenticator_data.len() < 55 || authenticator_data[32] & 0x40 == 0 {
                    return Err("invalid authenticator data".into());
                }
                authenticator_data[37..53]
                    .copy_from_slice(uuid::Uuid::parse_str(AAGUID)?.as_bytes());
                writer.write_map_start(3)?;
                writer.write_number(1)?;
                writer.write_text("none")?;
                writer.write_number(2)?;
                writer.write_bytes(authenticator_data)?;
                writer.write_number(3)?;
                writer.write_map_start(0)?;
            }
            PasskeyResult::Assertion {
                credential_id,
                authenticator_data,
                signature,
                user_handle,
            } => {
                writer.write_map_start(if user_handle.is_empty() { 4 } else { 5 })?;
                writer.write_number(1)?;
                writer.write_map_start(2)?;
                writer.write_text("id")?;
                writer.write_bytes(credential_id)?;
                writer.write_text("type")?;
                writer.write_text("public-key")?;
                writer.write_number(2)?;
                writer.write_bytes(authenticator_data)?;
                writer.write_number(3)?;
                writer.write_bytes(signature)?;
                if !user_handle.is_empty() {
                    writer.write_number(4)?;
                    writer.write_map_start(1)?;
                    writer.write_text("id")?;
                    writer.write_bytes(user_handle)?;
                }
                writer.write_number(5)?;
                writer.write_number(1)?;
            }
        }
        Ok(())
    })();
    encoded.map_err(|_| VaultError::Internal)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use win_webauthn::{CborParser, CborValue};

    #[test]
    fn registration_response_has_own_aaguid_and_none_attestation() {
        let mut authenticator_data = vec![0; 55];
        authenticator_data[32] = 0x45;
        let response = encode_response(PasskeyResult::Created {
            authenticator_data,
            credential_id: vec![1],
        })
        .unwrap();
        let map = CborParser::parse(&response).unwrap().into_map().unwrap();
        let auth_data = map
            .iter()
            .find(|(k, _)| *k == CborValue::PositiveInteger(2))
            .unwrap()
            .1
            .as_bytes()
            .unwrap();
        assert_eq!(
            &auth_data[37..53],
            uuid::Uuid::parse_str(AAGUID).unwrap().as_bytes()
        );
        assert!(map
            .iter()
            .any(|(k, v)| *k == CborValue::PositiveInteger(1) && v.as_text() == Some("none")));
        assert!(encode_response(PasskeyResult::Created {
            authenticator_data: vec![0; 32],
            credential_id: vec![]
        })
        .is_err());
    }

    #[test]
    fn assertion_response_omits_empty_user_and_includes_present_user() {
        for handle in [vec![], vec![42]] {
            let response = encode_response(PasskeyResult::Assertion {
                credential_id: vec![1],
                authenticator_data: vec![2],
                signature: vec![3],
                user_handle: handle.clone(),
            })
            .unwrap();
            let map = CborParser::parse(&response).unwrap().into_map().unwrap();
            assert_eq!(map.len(), if handle.is_empty() { 4 } else { 5 });
            assert_eq!(
                map.iter().any(|(k, _)| *k == CborValue::PositiveInteger(4)),
                !handle.is_empty()
            );
        }
    }

    #[test]
    fn pending_request_rejects_second_approval_cancellation_and_expiry() {
        let operation = PasskeyOperation {
            creating: false,
            rp_id: "example.invalid".to_owned(),
            user_name: None,
            user_display_name: None,
            user_handle: vec![],
            client_data_hash: vec![0; 32],
            credential_ids: vec![],
            algorithms: vec![],
        };
        let mut pending = Pending {
            id: "synthetic".to_owned(),
            operation,
            transaction: GUID::zeroed(),
            hash: vec![1; 32],
            created: Instant::now(),
            state: Mutex::new(PendingState::default()),
            ready: Condvar::new(),
        };
        assert!(pending.is_current());
        pending.begin().unwrap();
        assert!(pending.begin().is_err());
        assert!(pending.complete(Ok(vec![1])));
        assert!(!pending.complete(Err("notFound".to_owned())));
        pending.cancel();
        assert_eq!(pending.state.lock().unwrap().result, Some(Ok(vec![1])));
        assert!(!pending.is_current());
        pending.state = Mutex::new(PendingState::default());
        pending.cancel();
        assert!(!pending.complete(Ok(vec![2])));
        assert!(pending.state.lock().unwrap().result.is_none());
        assert!(!pending.is_current());
        assert!(pending.begin().is_err());
        pending.state = Mutex::new(PendingState::default());
        pending.created = Instant::now() - REQUEST_TIMEOUT;
        assert!(!pending.is_current());
        assert!(pending.begin().is_err());
        assert!(!pending.complete(Ok(vec![3])));
        assert!(pending.state.lock().unwrap().result.is_none());

        pending.state = Mutex::new(PendingState::default());
        pending.created = Instant::now();
        assert!(pending.reject_before_approval(VaultError::NotFound));
        assert_eq!(
            pending.state.lock().unwrap().result,
            Some(Err("notFound".to_owned()))
        );
        pending.state = Mutex::new(PendingState::default());
        pending.begin().unwrap();
        assert!(!pending.reject_before_approval(VaultError::NotFound));
    }
}
