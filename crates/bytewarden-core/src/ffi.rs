use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, OnceLock, TryLockError,
    },
};

use async_trait::async_trait;
use bitwarden_auth::token_management::PasswordManagerTokenHandler;
use bitwarden_generators::{
    GeneratorClient, GeneratorClientsExt, PassphraseGeneratorRequest, PasswordGeneratorRequest,
};
use bitwarden_vault::{CipherId, FolderId};
use bw_sdk_core::{
    auth::login::{
        LoginError, PasswordLoginRequest, PasswordLoginUnlockData, PreloginError,
        TwoFactorProvider, TwoFactorRequest,
    },
    client::{ClientSettings, DeviceType},
    key_management::MasterPasswordAuthenticationData,
    Client, ClientBuilder,
};
#[cfg(windows)]
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod auth_status_tests;
pub mod desktop;
mod desktop_transfer;
mod export;
#[cfg(windows)]
mod hello;
#[cfg(windows)]
mod hello_window;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod http_tests;
#[cfg(windows)]
mod passkeys;
#[cfg(windows)]
mod pin;
mod remembered_two_factor;
mod sends;

use crate::auth_request::{self, RequestError};
use crate::protocol_compat::{self, ResponseKind, SyncCompatibility};
#[cfg(windows)]
use crate::vault::EncryptedVaultState;
use crate::vault::{VaultCache, VaultEditorData, VaultError, VaultMutationResult, VaultTotp};
mod accounts;
use accounts::AuthAccounts;

const AUTH_FAILED: u32 = 0;
const AUTHENTICATED: u32 = 1;
const TWO_FACTOR_REQUIRED: u32 = 2;
const AUTH_TIMED_OUT: u32 = 3;
const AUTH_CANCELLED: u32 = 4;
const DEVICE_VERIFICATION_REQUIRED: u32 = 5;
const DEVICE_VERIFICATION_INVALID: u32 = 6;
const ACCOUNT_ALREADY_SIGNED_IN: u32 = 7;
const ACCOUNT_LIMIT_REACHED: u32 = 8;
const INVALID_TWO_FACTOR_CODE: u32 = 9;
const AUTH_RATE_LIMITED: u32 = 10;
const AUTH_NETWORK_ERROR: u32 = 11;
const AUTH_SERVER_ERROR: u32 = 12;
const AUTH_REJECTED: u32 = 13;
const TWO_FACTOR_MASK_SHIFT: u32 = 8;
const TWO_FACTOR_AUTHENTICATOR: u32 = 1 << 0;
const TWO_FACTOR_EMAIL: u32 = 1 << 1;
const TWO_FACTOR_YUBIKEY: u32 = 1 << 3;
const TWO_FACTOR_WEBAUTHN: u32 = 1 << 7;

const SESSION_SIGNED_OUT: u8 = 0;
const SESSION_UNLOCKED: u8 = 1;
const SESSION_LOCKED: u8 = 2;
static SESSION_STATE: AtomicU8 = AtomicU8::new(SESSION_SIGNED_OUT);
static AUTH_CLIENT: OnceLock<Mutex<AuthAccounts>> = OnceLock::new();
static ASYNC_RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
static GENERATOR: OnceLock<GeneratorClient> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
struct AuthServer {
    api_url: String,
    identity_url: String,
    self_hosted: bool,
}

pub(super) struct AuthSession {
    account_id: uuid::Uuid,
    email: String,
    server_mode: u32,
    server_url: String,
    state: u8,
    server: AuthServer,
    device_identifier: String,
    client: Client,
    device_login_middleware: Arc<DeviceLoginMiddleware>,
    unlock_data: Option<PasswordLoginUnlockData>,
    vault: VaultCache,
    open_detail: Option<OpenDetail>,
    offline_only: bool,
    send_write_uncertain: bool,
    #[cfg(windows)]
    pin_enrollment: Option<pin::PinEnrollment>,
}

struct OpenDetail {
    request_id: u64,
    item_id: String,
    revision: String,
    has_totp: bool,
    has_attachments: bool,
}

#[cfg(windows)]
#[derive(PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedUnlockData {
    master_password: bw_sdk_core::key_management::MasterPasswordUnlockData,
    account:
        bw_sdk_core::key_management::account_cryptographic_state::WrappedAccountCryptographicState,
}

#[cfg(windows)]
impl From<&PasswordLoginUnlockData> for PersistedUnlockData {
    fn from(value: &PasswordLoginUnlockData) -> Self {
        Self {
            master_password: value.master_password.clone(),
            account: value.account.clone(),
        }
    }
}

#[cfg(windows)]
impl From<PersistedUnlockData> for PasswordLoginUnlockData {
    fn from(value: PersistedUnlockData) -> Self {
        Self {
            master_password: value.master_password,
            account: value.account,
        }
    }
}

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedVaultEnvelope {
    version: u8,
    email: String,
    server_mode: u32,
    server_url: String,
    device_identifier: String,
    account_id: uuid::Uuid,
    unlock_data: Option<PersistedUnlockData>,
    state: EncryptedVaultState,
}

impl AuthSession {
    fn clear_vault_keys(&mut self) -> bool {
        self.open_detail = None;
        self.vault.discard_overview();
        catch_unwind(AssertUnwindSafe(|| {
            self.client.internal.get_key_store().clear()
        }))
        .is_ok()
    }

    #[cfg(windows)]
    fn restore_local_vault(&mut self, runtime: &tokio::runtime::Runtime) {
        let identity = local_cache_identity(self);
        let Some(encoded) = crate::local_cache::load(&identity) else {
            return;
        };
        let pending_write = crate::local_cache::has_pending(&identity);
        let Ok(envelope) = serde_json::from_slice::<PersistedVaultEnvelope>(&encoded) else {
            crate::local_cache::remove(&identity);
            return;
        };
        if envelope.version != 2 || envelope.email != self.email {
            crate::local_cache::remove(&identity);
            return;
        }
        self.account_id = envelope.account_id;
        let (Some(saved_unlock), Some(current_unlock)) = (
            envelope.unlock_data,
            self.unlock_data.as_ref().map(PersistedUnlockData::from),
        ) else {
            crate::local_cache::remove(&identity);
            return;
        };
        if saved_unlock != current_unlock {
            crate::local_cache::remove(&identity);
            return;
        }
        let Ok(mut cache) = VaultCache::from_saved_state(envelope.state) else {
            crate::local_cache::remove(&identity);
            return;
        };
        if pending_write {
            cache.mark_write_uncertain();
        }
        if runtime
            .block_on(cache.restore_overview(&self.client))
            .is_ok()
        {
            // Install the SDK repositories only after the cached overview is validated.
            cache.activate(&self.client);
            // Even after fresh login, writes require a fresh server snapshot.
            self.vault = cache;
        } else {
            crate::local_cache::remove(&identity);
        }
    }
}

#[cfg(windows)]
pub(super) fn restore_persisted_sessions(limit: usize) -> Vec<AuthSession> {
    let mut sessions = crate::local_cache::entries()
        .filter_map(|(digest, encoded)| {
            let pending_write = crate::local_cache::has_pending_digest(&digest);
            let envelope = serde_json::from_slice::<PersistedVaultEnvelope>(&encoded).ok()?;
            if envelope.version != 2 || envelope.email != envelope.email.trim().to_lowercase() {
                return None;
            }
            let server = auth_server(envelope.server_mode, &envelope.server_url)?;
            if crate::local_cache::account_digest(&format!(
                "{}\n{}",
                server.api_url, envelope.email
            )) != digest
                || uuid::Uuid::parse_str(&envelope.device_identifier).is_err()
            {
                return None;
            }
            let unlock_data = envelope.unlock_data.map(PasswordLoginUnlockData::from)?;
            let mut vault = VaultCache::from_saved_state(envelope.state).ok()?;
            if pending_write {
                vault.mark_write_uncertain();
            }
            if !vault.has_cached_snapshot() {
                return None;
            }
            let middleware = Arc::new(DeviceLoginMiddleware::new(
                envelope.device_identifier.clone(),
            ));
            Some(AuthSession {
                account_id: envelope.account_id,
                email: envelope.email,
                server_mode: envelope.server_mode,
                server_url: envelope.server_url,
                state: SESSION_LOCKED,
                server: server.clone(),
                device_identifier: envelope.device_identifier,
                client: build_auth_client(&server, middleware.clone()),
                device_login_middleware: middleware,
                unlock_data: Some(unlock_data),
                vault,
                open_detail: None,
                offline_only: true,
                send_write_uncertain: false,
                pin_enrollment: None,
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        left.email
            .cmp(&right.email)
            .then_with(|| left.server.api_url.cmp(&right.server.api_url))
    });
    sessions
}

#[cfg(windows)]
fn local_cache_identity(session: &AuthSession) -> String {
    format!("{}\n{}", session.server.api_url, session.email)
}

#[cfg(windows)]
fn remove_cached_vault(session: &AuthSession) -> bool {
    crate::local_cache::remove(&local_cache_identity(session))
}

#[cfg(windows)]
fn persist_cached_vault(session: &AuthSession) {
    if !crate::local_cache::is_configured() || !session.vault.has_cached_snapshot() {
        return;
    }
    let Some(unlock_data) = session.unlock_data.as_ref() else {
        return;
    };
    let Ok(state) = session.vault.snapshot_state() else {
        return;
    };
    let envelope = PersistedVaultEnvelope {
        version: 2,
        email: session.email.clone(),
        server_mode: session.server_mode,
        server_url: session.server_url.clone(),
        device_identifier: session.device_identifier.clone(),
        account_id: session.account_id,
        unlock_data: Some(unlock_data.into()),
        state,
    };
    let Ok(encoded) = serde_json::to_vec(&envelope).map(Zeroizing::new) else {
        return;
    };
    let identity = local_cache_identity(session);
    if crate::local_cache::save(&identity, &encoded) {
        crate::local_cache::clear_pending(&identity);
    }
}

#[cfg(windows)]
struct CacheWrite {
    new_marker: bool,
}

#[cfg(windows)]
impl CacheWrite {
    fn begin(session: &AuthSession) -> Result<Self, VaultError> {
        use crate::local_cache;

        if !session.vault.has_cached_snapshot() || !local_cache::is_configured() {
            return Ok(Self { new_marker: false });
        }
        let identity = local_cache_identity(session);
        // An older marker may represent a confirmed write whose snapshot could not be saved.
        let new_marker = !local_cache::has_pending(&identity);
        if !local_cache::mark_pending(&identity) {
            return Err(VaultError::LocalStorage);
        }
        Ok(Self { new_marker })
    }

    fn finish(self, session: &AuthSession, snapshot_changed: bool) {
        if session.vault.needs_reconciliation() {
            // Keep the last confirmed snapshot and the durable marker; don't save partial state.
            return;
        }
        if snapshot_changed {
            persist_cached_vault(session);
        } else if self.new_marker {
            crate::local_cache::clear_pending(&local_cache_identity(session));
        }
    }
}

struct DeviceLoginMiddleware {
    device_identifier: String,
    pending_otp: Mutex<Option<Zeroizing<String>>>,
}

impl DeviceLoginMiddleware {
    fn new(device_identifier: String) -> Self {
        Self {
            device_identifier,
            pending_otp: Mutex::new(None),
        }
    }

    fn arm(&self, otp: &str) {
        let mut pending = self
            .pending_otp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pending = (!otp.is_empty()).then(|| Zeroizing::new(otp.to_owned()));
    }

    fn rewrite_login_request(&self, request: &mut reqwest::Request) {
        let Some(body) = request.body().and_then(reqwest::Body::as_bytes) else {
            return;
        };
        let fields = Zeroizing::new(
            url::form_urlencoded::parse(body)
                .into_owned()
                .collect::<Vec<_>>(),
        );
        if !fields
            .iter()
            .any(|(key, value)| key == "grant_type" && value == "password")
        {
            return;
        }
        let otp = self
            .pending_otp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in fields.iter() {
            if matches!(
                key.as_str(),
                "deviceType" | "deviceIdentifier" | "deviceName" | "newDeviceOtp"
            ) {
                continue;
            }
            form.append_pair(key, value);
        }
        form.append_pair("deviceType", "6");
        form.append_pair("deviceIdentifier", &self.device_identifier);
        form.append_pair("deviceName", "BytewardenWinUI");
        if let Some(otp) = otp.as_ref() {
            form.append_pair("newDeviceOtp", otp);
        }
        *request.body_mut() = Some(reqwest::Body::from(form.finish()));
    }
}

#[async_trait]
impl reqwest_middleware::Middleware for DeviceLoginMiddleware {
    async fn handle(
        &self,
        mut request: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let path = request.url().path();
        let is_token = path.ends_with("/connect/token");
        let is_prelogin = path.ends_with("/accounts/prelogin/password");
        if is_token && request.method() == reqwest::Method::POST {
            self.rewrite_login_request(&mut request);
        }
        let response = next.run(request, extensions).await?;
        if is_token {
            protocol_compat::normalize(response, ResponseKind::Token).await
        } else if is_prelogin {
            protocol_compat::normalize(response, ResponseKind::Prelogin).await
        } else {
            Ok(response)
        }
    }
}

struct SensitiveLoginRequest(PasswordLoginRequest);

impl Drop for SensitiveLoginRequest {
    fn drop(&mut self) {
        self.0.password.zeroize();
        if let Some(two_factor) = self.0.two_factor.as_mut() {
            two_factor.token.zeroize();
        }
    }
}

struct SensitiveEmailRequest(bw_sdk_core::auth::login::TwoFactorEmailRequest);

impl Drop for SensitiveEmailRequest {
    fn drop(&mut self) {
        self.0.password.zeroize();
    }
}

fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    ASYNC_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
}

#[cfg(test)]
pub(in crate::ffi) static AUTH_TEST_LOCK: Mutex<()> = Mutex::new(());

fn auth_client() -> &'static Mutex<AuthAccounts> {
    AUTH_CLIENT.get_or_init(|| Mutex::new(AuthAccounts::default()))
}

fn auth_server(mode: u32, base_url: &str) -> Option<AuthServer> {
    match mode {
        0 => Some(AuthServer {
            api_url: "https://api.bitwarden.com".to_owned(),
            identity_url: "https://identity.bitwarden.com".to_owned(),
            self_hosted: false,
        }),
        1 => Some(AuthServer {
            api_url: "https://api.bitwarden.eu".to_owned(),
            identity_url: "https://identity.bitwarden.eu".to_owned(),
            self_hosted: false,
        }),
        2 => custom_auth_server(base_url),
        _ => None,
    }
}

fn custom_auth_server(base_url: &str) -> Option<AuthServer> {
    let base_url = Url::parse(base_url.trim()).ok()?;
    let is_loopback = match base_url.host()? {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    };
    if (base_url.scheme() != "https" && !(base_url.scheme() == "http" && is_loopback))
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        return None;
    }

    let final_path_segment = base_url
        .path_segments()?
        .rfind(|segment| !segment.is_empty())
        .unwrap_or_default();
    if matches!(final_path_segment, "api" | "identity") {
        return None;
    }

    fn append_endpoint(base_url: &Url, endpoint: &str) -> Option<String> {
        let mut url = base_url.clone();
        url.path_segments_mut().ok()?.pop_if_empty().push(endpoint);
        Some(url.into())
    }

    Some(AuthServer {
        api_url: append_endpoint(&base_url, "api")?,
        identity_url: append_endpoint(&base_url, "identity")?,
        self_hosted: true,
    })
}

fn build_auth_client(server: &AuthServer, middleware: Arc<DeviceLoginMiddleware>) -> Client {
    let settings = ClientSettings {
        api_url: server.api_url.clone(),
        identity_url: server.identity_url.clone(),
        device_type: DeviceType::WindowsDesktop,
        user_agent: format!("Bytewarden/{} (Windows Desktop)", env!("CARGO_PKG_VERSION")),
        bitwarden_client_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        bitwarden_package_type: Some("msix".to_owned()),
        ..ClientSettings::default()
    };

    let builder = ClientBuilder::new()
        .with_settings(settings)
        .with_token_handler(std::sync::Arc::new(PasswordManagerTokenHandler::default()))
        .with_identity_middleware(vec![middleware]);
    if server.self_hosted {
        #[cfg(not(target_arch = "wasm32"))]
        let builder = builder.with_loopback_http();
        builder
            .with_middleware(vec![
                Arc::new(SyncCompatibility),
                Arc::new(sends::SendUpdateCompatibility),
            ])
            .build()
    } else {
        builder
            .with_middleware(vec![Arc::new(sends::SendUpdateCompatibility)])
            .build()
    }
}

fn auth_client_for_server<'a>(
    session: &'a mut Option<AuthSession>,
    server: &AuthServer,
    identity: (&str, u32, &str),
    device_identifier: &str,
) -> &'a mut AuthSession {
    let (email, server_mode, server_url) = identity;
    let device_identifier = uuid::Uuid::parse_str(device_identifier)
        .map(|id| id.to_string())
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
    if session.as_ref().is_none_or(|current| {
        current.server != *server
            || current.device_identifier != device_identifier
            || current.email != email
    }) {
        let middleware = Arc::new(DeviceLoginMiddleware::new(device_identifier.clone()));
        *session = Some(AuthSession {
            account_id: uuid::Uuid::new_v4(),
            email: email.to_owned(),
            server_mode,
            server_url: if server_mode == 2 {
                server_url.to_owned()
            } else {
                String::new()
            },
            state: SESSION_SIGNED_OUT,
            server: server.clone(),
            device_identifier,
            client: build_auth_client(server, middleware.clone()),
            device_login_middleware: middleware,
            unlock_data: None,
            vault: VaultCache::default(),
            open_detail: None,
            offline_only: false,
            send_write_uncertain: false,
            #[cfg(windows)]
            pin_enrollment: None,
        });
    }

    session.as_mut().expect("auth session was initialized")
}

unsafe fn read_utf8(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }

    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

fn two_factor_provider(value: u32) -> Option<TwoFactorProvider> {
    match value {
        0 => Some(TwoFactorProvider::Authenticator),
        1 => Some(TwoFactorProvider::Email),
        3 => Some(TwoFactorProvider::Yubikey),
        _ => None,
    }
}

fn advertised_two_factor_methods(
    response: &bw_sdk_core::auth::login::response::two_factor::TwoFactorProviders,
) -> u32 {
    let mut methods = 0;
    if response.authenticator.is_some() {
        methods |= TWO_FACTOR_AUTHENTICATOR;
    }
    if response.email.is_some() {
        methods |= TWO_FACTOR_EMAIL;
    }
    if response.yubi_key.is_some() {
        methods |= TWO_FACTOR_YUBIKEY;
    }
    if response.web_authn.is_some() {
        methods |= TWO_FACTOR_WEBAUTHN;
    }
    methods
}

fn login_status(
    result: Result<
        bw_sdk_core::auth::login::PasswordLoginResponse,
        bw_sdk_core::auth::login::LoginError,
    >,
) -> u32 {
    match result {
        Ok(response) if response.authenticated => AUTHENTICATED,
        Ok(response) => {
            let Some(providers) = response.two_factor else {
                return AUTH_FAILED;
            };
            let methods = advertised_two_factor_methods(&providers);
            TWO_FACTOR_REQUIRED | (methods << TWO_FACTOR_MASK_SHIFT)
        }
        Err(error) => login_error_status(&error),
    }
}

fn login_error_status(error: &LoginError) -> u32 {
    match error {
        LoginError::IdentityFail(error)
            if error.error_model.message.trim() == "new device verification required"
                || error.error_description.trim() == "new device verification required" =>
        {
            DEVICE_VERIFICATION_REQUIRED
        }
        LoginError::IdentityFail(error)
            if error.error_model.message.trim() == "invalid new device otp"
                || error.error_description.trim() == "invalid new device otp" =>
        {
            DEVICE_VERIFICATION_INVALID
        }
        LoginError::IdentityFail(error) if error.error.eq_ignore_ascii_case("TooManyRequests") => {
            AUTH_RATE_LIMITED
        }
        LoginError::IdentityFail(error)
            if is_invalid_two_factor_message(&error.error_model.message)
                || is_invalid_two_factor_message(&error.error_description) =>
        {
            INVALID_TWO_FACTOR_CODE
        }
        // invalid_grant rejects authentication, but does not prove a wrong password.
        LoginError::IdentityFail(error) if error.error.eq_ignore_ascii_case("invalid_grant") => {
            AUTH_REJECTED
        }
        LoginError::AuthenticationFailed => AUTH_REJECTED,
        LoginError::IdentityFail(_) => AUTH_SERVER_ERROR,
        LoginError::Api(error) => auth_api_error_status(error),
        LoginError::Prelogin(error) => auth_prelogin_error_status(error),
        LoginError::Serde(_)
        | LoginError::MissingField(_)
        | LoginError::JwtTokenParse(_)
        | LoginError::JwtTokenMissingEmail
        | LoginError::InvalidResponse => AUTH_SERVER_ERROR,
        _ => AUTH_FAILED,
    }
}

fn auth_prelogin_error_status(error: &PreloginError) -> u32 {
    match error {
        PreloginError::Api(error) => auth_api_error_status(error),
        PreloginError::MissingField(_) => AUTH_SERVER_ERROR,
    }
}

// Classify only typed metadata. Never format, log, or return an SDK error/body/URL.
fn auth_api_error_status(error: &bw_sdk_core::ApiError) -> u32 {
    use bw_sdk_core::ApiError;
    match error {
        ApiError::Reqwest(error) => auth_reqwest_error_status(error),
        ApiError::ReqwestMiddleware(reqwest_middleware::Error::Reqwest(error)) => {
            auth_reqwest_error_status(error)
        }
        ApiError::ReqwestMiddleware(reqwest_middleware::Error::Middleware(_)) => AUTH_FAILED,
        ApiError::Response(response) => auth_http_status(response.status),
        ApiError::Serde(_) => AUTH_SERVER_ERROR,
        ApiError::Io(error) => match error.kind() {
            std::io::ErrorKind::TimedOut => AUTH_TIMED_OUT,
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof => AUTH_NETWORK_ERROR,
            std::io::ErrorKind::InvalidData => AUTH_SERVER_ERROR,
            _ => AUTH_FAILED,
        },
    }
}

fn auth_http_status(status: reqwest::StatusCode) -> u32 {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        AUTH_RATE_LIMITED
    } else {
        // An HTTP error alone does not establish that the credentials were rejected.
        AUTH_SERVER_ERROR
    }
}

fn auth_reqwest_error_status(error: &reqwest::Error) -> u32 {
    if let Some(status) = error.status() {
        auth_http_status(status)
    } else if error.is_timeout() {
        AUTH_TIMED_OUT
    } else if error.is_decode() {
        if reqwest_error_has_incomplete_http_source(error) {
            AUTH_NETWORK_ERROR
        } else {
            AUTH_SERVER_ERROR
        }
    } else if error.is_connect() || error.is_request() || error.is_body() {
        // DNS, TLS/connect and request sending can fail before a usable response exists.
        AUTH_NETWORK_ERROR
    } else {
        AUTH_FAILED
    }
}

fn reqwest_error_has_incomplete_http_source(error: &reqwest::Error) -> bool {
    use std::error::Error as _;
    let mut source = error.source();
    while let Some(current) = source {
        if let Some(hyper) = current.downcast_ref::<hyper::Error>() {
            return hyper.is_incomplete_message() || hyper.is_closed();
        }
        source = current.source();
    }
    false
}

fn login_status_ref_is_invalid_code(error: &LoginError) -> bool {
    matches!(error, LoginError::IdentityFail(error)
        if is_invalid_two_factor_message(&error.error_model.message)
            || is_invalid_two_factor_message(&error.error_description))
}

fn is_invalid_two_factor_message(message: &str) -> bool {
    let message = message.trim().trim_end_matches('.');
    [
        "Two-step token is invalid. Try again",
        "Two factor token is invalid",
    ]
    .iter()
    .any(|expected| message.eq_ignore_ascii_case(expected))
}

fn generate_password(
    length: u32,
    options: u32,
    min_numbers: u32,
    min_special: u32,
) -> Option<String> {
    let min_numbers = u8::try_from(min_numbers).ok()?;
    let min_special = u8::try_from(min_special).ok()?;
    if min_numbers > 9 || min_special > 9 {
        return None;
    }

    let request = PasswordGeneratorRequest {
        uppercase: options & 1 != 0,
        lowercase: options & 2 != 0,
        numbers: options & 4 != 0,
        special: options & 8 != 0,
        length: length.clamp(5, 128) as u8,
        avoid_ambiguous: options & 16 != 0,
        min_number: Some(min_numbers),
        min_special: Some(min_special),
        ..Default::default()
    };

    generator().password(request).ok()
}

fn generate_passphrase(
    num_words: u32,
    word_separator: &str,
    capitalize: bool,
    include_number: bool,
) -> Option<String> {
    let request = PassphraseGeneratorRequest {
        num_words: u8::try_from(num_words).ok()?,
        word_separator: word_separator.to_owned(),
        capitalize,
        include_number,
    };

    generator().passphrase(request).ok()
}

fn generator() -> &'static GeneratorClient {
    // This instance never receives account credentials or tokens.
    GENERATOR.get_or_init(|| Client::new(None).generator())
}

fn into_raw_string(value: String) -> *mut c_char {
    CString::new(value)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn bw_core_version() -> *const c_char {
    c"0.2.0".as_ptr()
}

#[no_mangle]
pub extern "C" fn bw_generate_password(
    length: u32,
    options: u32,
    min_numbers: u32,
    min_special: u32,
) -> *mut c_char {
    generate_password(length, options, min_numbers, min_special)
        .map(into_raw_string)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn bw_generate_passphrase(
    num_words: u32,
    word_separator: *const c_char,
    capitalize: u8,
    include_number: u8,
) -> *mut c_char {
    if word_separator.is_null() {
        return std::ptr::null_mut();
    }

    let Ok(word_separator) = (unsafe { CStr::from_ptr(word_separator) }).to_str() else {
        return std::ptr::null_mut();
    };

    generate_passphrase(
        num_words,
        word_separator,
        capitalize != 0,
        include_number != 0,
    )
    .map(into_raw_string)
    .unwrap_or(std::ptr::null_mut())
}

/// Authenticate through the official Bitwarden Rust SDK. The result's low byte is a status;
/// when status is TWO_FACTOR_REQUIRED, the upper bits contain a supported-provider bitmask.
#[no_mangle]
pub extern "C" fn bw_auth_begin_request() -> u64 {
    auth_request::begin()
}

#[no_mangle]
pub extern "C" fn bw_auth_cancel_request(request_id: u64) {
    auth_request::cancel(request_id);
}

#[no_mangle]
pub unsafe extern "C" fn bw_auth_validate_server(mode: u32, server_url: *const c_char) -> u8 {
    let Some(server_url) = (unsafe { read_utf8(server_url) }) else {
        return 0;
    };
    u8::from(auth_server(mode, &server_url).is_some())
}

#[no_mangle]
pub unsafe extern "C" fn bw_auth_login(
    email: *const c_char,
    password: *const c_char,
    provider: u32,
    token: *const c_char,
    server_mode: u32,
    server_url: *const c_char,
    device_identifier: *const c_char,
    device_otp: *const c_char,
    request_id: u64,
) -> u32 {
    // Preserve the original ABI for clients that do not request remembered 2FA.
    unsafe {
        bw_auth_login_with_remember(
            email,
            password,
            provider,
            token,
            server_mode,
            server_url,
            device_identifier,
            device_otp,
            0,
            request_id,
        )
    }
}

/// Password login with an explicit 30-day remembered two-factor choice.
#[no_mangle]
pub unsafe extern "C" fn bw_auth_login_with_remember(
    email: *const c_char,
    password: *const c_char,
    provider: u32,
    token: *const c_char,
    server_mode: u32,
    server_url: *const c_char,
    device_identifier: *const c_char,
    device_otp: *const c_char,
    remember: u8,
    request_id: u64,
) -> u32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(email) = (unsafe { read_utf8(email) }) else {
            return AUTH_FAILED;
        };
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return AUTH_FAILED;
        };
        let password = Zeroizing::new(password);
        let Some(token) = (unsafe { read_utf8(token) }) else {
            return AUTH_FAILED;
        };
        let token = Zeroizing::new(token);
        let Some(server_url) = (unsafe { read_utf8(server_url) }) else {
            return AUTH_FAILED;
        };
        let Some(device_identifier) = (unsafe { read_utf8(device_identifier) }) else {
            return AUTH_FAILED;
        };
        let Some(device_otp) = (unsafe { read_utf8(device_otp) }) else {
            return AUTH_FAILED;
        };
        let device_otp = Zeroizing::new(device_otp);
        let Some(server) = auth_server(server_mode, &server_url) else {
            return AUTH_FAILED;
        };

        let email = email.trim().to_lowercase();
        if email.is_empty() || password.is_empty() {
            return AUTH_FAILED;
        }
        // Use the same canonical device identity for token binding and SDK transport.
        let device_identifier = uuid::Uuid::parse_str(&device_identifier)
            .map(|id| id.to_string())
            .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
        let remember_identity =
            remembered_two_factor::identity(&server, &email, &device_identifier);
        let remembered = token
            .is_empty()
            .then(|| remembered_two_factor::load(&remember_identity))
            .flatten();
        let using_remembered = remembered.is_some();
        let two_factor = if token.is_empty() {
            remembered.map(|token| TwoFactorRequest {
                token: token.to_string(),
                provider: TwoFactorProvider::Remember,
                remember: false,
            })
        } else {
            let Some(provider) = two_factor_provider(provider) else {
                return AUTH_FAILED;
            };
            Some(TwoFactorRequest {
                token: if provider == TwoFactorProvider::Authenticator {
                    let normalized = Zeroizing::new(
                        token
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect::<String>(),
                    );
                    if normalized.len() != 6 || !normalized.bytes().all(|b| b.is_ascii_digit()) {
                        return INVALID_TWO_FACTOR_CODE;
                    }
                    normalized.to_string()
                } else {
                    token.to_string()
                },
                provider,
                remember: remember != 0,
            })
        };

        let mut request = SensitiveLoginRequest(PasswordLoginRequest {
            email,
            password: password.to_string(),
            two_factor,
        });

        let Some(runtime) = runtime() else {
            return AUTH_FAILED;
        };
        let mut session = accounts::sessions();
        if !auth_request::is_current(request_id) {
            return AUTH_CANCELLED;
        }
        if let Err(status) = session.can_login(&server, &request.0.email) {
            return status;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let pending = auth_client_for_server(
                &mut session.pending,
                &server,
                (&request.0.email, server_mode, server_url.trim()),
                &device_identifier,
            );
            pending.device_login_middleware.arm(&device_otp);
            let mut response = runtime.block_on(auth_request::run(
                request_id,
                pending
                    .client
                    .auth()
                    .login_password_with_unlock_data(&request.0),
            ));
            // Some self-hosted hosts reject provider 5 as an invalid code rather than
            // returning a challenge. Retry once without that credential to obtain the
            // real advertised methods; never retry a bad password/network/rate limit.
            if using_remembered
                && matches!(&response, Ok(Err(error)) if login_status_ref_is_invalid_code(error))
            {
                remembered_two_factor::clear(&remember_identity);
                if let Some(mut factor) = request.0.two_factor.take() {
                    factor.token.zeroize();
                }
                pending.device_login_middleware.arm(&device_otp);
                response = runtime.block_on(auth_request::run(
                    request_id,
                    pending
                        .client
                        .auth()
                        .login_password_with_unlock_data(&request.0),
                ));
            }
            pending.device_login_middleware.arm("");
            match response {
                Ok(Ok((mut response, unlock_data))) if auth_request::is_current(request_id) => {
                    let issued_token = response.two_factor_token.take().map(Zeroizing::new);
                    if response.authenticated {
                        if unlock_data.is_none() {
                            return AUTH_FAILED;
                        }
                        if !using_remembered && !token.is_empty() {
                            // Only an explicitly selected, successfully verified 2FA
                            // method may create/renew this grant. Normal reuse does not
                            // slide its local expiry another thirty days.
                            remembered_two_factor::clear(&remember_identity);
                            if remember != 0 {
                                if let Some(issued_token) = issued_token.as_deref() {
                                    remembered_two_factor::save(&remember_identity, issued_token);
                                }
                            }
                        }
                        let mut authenticated = session
                            .pending
                            .take()
                            .expect("successful login has a client");
                        authenticated.unlock_data = unlock_data;
                        authenticated.state = SESSION_UNLOCKED;
                        #[cfg(windows)]
                        authenticated.restore_local_vault(runtime);
                        // Cache restoration may take time for large vaults. Do not publish a
                        // session if a lock, account switch or cancellation arrived meanwhile.
                        if !auth_request::is_current(request_id) {
                            return AUTH_CANCELLED;
                        }
                        session.activate(authenticated);
                    } else if response.two_factor.is_some() {
                        remembered_two_factor::clear(&remember_identity);
                    }
                    login_status(Ok(response))
                }
                Ok(Ok(_)) => AUTH_CANCELLED,
                Ok(Err(error)) => login_status(Err(error)),
                Err(RequestError::TimedOut) => AUTH_TIMED_OUT,
                Err(RequestError::Cancelled) => AUTH_CANCELLED,
            }
        }))
        .unwrap_or(AUTH_FAILED);
        if !matches!(
            result & 0xff,
            AUTHENTICATED
                | TWO_FACTOR_REQUIRED
                | DEVICE_VERIFICATION_REQUIRED
                | DEVICE_VERIFICATION_INVALID
        ) {
            // Discard partially authenticated state if login failed after setting tokens.
            session.pending = None;
        }
        result
    }))
    .unwrap_or(AUTH_FAILED)
}

/// Send a Bitwarden email two-factor authentication code for the supplied account.
#[no_mangle]
pub unsafe extern "C" fn bw_auth_send_two_factor_email(
    email: *const c_char,
    password: *const c_char,
    server_mode: u32,
    server_url: *const c_char,
    request_id: u64,
) -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return 0;
        };
        let password = Zeroizing::new(password);
        let Some(email) = (unsafe { read_utf8(email) }) else {
            return 0;
        };
        let Some(server_url) = (unsafe { read_utf8(server_url) }) else {
            return 0;
        };
        let Some(server) = auth_server(server_mode, &server_url) else {
            return 0;
        };
        let email = email.trim().to_lowercase();
        if email.is_empty() || password.is_empty() {
            return 0;
        }
        let request = SensitiveEmailRequest(bw_sdk_core::auth::login::TwoFactorEmailRequest {
            email,
            password: password.to_string(),
        });

        let result = catch_unwind(AssertUnwindSafe(|| {
            let runtime = runtime().ok_or(())?;
            let mut session = accounts::sessions();
            if let Err(status) = session.can_login(&server, &request.0.email) {
                return Ok(status as u8);
            }
            let device_identifier = session
                .pending
                .as_ref()
                .map(|current| current.device_identifier.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let pending = auth_client_for_server(
                &mut session.pending,
                &server,
                (&request.0.email, server_mode, server_url.trim()),
                &device_identifier,
            );
            let response = runtime.block_on(auth_request::run(
                request_id,
                pending.client.auth().send_two_factor_email(&request.0),
            ));
            let status = match response {
                Ok(Ok(())) => AUTHENTICATED as u8,
                Ok(Err(error)) => match error {
                    bw_sdk_core::auth::login::TwoFactorEmailError::Api(error) => {
                        auth_api_error_status(&error) as u8
                    }
                    bw_sdk_core::auth::login::TwoFactorEmailError::Prelogin(error) => {
                        auth_prelogin_error_status(&error) as u8
                    }
                    _ => AUTH_FAILED as u8,
                },
                Err(RequestError::TimedOut) => AUTH_TIMED_OUT as u8,
                Err(RequestError::Cancelled) => AUTH_CANCELLED as u8,
            };
            if status == AUTH_TIMED_OUT as u8 || status == AUTH_CANCELLED as u8 {
                session.pending = None;
            }
            Ok::<_, ()>(status)
        }));

        match result {
            Ok(Ok(status)) => status,
            _ => 0,
        }
    }))
    .unwrap_or(0)
}

/// Resend the email code for Bitwarden new-device verification.
#[no_mangle]
pub unsafe extern "C" fn bw_auth_resend_new_device_verification(
    email: *const c_char,
    password: *const c_char,
    device_identifier: *const c_char,
    server_mode: u32,
    server_url: *const c_char,
    request_id: u64,
) -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        let (Some(email), Some(password), Some(device_identifier), Some(server_url)) = (
            unsafe { read_utf8(email) },
            unsafe { read_utf8(password) },
            unsafe { read_utf8(device_identifier) },
            unsafe { read_utf8(server_url) },
        ) else {
            return AUTH_FAILED as u8;
        };
        let password = Zeroizing::new(password);
        let email = email.trim().to_lowercase();
        let Some(server) = auth_server(server_mode, &server_url) else {
            return AUTH_FAILED as u8;
        };
        if email.is_empty()
            || password.is_empty()
            || uuid::Uuid::parse_str(&device_identifier).is_err()
        {
            return AUTH_FAILED as u8;
        }

        let result = catch_unwind(AssertUnwindSafe(|| -> Result<u8, ()> {
            let runtime = runtime().ok_or(())?;
            let mut session = accounts::sessions();
            if let Err(status) = session.can_login(&server, &email) {
                return Ok(status as u8);
            }
            let pending = auth_client_for_server(
                &mut session.pending,
                &server,
                (&email, server_mode, server_url.trim()),
                &device_identifier,
            );
            let kdf = runtime.block_on(auth_request::run(
                request_id,
                pending.client.auth().prelogin(email.clone()),
            ));
            let kdf = match kdf {
                Ok(Ok(kdf)) => kdf,
                Err(RequestError::TimedOut) => return Ok(AUTH_TIMED_OUT as u8),
                Err(RequestError::Cancelled) => return Ok(AUTH_CANCELLED as u8),
                Ok(Err(error)) => return Ok(auth_prelogin_error_status(&error) as u8),
            };
            let auth_data = MasterPasswordAuthenticationData::derive(&password, &kdf, &email)
                .map_err(|_| ())?;
            let password_hash =
                Zeroizing::new(auth_data.master_password_authentication_hash.to_string());
            drop(auth_data);
            let config = pending.client.internal.get_api_configurations();
            let endpoint = format!(
                "{}/accounts/resend-new-device-otp",
                config.api_config.base_path.trim_end_matches('/')
            );
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct ResendRequest<'a> {
                email: &'a str,
                master_password_hash: &'a str,
            }
            let request = config
                .api_config
                .client
                .post(endpoint)
                .header("Device-Identifier", &device_identifier)
                .json(&ResendRequest {
                    email: &email,
                    master_password_hash: &password_hash,
                });
            match runtime.block_on(auth_request::run(request_id, request.send())) {
                Ok(Ok(response)) if response.status().is_success() => Ok(AUTHENTICATED as u8),
                Ok(Ok(response)) => Ok(auth_http_status(response.status()) as u8),
                Ok(Err(reqwest_middleware::Error::Reqwest(error))) => {
                    Ok(auth_reqwest_error_status(&error) as u8)
                }
                Ok(Err(reqwest_middleware::Error::Middleware(_))) => Ok(AUTH_FAILED as u8),
                Err(RequestError::TimedOut) => Ok(AUTH_TIMED_OUT as u8),
                Err(RequestError::Cancelled) => Ok(AUTH_CANCELLED as u8),
            }
        }));

        match result {
            Ok(Ok(status)) => status,
            _ => AUTH_FAILED as u8,
        }
    }))
    .unwrap_or(AUTH_FAILED as u8)
}

#[no_mangle]
pub extern "C" fn bw_auth_is_authenticated() -> u8 {
    u8::from(bw_auth_session_state() == SESSION_UNLOCKED)
}

#[no_mangle]
pub extern "C" fn bw_auth_session_state() -> u8 {
    SESSION_STATE.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn bw_auth_unlock(password: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return vault_error("wrongPassword");
        };
        let password = Zeroizing::new(password);
        complete_vault_unlock(request_id, |session, keys| {
            keys.unlock(&session.client, &password)
                .map_err(|_| "wrongPassword".to_owned())
        })
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

fn complete_vault_unlock(
    request_id: u64,
    initialize: impl FnOnce(&AuthSession, &PasswordLoginUnlockData) -> Result<(), String>,
) -> *mut c_char {
    match complete_vault_unlock_raw(request_id, initialize) {
        Ok(()) => vault_success(&true),
        Err(code) => vault_error(&code),
    }
}

fn complete_vault_unlock_raw(
    request_id: u64,
    initialize: impl FnOnce(&AuthSession, &PasswordLoginUnlockData) -> Result<(), String>,
) -> Result<(), String> {
    let Some(runtime) = runtime() else {
        return Err("internal".to_owned());
    };
    let mut guard = auth_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session) = guard
        .active
        .as_mut()
        .filter(|_| bw_auth_session_state() == SESSION_LOCKED)
    else {
        return Err("unlockUnavailable".to_owned());
    };
    if session.unlock_data.is_none() {
        return Err("unlockUnavailable".to_owned());
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(auth_request::run(request_id, async {
            let keys = session.unlock_data.as_ref().ok_or("unlockUnavailable")?;
            initialize(session, keys)?;
            if session.offline_only {
                #[cfg(windows)]
                session
                    .client
                    .platform()
                    .state()
                    .value::<bw_sdk_core::client::persisted_state::USER_LOGIN_METHOD>()
                    .set(
                        bw_sdk_core::client::login_method::UserLoginMethod::Username {
                            client_id: "web".to_owned(), // Mirrors the SDK password flow.
                            email: keys.master_password.salt.clone(),
                            kdf: keys.master_password.kdf.clone(),
                        },
                    )
                    .await
                    .map_err(|_| "internal".to_owned())?;
            }
            session
                .vault
                .restore_overview(&session.client)
                .await
                .map_err(|error| error.to_string())
        }))
    }));
    let code = match result {
        Ok(Ok(Ok(()))) if auth_request::is_current(request_id) => {
            session.state = SESSION_UNLOCKED;
            guard.publish_state();
            return Ok(());
        }
        Ok(Ok(Ok(()))) | Ok(Err(RequestError::Cancelled)) => "cancelled".to_owned(),
        Ok(Err(RequestError::TimedOut)) => "timedOut".to_owned(),
        Ok(Ok(Err(error))) => error.to_string(),
        Err(_) => "internal".to_owned(),
    };
    if !session.clear_vault_keys() {
        guard.remove_active();
        return Err("internal".to_owned());
    }
    Err(code)
}

fn vault_access_error() -> *mut c_char {
    if bw_auth_session_state() == SESSION_LOCKED {
        vault_error("locked")
    } else {
        vault_error("notAuthenticated")
    }
}

fn vault_error(code: &str) -> *mut c_char {
    into_raw_string(serde_json::json!({"ok": false, "error": code}).to_string())
}

fn vault_success(value: &impl serde::Serialize) -> *mut c_char {
    into_raw_string(serde_json::json!({"ok": true, "data": value}).to_string())
}

#[no_mangle]
pub extern "C" fn bw_vault_overview() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        if session.vault.needs_reconciliation() {
            return vault_error("notSynced");
        }
        match session.vault.overview() {
            Ok(overview) => vault_success(overview),
            Err(error) => vault_error(&error.to_string()),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub extern "C" fn bw_vault_sync(request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        if session.offline_only {
            return vault_error("signInRequired");
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(auth_request::run(
                request_id,
                VaultCache::fetch(&session.client, &mut session.unlock_data),
            ))
        }));
        // Sync can discover key rotation before rejecting other data or being cancelled.
        // Revocation must not depend on installing the replacement overview.
        #[cfg(windows)]
        if session.unlock_data.is_none() {
            pin::clear_session(session);
            if !remove_cached_vault(session) {
                return vault_error("cacheRevocationFailed");
            }
        }
        match result {
            Ok(Ok(Ok(cache))) if auth_request::is_current(request_id) => {
                cache.activate(&session.client);
                session.vault = cache;
                session.open_detail = None;
                #[cfg(windows)]
                persist_cached_vault(session);
                #[cfg(windows)]
                runtime.block_on(crate::passkey_provider::sync_metadata(
                    &session.vault,
                    &session.client,
                ));
                vault_success(
                    session
                        .vault
                        .overview()
                        .expect("successful sync has an overview"),
                )
            }
            Ok(Ok(Ok(_))) | Ok(Err(RequestError::Cancelled)) => vault_error("cancelled"),
            Ok(Err(RequestError::TimedOut)) => vault_error("timedOut"),
            Ok(Ok(Err(error))) => vault_error(&error.to_string()),
            Err(_) => vault_error("internal"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_detail(
    id: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(id) = (unsafe { read_utf8(id) }) else {
            return vault_error("notFound");
        };
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return vault_error("passwordRequired");
        };
        let password = Zeroizing::new(password);
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        match runtime.block_on(auth_request::run(
            request_id,
            session.vault.detail(&session.client, &id, &password),
        )) {
            Ok(Ok(mut detail)) if auth_request::is_current(request_id) => {
                // Reuse this authorization only for the currently open item. The cache remains encrypted.
                let has_totp = matches!(detail.totp, Some(VaultTotp::Ready { .. }));
                let has_attachments = !session.vault.is_read_only()
                    && (!detail.attachment_files.is_empty() || detail.can_upload_attachments);
                detail.totp_request_id = has_totp.then_some(request_id);
                detail.attachment_request_id = has_attachments.then_some(request_id);
                session.open_detail = (has_totp || has_attachments).then(|| OpenDetail {
                    request_id,
                    item_id: id,
                    revision: detail.revision_date.clone(),
                    has_totp,
                    has_attachments,
                });
                vault_success(&detail)
            }
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(error)) => vault_error(&error.to_string()),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub extern "C" fn bw_vault_totp_refresh(request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if bw_auth_session_state() != SESSION_UNLOCKED {
            return vault_access_error();
        }
        if !auth_request::is_current(request_id) {
            return vault_error("totpViewExpired");
        }
        let guard = match auth_client().try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return vault_error("totpBusy"),
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        let Some(session) = guard
            .active
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        let Some(detail) = session
            .open_detail
            .as_ref()
            .filter(|detail| detail.request_id == request_id && detail.has_totp)
        else {
            return vault_error("totpViewExpired");
        };
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        match runtime.block_on(auth_request::run(
            request_id,
            session.vault.refresh_totp(&session.client, &detail.item_id),
        )) {
            Ok(Ok(code)) if auth_request::is_current(request_id) => vault_success(&code),
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("totpViewExpired"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(error)) => vault_error(&error.to_string()),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_download_attachment(
    attachment_id: *const c_char,
    destination: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let (Some(attachment_id), Some(destination)) =
            (unsafe { read_utf8(attachment_id) }, unsafe {
                read_utf8(destination)
            })
        else {
            return vault_error("validation");
        };
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        if !auth_request::is_current(request_id) {
            return vault_error("attachmentViewExpired");
        }
        let Some(detail) = session
            .open_detail
            .as_ref()
            .filter(|detail| detail.request_id == request_id && detail.has_attachments)
        else {
            return vault_error("attachmentViewExpired");
        };
        let destination = Path::new(&destination);
        if !destination.is_absolute() || destination.file_name().is_none() {
            return vault_error("fileAccess");
        }
        let Ok(replace_existing) = destination.try_exists() else {
            return vault_error("fileAccess");
        };
        match runtime.block_on(auth_request::run_transfer(
            request_id,
            session.vault.decrypt_attachment(
                &session.client,
                &session.server.api_url,
                &detail.item_id,
                &detail.revision,
                &attachment_id,
            ),
        )) {
            Ok(Ok(contents)) => match crate::vault::attachments::save_download(
                &contents,
                destination,
                replace_existing,
                request_id,
            ) {
                Ok(()) => vault_success(&true),
                Err(error) => vault_error(&error.to_string()),
            },
            Ok(Err(error)) => vault_error(&error.to_string()),
            Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_editor(
    id: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(id) = (unsafe { read_utf8(id) }) else {
            return vault_error("notFound");
        };
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return vault_error("passwordRequired");
        };
        let password = Zeroizing::new(password);
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        match runtime.block_on(auth_request::run(
            request_id,
            session.vault.editor(&session.client, &id, &password),
        )) {
            Ok(Ok(data)) if auth_request::is_current(request_id) => vault_success(&data),
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(error)) => vault_error(&error.to_string()),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

enum VaultWriteCommand {
    Create(VaultEditorData),
    Update(VaultEditorData, Zeroizing<String>),
    CreateFolder(String),
    RenameFolder {
        id: String,
        name: String,
    },
    DeleteFolders(Vec<FolderId>),
    MoveItems(VaultItemMove),
    UploadAttachment(String),
    DeleteAttachment(String),
    Import {
        account_id: uuid::Uuid,
        source: String,
        password: Zeroizing<String>,
    },
    Action {
        id: String,
        action: String,
        revision: String,
        password: Zeroizing<String>,
    },
    BulkAction {
        ids: Vec<CipherId>,
        action: String,
        password: Zeroizing<String>,
    },
}

fn perform_vault_write(command: VaultWriteCommand, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        if let Err(error) = session.vault.ensure_synced() {
            return vault_error(&error.to_string());
        }
        if let VaultWriteCommand::Import { account_id, .. } = &command {
            if session.account_id != *account_id {
                return vault_error("cancelled");
            }
        }
        let importing = matches!(&command, VaultWriteCommand::Import { .. });
        session.vault.reset_write_attempt();
        #[cfg(windows)]
        let cache_write = match CacheWrite::begin(session) {
            Ok(write) => write,
            Err(error) => return vault_error(&error.to_string()),
        };
        let upload = matches!(&command, VaultWriteCommand::UploadAttachment(_));
        let mut pending_upload = None;
        #[cfg(windows)]
        let affects_passkeys = matches!(
            &command,
            VaultWriteCommand::Create(..)
                | VaultWriteCommand::Update(..)
                | VaultWriteCommand::Import { .. }
                | VaultWriteCommand::Action { .. }
                | VaultWriteCommand::BulkAction { .. }
        );
        // Catch inside the session guard: a panic after sending a write must force reconciliation.
        let result = catch_unwind(AssertUnwindSafe(|| {
            let write = async {
                match command {
                    VaultWriteCommand::Create(data) => {
                        session.vault.create(&session.client, data).await
                    }
                    VaultWriteCommand::Update(data, password) => {
                        session.vault.update(&session.client, data, &password).await
                    }
                    VaultWriteCommand::CreateFolder(name) => {
                        session.vault.create_folder(&session.client, &name).await
                    }
                    VaultWriteCommand::RenameFolder { id, name } => {
                        session
                            .vault
                            .rename_folder(&session.client, &id, &name)
                            .await
                    }
                    VaultWriteCommand::DeleteFolders(ids) => {
                        session.vault.delete_folders(&session.client, ids).await
                    }
                    VaultWriteCommand::MoveItems(args) => {
                        session
                            .vault
                            .move_items(&session.client, args.ids, args.folder_id)
                            .await
                    }
                    VaultWriteCommand::UploadAttachment(path) => {
                        let detail = session
                            .open_detail
                            .as_ref()
                            .filter(|detail| {
                                detail.request_id == request_id && detail.has_attachments
                            })
                            .ok_or(VaultError::AttachmentViewExpired)?;
                        Box::pin(session.vault.upload_attachment(
                            &session.client,
                            (&detail.item_id, &detail.revision),
                            Path::new(&path),
                            request_id,
                            &mut pending_upload,
                        ))
                        .await
                    }
                    VaultWriteCommand::DeleteAttachment(id) => {
                        let detail = session
                            .open_detail
                            .as_ref()
                            .filter(|detail| {
                                detail.request_id == request_id && detail.has_attachments
                            })
                            .ok_or(VaultError::AttachmentViewExpired)?;
                        Box::pin(session.vault.delete_attachment(
                            &session.client,
                            (&detail.item_id, &detail.revision),
                            &id,
                        ))
                        .await
                    }
                    VaultWriteCommand::Import {
                        source, password, ..
                    } => Box::pin(export::import(session, &source, &password, request_id)).await,
                    VaultWriteCommand::Action {
                        id,
                        action,
                        revision,
                        password,
                    } => {
                        session
                            .vault
                            .action(&session.client, &id, &action, &revision, &password)
                            .await
                    }
                    VaultWriteCommand::BulkAction {
                        ids,
                        action,
                        password,
                    } => {
                        session
                            .vault
                            .bulk_action(&session.client, ids, &action, &password)
                            .await
                    }
                }
            };
            if upload || importing {
                runtime.block_on(auth_request::run_transfer(request_id, Box::pin(write)))
            } else {
                runtime.block_on(auth_request::run(request_id, write))
            }
        }));
        #[cfg(windows)]
        let rolled_back_upload = pending_upload.is_some();
        if let Some(pending) = pending_upload {
            // Once the upload future stops, rollback only the slot created by this request.
            let recovered = catch_unwind(AssertUnwindSafe(|| {
                runtime.block_on(Box::pin(
                    session.vault.rollback_upload(&session.client, pending),
                ))
            }))
            .unwrap_or(false);
            if !recovered || session.vault.needs_reconciliation() {
                session.vault.mark_write_uncertain();
                return vault_error("writeUncertain");
            }
            session.open_detail = None;
        }
        let interrupted = matches!(result, Ok(Err(_)) | Err(_)) && session.vault.write_in_flight();
        if interrupted {
            session.vault.mark_write_uncertain();
        }
        #[cfg(windows)]
        if importing && session.unlock_data.is_none() {
            pin::clear_session(session);
            if !remove_cached_vault(session) {
                return vault_error("cacheRevocationFailed");
            }
        }
        #[cfg(windows)]
        cache_write.finish(
            session,
            rolled_back_upload || matches!(result, Ok(Ok(Ok(_)))),
        );
        match result {
            // An acknowledged success is authoritative even if cancellation races its return.
            Ok(Ok(Ok(result))) => {
                #[cfg(windows)]
                if affects_passkeys {
                    runtime.block_on(crate::passkey_provider::sync_metadata(
                        &session.vault,
                        &session.client,
                    ));
                }
                vault_write_success(&result)
            }
            Ok(Ok(Err(error))) => vault_error(&error.to_string()),
            _ if interrupted => vault_error("writeUncertain"),
            Ok(Err(RequestError::Cancelled)) => vault_error("cancelled"),
            Ok(Err(RequestError::TimedOut)) => vault_error("timedOut"),
            Err(_) => vault_error("internal"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_upload_attachment(
    path: *const c_char,
    detail_request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(path) = (unsafe { read_utf8(path) }) else {
            return vault_error("attachmentFileRead");
        };
        perform_vault_write(VaultWriteCommand::UploadAttachment(path), detail_request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_delete_attachment(
    attachment_id: *const c_char,
    detail_request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(id) = (unsafe { read_utf8(attachment_id) }) else {
            return vault_error("validation");
        };
        perform_vault_write(VaultWriteCommand::DeleteAttachment(id), detail_request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

fn vault_write_success(result: &VaultMutationResult) -> *mut c_char {
    vault_success(result)
}

unsafe fn read_editor_json(value: *const c_char) -> Option<VaultEditorData> {
    let json = Zeroizing::new(unsafe { read_utf8(value) }?);
    if json.len() > 1_048_576 {
        return None;
    }
    serde_json::from_str(&json).ok()
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_create(json: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(data) = (unsafe { read_editor_json(json) }) else {
            return vault_error("validation");
        };
        perform_vault_write(VaultWriteCommand::Create(data), request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_update(
    json: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(data) = (unsafe { read_editor_json(json) }) else {
            return vault_error("validation");
        };
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return vault_error("passwordRequired");
        };
        perform_vault_write(
            VaultWriteCommand::Update(data, Zeroizing::new(password)),
            request_id,
        )
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_folder_create(name: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(name) = (unsafe { read_utf8(name) }) else {
            return vault_error("validation");
        };
        perform_vault_write(VaultWriteCommand::CreateFolder(name), request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_folder_rename(
    id: *const c_char,
    name: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let (Some(id), Some(name)) = (unsafe { read_utf8(id) }, unsafe { read_utf8(name) }) else {
            return vault_error("validation");
        };
        perform_vault_write(VaultWriteCommand::RenameFolder { id, name }, request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_folders_delete(ids: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(json) = (unsafe { read_utf8(ids) }) else {
            return vault_error("validation");
        };
        let Ok(ids) = serde_json::from_str(&json) else {
            return vault_error("validation");
        };
        perform_vault_write(VaultWriteCommand::DeleteFolders(ids), request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultItemMove {
    ids: Vec<CipherId>,
    folder_id: Option<FolderId>,
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_move_items(data: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(data) = (unsafe { read_utf8(data) }) else {
            return vault_error("validation");
        };
        let Ok(args) = serde_json::from_str(&data) else {
            return vault_error("validation");
        };
        perform_vault_write(VaultWriteCommand::MoveItems(args), request_id)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_action(
    id: *const c_char,
    action: *const c_char,
    revision: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let (Some(id), Some(action), Some(revision), Some(password)) = (
            unsafe { read_utf8(id) },
            unsafe { read_utf8(action) },
            unsafe { read_utf8(revision) },
            unsafe { read_utf8(password) },
        ) else {
            return vault_error("validation");
        };
        perform_vault_write(
            VaultWriteCommand::Action {
                id,
                action,
                revision,
                password: Zeroizing::new(password),
            },
            request_id,
        )
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultBulkActionRequest {
    ids: Vec<CipherId>,
    action: String,
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_bulk_action(
    json: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let (Some(json), Some(password)) =
            (unsafe { read_utf8(json) }, unsafe { read_utf8(password) })
        else {
            return vault_error("validation");
        };
        let password = Zeroizing::new(password);
        let Ok(args) = serde_json::from_str::<VaultBulkActionRequest>(&json) else {
            return vault_error("validation");
        };
        perform_vault_write(
            VaultWriteCommand::BulkAction {
                ids: args.ids,
                action: args.action,
                password,
            },
            request_id,
        )
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_password_strength(
    password: *const c_char,
    username: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return -1;
        };
        let password = Zeroizing::new(password);
        if password.is_empty() {
            return -1;
        }
        let username = unsafe { read_utf8(username) }.filter(|value| !value.is_empty());
        let Some(runtime) = runtime() else {
            return -1;
        };
        let details = bitwarden_vault::CipherLoginDetails {
            id: Default::default(),
            password: password.to_string(),
            username,
        };
        let options = bitwarden_vault::CipherRiskOptions {
            password_map: None,
            check_exposed: false,
            hibp_base_url: None,
        };
        use bitwarden_vault::VaultClientExt;
        runtime
            .block_on(
                Client::new(None)
                    .vault()
                    .cipher_risk()
                    .compute_risk(vec![details], options),
            )
            .ok()
            .and_then(|results| results.into_iter().next())
            .map_or(-1, |result| i32::from(result.password_strength))
    }))
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn bw_check_password_exposed(
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return vault_error("validation");
        };
        let password = Zeroizing::new(password);
        if password.is_empty() {
            return vault_error("validation");
        }
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let details = bitwarden_vault::CipherLoginDetails {
            id: Default::default(),
            password: password.to_string(),
            username: None,
        };
        let options = bitwarden_vault::CipherRiskOptions {
            password_map: None,
            check_exposed: true,
            hibp_base_url: None,
        };
        use bitwarden_vault::{ExposedPasswordResult, VaultClientExt};
        match runtime.block_on(auth_request::run(
            request_id,
            Client::new(None)
                .vault()
                .cipher_risk()
                .compute_risk(vec![details], options),
        )) {
            Ok(Ok(results)) if auth_request::is_current(request_id) => {
                match results.into_iter().next().map(|r| r.exposed_result) {
                    Some(ExposedPasswordResult::Found(count)) => vault_success(&count),
                    Some(ExposedPasswordResult::Error(_)) => vault_error("hibpError"),
                    _ => vault_error("validation"),
                }
            }
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(_)) => vault_error("hibpError"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_generate_username(json: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(json) = (unsafe { read_utf8(json) }) else {
            return vault_error("validation");
        };
        let json = Zeroizing::new(json);
        if json.len() > 16_384 {
            return vault_error("validation");
        }
        let Ok(request) =
            serde_json::from_str::<bitwarden_generators::UsernameGeneratorRequest>(&json)
        else {
            return vault_error("validation");
        };
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        match runtime.block_on(auth_request::run(request_id, generator().username(request))) {
            Ok(Ok(value)) if auth_request::is_current(request_id) => vault_success(&value),
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(bitwarden_generators::UsernameError::InvalidApiKey)) => {
                vault_error("validation")
            }
            Ok(Err(_)) => vault_error("network"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub extern "C" fn bw_vault_fingerprint() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        let Some(user_id) = session.vault.user_id() else {
            return vault_error("notSynced");
        };
        match session
            .client
            .platform()
            .user_fingerprint(user_id.to_string())
        {
            Ok(phrase) => vault_success(&phrase),
            Err(_) => vault_error("decryptionFailed"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    let owned = unsafe { CString::from_raw(value) };
    let length = owned.as_bytes_with_nul().len();
    unsafe { std::ptr::write_bytes(owned.as_ptr().cast_mut(), 0, length) };
    drop(owned);
}

#[cfg(test)]
mod tests {
    use super::{auth_server, custom_auth_server, generate_passphrase, generate_password};
    use bitwarden_generators::{GeneratorClientsExt, PasswordGeneratorRequest};
    use bw_sdk_core::Client;

    #[test]
    fn generated_password_obeys_length_and_character_options() {
        let password = generate_password(24, 1 | 2 | 4 | 8, 2, 2).expect("valid options");
        assert_eq!(password.len(), 24);
        assert!(password.bytes().any(|byte| byte.is_ascii_lowercase()));
        assert!(password.bytes().any(|byte| byte.is_ascii_uppercase()));
        assert!(password.bytes().filter(u8::is_ascii_digit).count() >= 2);
        assert!(
            password
                .bytes()
                .filter(|byte| b"!@#$%^&*".contains(byte))
                .count()
                >= 2
        );
    }

    #[test]
    fn password_generator_clamps_length_and_respects_ambiguous_setting() {
        let password = generate_password(1, 1 | 2 | 16, 0, 0).expect("valid options");
        assert_eq!(password.len(), 5);
        assert!(!password.bytes().any(|byte| b"IOl01".contains(&byte)));
        assert_eq!(
            generate_password(1000, 1, 0, 0)
                .expect("valid options")
                .len(),
            128
        );
    }

    #[test]
    fn impossible_password_settings_are_rejected() {
        assert!(generate_password(5, 1 | 2 | 4, 4, 0).is_none());
        assert!(generate_password(20, 0, 0, 0).is_none());
        assert!(generate_password(20, 4, 10, 0).is_none());
    }

    #[test]
    fn passphrase_options_match_desktop_defaults_and_bounds() {
        let passphrase = generate_passphrase(6, "-", false, false).expect("valid options");
        assert_eq!(passphrase.split('-').count(), 6);
        assert!(generate_passphrase(2, "-", false, false).is_none());
        assert!(generate_passphrase(21, "-", false, false).is_none());
    }

    #[test]
    fn passphrase_can_capitalize_and_include_a_number() {
        let passphrase = generate_passphrase(6, "-", true, true).expect("valid options");
        let words: Vec<_> = passphrase.split('-').collect();
        assert_eq!(words.len(), 6);
        assert!(words
            .iter()
            .all(|word| word.chars().next().unwrap().is_uppercase()));
        assert_eq!(passphrase.chars().filter(char::is_ascii_digit).count(), 1);
    }

    #[test]
    fn sdk_generator_is_available_without_a_vault_session() {
        let generated = Client::new(None)
            .generator()
            .password(PasswordGeneratorRequest::default())
            .expect("generator does not require a vault session");
        assert_eq!(generated.len(), 16);
    }

    #[test]
    fn auth_rejects_empty_credentials_without_network_access() {
        let email = std::ffi::CString::new(" ").expect("email has no NUL");
        let password = std::ffi::CString::new("test-password").expect("password has no NUL");
        let token = std::ffi::CString::new("").expect("token has no NUL");
        let server_url = std::ffi::CString::new("").expect("server URL has no NUL");
        let result = unsafe {
            super::bw_auth_login(
                email.as_ptr(),
                password.as_ptr(),
                0,
                token.as_ptr(),
                0,
                server_url.as_ptr(),
                std::ffi::CString::new("11111111-1111-4111-8111-111111111111")
                    .unwrap()
                    .as_ptr(),
                token.as_ptr(),
                0,
            )
        };
        assert_eq!(result, super::AUTH_FAILED);
    }

    #[test]
    fn refresh_never_consumes_or_transmits_the_armed_new_device_otp() {
        let middleware = super::DeviceLoginMiddleware::new("synthetic-device".to_owned());
        middleware.arm("246810");
        let endpoint = "https://identity.example.invalid/connect/token"
            .parse()
            .unwrap();
        let mut request = reqwest::Request::new(reqwest::Method::POST, endpoint);
        let refresh = "grant_type=refresh_token&refresh_token=synthetic";
        *request.body_mut() = Some(refresh.into());
        middleware.rewrite_login_request(&mut request);
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            refresh.as_bytes()
        );
        *request.body_mut() =
            Some("grant_type=password&twoFactorProvider=0&twoFactorToken=012345".into());
        middleware.rewrite_login_request(&mut request);
        let body = request.body().unwrap().as_bytes().unwrap();
        let fields: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body).collect();
        assert_eq!(fields["deviceIdentifier"], "synthetic-device");
        assert_eq!(fields["newDeviceOtp"], "246810");
        assert_eq!(fields["twoFactorToken"], "012345");
    }

    #[test]
    fn auth_rejects_unimplemented_two_factor_provider_without_network_access() {
        let email = std::ffi::CString::new("user@example.com").expect("email has no NUL");
        let password = std::ffi::CString::new("test-password").expect("password has no NUL");
        let token = std::ffi::CString::new("123456").expect("token has no NUL");
        let server_url = std::ffi::CString::new("").expect("server URL has no NUL");
        let result = unsafe {
            super::bw_auth_login(
                email.as_ptr(),
                password.as_ptr(),
                2,
                token.as_ptr(),
                0,
                server_url.as_ptr(),
                std::ffi::CString::new("11111111-1111-4111-8111-111111111111")
                    .unwrap()
                    .as_ptr(),
                token.as_ptr(),
                0,
            )
        };
        assert_eq!(result, super::AUTH_FAILED);
    }

    #[test]
    fn auth_logout_clears_in_memory_session_state() {
        let _guard = super::AUTH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::SESSION_STATE.store(
            super::SESSION_UNLOCKED,
            std::sync::atomic::Ordering::Release,
        );
        super::accounts::bw_auth_logout_all();
        assert_eq!(super::bw_auth_is_authenticated(), 0);
    }

    #[test]
    fn built_in_server_regions_use_their_matching_api_and_identity_hosts() {
        let us = auth_server(0, "").expect("US cloud region");
        assert_eq!(us.api_url, "https://api.bitwarden.com");
        assert_eq!(us.identity_url, "https://identity.bitwarden.com");

        let eu = auth_server(1, "").expect("EU cloud region");
        assert_eq!(eu.api_url, "https://api.bitwarden.eu");
        assert_eq!(eu.identity_url, "https://identity.bitwarden.eu");
        assert!(auth_server(3, "").is_none());
    }

    #[test]
    fn custom_server_builds_api_and_identity_urls_from_server_base() {
        let server =
            custom_auth_server(" https://vault.example.com/ ").expect("valid server base URL");
        assert_eq!(server.api_url, "https://vault.example.com/api");
        assert_eq!(server.identity_url, "https://vault.example.com/identity");

        let nested = custom_auth_server("https://vault.example.com/bitwarden/")
            .expect("valid server base URL with path prefix");
        assert_eq!(nested.api_url, "https://vault.example.com/bitwarden/api");
        assert_eq!(
            nested.identity_url,
            "https://vault.example.com/bitwarden/identity"
        );
    }

    #[test]
    fn custom_server_rejects_unsafe_or_ambiguous_urls() {
        for value in [
            "",
            "vault.example.com",
            "ftp://vault.example.com",
            "https://user:password@vault.example.com",
            "https://vault.example.com?tenant=one",
            "https://vault.example.com/#fragment",
            "http://vault.example.com",
            "https://vault.example.com/api",
            "https://vault.example.com/api/",
            "https://vault.example.com/identity",
            "https://vault.example.com/identity/",
        ] {
            assert!(
                custom_auth_server(value).is_none(),
                "unexpectedly accepted {value}"
            );
        }
        assert_eq!(
            custom_auth_server("http://localhost:8080")
                .expect("loopback HTTP server")
                .api_url,
            "http://localhost:8080/api"
        );
    }

    unsafe fn take_ffi_json(ptr: *mut std::ffi::c_char) -> serde_json::Value {
        let text = unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_str()
            .expect("FFI string is valid UTF-8")
            .to_owned();
        unsafe { super::bw_string_free(ptr) };
        serde_json::from_str(&text).expect("FFI result is JSON")
    }

    #[test]
    fn password_strength_scores_weak_and_strong_passwords() {
        let empty = std::ffi::CString::new("").unwrap();
        let weak = std::ffi::CString::new("123456").unwrap();
        let strong = std::ffi::CString::new("Correct-Horse-Battery-Staple-99!@").unwrap();
        let user = std::ffi::CString::new("alice").unwrap();
        unsafe {
            assert_eq!(
                super::bw_password_strength(std::ptr::null(), user.as_ptr()),
                -1
            );
            assert_eq!(
                super::bw_password_strength(empty.as_ptr(), user.as_ptr()),
                -1
            );
            assert!((0..=1).contains(&super::bw_password_strength(weak.as_ptr(), user.as_ptr())));
            assert_eq!(
                super::bw_password_strength(strong.as_ptr(), user.as_ptr()),
                4
            );
        }
    }

    #[test]
    fn username_generator_supports_word_subaddress_and_catchall() {
        let _guard = super::AUTH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (payload, check) in [
            (
                r#"{"word":{"capitalize":true,"include_number":true}}"#,
                Box::new(|value: &str| {
                    !value.is_empty() && value.chars().any(|c| c.is_ascii_digit())
                }) as Box<dyn Fn(&str) -> bool>,
            ),
            (
                r#"{"subaddress":{"type":"random","email":"user@example.com"}}"#,
                Box::new(|value: &str| {
                    value.starts_with("user+") && value.ends_with("@example.com")
                }),
            ),
            (
                r#"{"catchall":{"type":"random","domain":"example.com"}}"#,
                Box::new(|value: &str| {
                    value.ends_with("@example.com") && value.len() > "@example.com".len()
                }),
            ),
        ] {
            let json = std::ffi::CString::new(payload).unwrap();
            let request_id = crate::auth_request::begin();
            let response =
                unsafe { take_ffi_json(super::bw_generate_username(json.as_ptr(), request_id)) };
            assert_eq!(response["ok"], true, "failed payload {payload}: {response}");
            let generated = response["data"].as_str().unwrap();
            assert!(
                check(generated),
                "unexpected generated username {generated}"
            );
        }
        let invalid = std::ffi::CString::new("{invalid").unwrap();
        let request_id = crate::auth_request::begin();
        let response =
            unsafe { take_ffi_json(super::bw_generate_username(invalid.as_ptr(), request_id)) };
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"], "validation");
    }

    #[test]
    fn vault_fingerprint_requires_unlocked_synced_session_and_returns_five_words() {
        struct SessionReset;
        impl Drop for SessionReset {
            fn drop(&mut self) {
                super::accounts::bw_auth_logout_all();
            }
        }

        let _guard = super::AUTH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _reset = SessionReset;
        super::accounts::bw_auth_logout_all();
        let signed_out = unsafe { take_ffi_json(super::bw_vault_fingerprint()) };
        assert_eq!(signed_out["error"], "notAuthenticated");

        let runtime = super::runtime().expect("runtime");
        let client = runtime.block_on(Client::init_test_account(
            bw_sdk_core::client::test_accounts::test_bitwarden_com_account(),
        ));
        let user_id = client.internal.get_user_id().unwrap();
        let sync_response = bitwarden_api_api::models::SyncResponseModel {
            profile: Some(Box::new(bitwarden_api_api::models::ProfileResponseModel {
                id: Some(user_id.to_string().parse().unwrap()),
                organizations: Some(vec![]),
                ..Default::default()
            })),
            ciphers: Some(vec![]),
            folders: Some(vec![]),
            ..Default::default()
        };
        let vault = runtime
            .block_on(crate::vault::VaultCache::from_response(
                &client,
                sync_response,
                &mut None,
            ))
            .unwrap();
        let server = auth_server(0, "").unwrap();
        let device_identifier = "11111111-1111-4111-8111-111111111111".to_owned();
        let middleware =
            std::sync::Arc::new(super::DeviceLoginMiddleware::new(device_identifier.clone()));
        {
            let mut accounts = super::auth_client().lock().unwrap();
            accounts.active = Some(super::AuthSession {
                account_id: uuid::Uuid::new_v4(),
                email: "fingerprint@example.invalid".to_owned(),
                server_mode: 0,
                server_url: String::new(),
                state: super::SESSION_UNLOCKED,
                server,
                device_identifier,
                client,
                device_login_middleware: middleware,
                unlock_data: None,
                vault,
                open_detail: None,
                offline_only: false,
                send_write_uncertain: false,
                #[cfg(windows)]
                pin_enrollment: None,
            });
            super::SESSION_STATE.store(
                super::SESSION_UNLOCKED,
                std::sync::atomic::Ordering::Release,
            );
        }
        let response = unsafe { take_ffi_json(super::bw_vault_fingerprint()) };
        assert_eq!(
            response["ok"], true,
            "unexpected fingerprint error: {response}"
        );
        let phrase = response["data"].as_str().unwrap();
        assert_eq!(phrase.split('-').count(), 5, "unexpected phrase {phrase}");
        assert_eq!(super::accounts::bw_auth_lock(), 1);
        let locked = unsafe { take_ffi_json(super::bw_vault_fingerprint()) };
        assert_eq!(locked["error"], "locked");
    }
}
