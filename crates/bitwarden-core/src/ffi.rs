use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use async_trait::async_trait;
use bitwarden_auth::token_management::PasswordManagerTokenHandler;
use bitwarden_generators::{
    GeneratorClient, GeneratorClientsExt, PassphraseGeneratorRequest, PasswordGeneratorRequest,
};
use bw_sdk_core::{
    auth::login::{
        LoginError, PasswordLoginRequest, PasswordLoginUnlockData, TwoFactorProvider,
        TwoFactorRequest,
    },
    client::{ClientSettings, DeviceType},
    key_management::MasterPasswordAuthenticationData,
    Client, ClientBuilder,
};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

use crate::auth_request::{self, RequestError};
use crate::vault::{VaultCache, VaultEditorData, VaultError, VaultMutationResult};

const AUTH_FAILED: u32 = 0;
const AUTHENTICATED: u32 = 1;
const TWO_FACTOR_REQUIRED: u32 = 2;
const AUTH_TIMED_OUT: u32 = 3;
const AUTH_CANCELLED: u32 = 4;
const DEVICE_VERIFICATION_REQUIRED: u32 = 5;
const DEVICE_VERIFICATION_INVALID: u32 = 6;
const TWO_FACTOR_MASK_SHIFT: u32 = 8;
const TWO_FACTOR_AUTHENTICATOR: u32 = 1 << 0;
const TWO_FACTOR_EMAIL: u32 = 1 << 1;
const TWO_FACTOR_YUBIKEY: u32 = 1 << 3;

const SESSION_SIGNED_OUT: u8 = 0;
const SESSION_UNLOCKED: u8 = 1;
const SESSION_LOCKED: u8 = 2;
static SESSION_STATE: AtomicU8 = AtomicU8::new(SESSION_SIGNED_OUT);
static AUTH_CLIENT: OnceLock<Mutex<Option<AuthSession>>> = OnceLock::new();
static ASYNC_RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
static GENERATOR: OnceLock<GeneratorClient> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
struct AuthServer {
    api_url: String,
    identity_url: String,
}

struct AuthSession {
    server: AuthServer,
    device_identifier: String,
    client: Client,
    device_login_middleware: Arc<DeviceLoginMiddleware>,
    unlock_data: Option<PasswordLoginUnlockData>,
    vault: VaultCache,
}

impl AuthSession {
    fn clear_vault_keys(&mut self) -> bool {
        self.vault.discard_overview();
        catch_unwind(AssertUnwindSafe(|| {
            self.client.internal.get_key_store().clear()
        }))
        .is_ok()
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

    fn rewrite_login_request(&self, request: &mut reqwest::Request, new_device_otp: Option<&str>) {
        let Some(body) = request.body().and_then(reqwest::Body::as_bytes) else {
            return;
        };
        let mut fields: Vec<(String, String)> =
            url::form_urlencoded::parse(body).into_owned().collect();
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in &fields {
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
        form.append_pair("deviceName", "BitwardenWinUI");
        if let Some(otp) = new_device_otp {
            form.append_pair("newDeviceOtp", otp);
        }
        let encoded = form.finish();
        for (key, value) in &mut fields {
            if key == "password" {
                value.zeroize();
            }
        }
        *request.body_mut() = Some(reqwest::Body::from(encoded));
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
        if request.url().path().ends_with("/connect/token")
            && request.method() == reqwest::Method::POST
        {
            let otp = self
                .pending_otp
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            self.rewrite_login_request(&mut request, otp.as_deref().map(String::as_str));
        }
        next.run(request, extensions).await
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

fn auth_client() -> &'static Mutex<Option<AuthSession>> {
    AUTH_CLIENT.get_or_init(|| Mutex::new(None))
}

fn auth_server(mode: u32, base_url: &str) -> Option<AuthServer> {
    match mode {
        0 => Some(AuthServer {
            api_url: "https://api.bitwarden.com".to_owned(),
            identity_url: "https://identity.bitwarden.com".to_owned(),
        }),
        1 => Some(AuthServer {
            api_url: "https://api.bitwarden.eu".to_owned(),
            identity_url: "https://identity.bitwarden.eu".to_owned(),
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
    })
}

fn build_auth_client(server: &AuthServer, middleware: Arc<DeviceLoginMiddleware>) -> Client {
    let settings = ClientSettings {
        api_url: server.api_url.clone(),
        identity_url: server.identity_url.clone(),
        device_type: DeviceType::WindowsDesktop,
        user_agent: format!(
            "Bitwarden-Rust/{} (Windows Desktop)",
            env!("CARGO_PKG_VERSION")
        ),
        bitwarden_client_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        bitwarden_package_type: Some("msix".to_owned()),
        ..ClientSettings::default()
    };

    ClientBuilder::new()
        .with_settings(settings)
        .with_token_handler(std::sync::Arc::new(PasswordManagerTokenHandler::default()))
        .with_identity_middleware(vec![middleware])
        .build()
}

fn auth_client_for_server<'a>(
    session: &'a mut Option<AuthSession>,
    server: &AuthServer,
    device_identifier: &str,
) -> &'a Client {
    if session.as_ref().is_none_or(|current| {
        current.server != *server || current.device_identifier != device_identifier
    }) {
        let device_identifier = if uuid::Uuid::parse_str(device_identifier).is_ok() {
            device_identifier.to_owned()
        } else {
            // Never share one global device identifier across installations.
            uuid::Uuid::new_v4().to_string()
        };
        let middleware = Arc::new(DeviceLoginMiddleware::new(device_identifier.clone()));
        *session = Some(AuthSession {
            server: server.clone(),
            device_identifier,
            client: build_auth_client(server, middleware.clone()),
            device_login_middleware: middleware,
            unlock_data: None,
            vault: VaultCache::default(),
        });
    }

    &session
        .as_ref()
        .expect("auth session was initialized")
        .client
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

fn supported_two_factor_methods(
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
            let methods = supported_two_factor_methods(&providers);
            TWO_FACTOR_REQUIRED | (methods << TWO_FACTOR_MASK_SHIFT)
        }
        Err(LoginError::IdentityFail(error))
            if error.to_string().trim() == "new device verification required" =>
        {
            DEVICE_VERIFICATION_REQUIRED
        }
        Err(LoginError::IdentityFail(error))
            if error.to_string().trim() == "invalid new device otp" =>
        {
            DEVICE_VERIFICATION_INVALID
        }
        Err(_) => AUTH_FAILED,
    }
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

        let email = email.trim().to_owned();
        if email.is_empty() || password.is_empty() {
            return AUTH_FAILED;
        }
        let two_factor = if token.is_empty() {
            None
        } else {
            let Some(provider) = two_factor_provider(provider) else {
                return AUTH_FAILED;
            };
            Some(TwoFactorRequest {
                token: token.to_string(),
                provider,
                remember: false,
            })
        };

        let request = SensitiveLoginRequest(PasswordLoginRequest {
            email,
            password: password.to_string(),
            two_factor,
        });

        let Some(runtime) = runtime() else {
            return AUTH_FAILED;
        };
        let mut session = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if bw_auth_session_state() != SESSION_SIGNED_OUT {
            return AUTH_FAILED;
        }
        if !auth_request::is_current(request_id) {
            return AUTH_CANCELLED;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            auth_client_for_server(&mut session, &server, &device_identifier);
            let active_session = session
                .as_mut()
                .expect("authentication session was initialized");
            active_session.device_login_middleware.arm(&device_otp);
            let response = runtime.block_on(auth_request::run(
                request_id,
                active_session
                    .client
                    .auth()
                    .login_password_with_unlock_data(&request.0),
            ));
            active_session.device_login_middleware.arm("");
            match response {
                Ok(Ok((response, unlock_data))) if auth_request::is_current(request_id) => {
                    if response.authenticated {
                        if unlock_data.is_none() {
                            return AUTH_FAILED;
                        }
                        active_session.unlock_data = unlock_data;
                        SESSION_STATE.store(SESSION_UNLOCKED, Ordering::Release);
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
            *session = None;
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
        let email = email.trim().to_owned();
        if email.is_empty() || password.is_empty() {
            return 0;
        }
        let request = SensitiveEmailRequest(bw_sdk_core::auth::login::TwoFactorEmailRequest {
            email,
            password: password.to_string(),
        });

        let result = catch_unwind(AssertUnwindSafe(|| {
            let runtime = runtime().ok_or(())?;
            let mut session = auth_client()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if bw_auth_session_state() != SESSION_SIGNED_OUT {
                return Ok(AUTH_FAILED as u8);
            }
            let device_identifier = session
                .as_ref()
                .map(|current| current.device_identifier.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let client = auth_client_for_server(&mut session, &server, &device_identifier);
            let response = runtime.block_on(auth_request::run(
                request_id,
                client.auth().send_two_factor_email(&request.0),
            ));
            let status = match response {
                Ok(Ok(())) => AUTHENTICATED as u8,
                Ok(Err(_)) => AUTH_FAILED as u8,
                Err(RequestError::TimedOut) => AUTH_TIMED_OUT as u8,
                Err(RequestError::Cancelled) => AUTH_CANCELLED as u8,
            };
            if status == AUTH_TIMED_OUT as u8 || status == AUTH_CANCELLED as u8 {
                *session = None;
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
        let email = email.trim().to_owned();
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
            let mut session = auth_client()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if bw_auth_session_state() != SESSION_SIGNED_OUT {
                return Ok(AUTH_FAILED as u8);
            }
            auth_client_for_server(&mut session, &server, &device_identifier);
            let active_session = session
                .as_ref()
                .expect("authentication session was initialized");
            let kdf = runtime.block_on(auth_request::run(
                request_id,
                active_session.client.auth().prelogin(email.clone()),
            ));
            let kdf = match kdf {
                Ok(Ok(kdf)) => kdf,
                Err(RequestError::TimedOut) => return Ok(AUTH_TIMED_OUT as u8),
                Err(RequestError::Cancelled) => return Ok(AUTH_CANCELLED as u8),
                Ok(Err(_)) => return Ok(AUTH_FAILED as u8),
            };
            let auth_data = MasterPasswordAuthenticationData::derive(&password, &kdf, &email)
                .map_err(|_| ())?;
            let password_hash =
                Zeroizing::new(auth_data.master_password_authentication_hash.to_string());
            drop(auth_data);
            let config = active_session.client.internal.get_api_configurations();
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
                Ok(Ok(_)) | Ok(Err(_)) => Ok(AUTH_FAILED as u8),
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
pub extern "C" fn bw_auth_logout() {
    // Wake an in-flight network operation before waiting for its session guard.
    auth_request::begin();
    let mut session = auth_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(mut active) = session.take() {
        active.clear_vault_keys();
    }
    SESSION_STATE.store(SESSION_SIGNED_OUT, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn bw_auth_lock() -> u8 {
    catch_unwind(AssertUnwindSafe(|| {
        auth_request::begin();
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if bw_auth_session_state() == SESSION_LOCKED {
            return 1;
        }
        let Some(session) = guard
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return 0;
        };
        if !session.clear_vault_keys() {
            *guard = None;
            SESSION_STATE.store(SESSION_SIGNED_OUT, Ordering::Release);
            return 0;
        }
        SESSION_STATE.store(SESSION_LOCKED, Ordering::Release);
        1
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn bw_auth_unlock(password: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(password) = (unsafe { read_utf8(password) }) else {
            return vault_error("wrongPassword");
        };
        let password = Zeroizing::new(password);
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_LOCKED)
        else {
            return vault_error("unlockUnavailable");
        };
        let Some(keys) = session.unlock_data.as_ref() else {
            return vault_error("unlockUnavailable");
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(auth_request::run(request_id, async {
                keys.unlock(&session.client, &password)
                    .map_err(|_| VaultError::WrongPassword)?;
                session.vault.restore_overview(&session.client).await
            }))
        }));
        let code = match result {
            Ok(Ok(Ok(()))) if auth_request::is_current(request_id) => {
                SESSION_STATE.store(SESSION_UNLOCKED, Ordering::Release);
                return vault_success(&true);
            }
            Ok(Ok(Ok(()))) | Ok(Err(RequestError::Cancelled)) => "cancelled".to_owned(),
            Ok(Err(RequestError::TimedOut)) => "timedOut".to_owned(),
            Ok(Ok(Err(error))) => error.to_string(),
            Err(_) => "internal".to_owned(),
        };
        if !session.clear_vault_keys() {
            *guard = None;
            SESSION_STATE.store(SESSION_SIGNED_OUT, Ordering::Release);
            return vault_error("internal");
        }
        vault_error(&code)
    }))
    .unwrap_or_else(|_| vault_error("internal"))
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
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        match runtime.block_on(auth_request::run(
            request_id,
            VaultCache::fetch(&session.client, &mut session.unlock_data),
        )) {
            Ok(Ok(cache)) if auth_request::is_current(request_id) => {
                cache.activate(&session.client);
                session.vault = cache;
                vault_success(
                    session
                        .vault
                        .overview()
                        .expect("successful sync has an overview"),
                )
            }
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(error)) => vault_error(&error.to_string()),
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
        let guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .as_ref()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        match runtime.block_on(auth_request::run(
            request_id,
            session.vault.detail(&session.client, &id, &password),
        )) {
            Ok(Ok(detail)) if auth_request::is_current(request_id) => vault_success(&detail),
            Ok(Ok(_)) | Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
            Ok(Err(error)) => vault_error(&error.to_string()),
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
    Action {
        id: String,
        action: String,
        revision: String,
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
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        session.vault.reset_write_attempt();
        // Catch inside the session guard: a panic after sending a write must force reconciliation.
        let result = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(auth_request::run(request_id, async {
                match command {
                    VaultWriteCommand::Create(data) => {
                        session.vault.create(&session.client, data).await
                    }
                    VaultWriteCommand::Update(data, password) => {
                        session.vault.update(&session.client, data, &password).await
                    }
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
                }
            }))
        }));
        match result {
            // An acknowledged success is authoritative even if cancellation races its return.
            Ok(Ok(Ok(result))) => vault_write_success(&result),
            Ok(Ok(Err(error))) => vault_error(&error.to_string()),
            _ if session.vault.write_in_flight() => {
                session.vault.mark_write_uncertain();
                vault_error("writeUncertain")
            }
            Ok(Err(RequestError::Cancelled)) => vault_error("cancelled"),
            Ok(Err(RequestError::TimedOut)) => vault_error("timedOut"),
            Err(_) => vault_error("internal"),
        }
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
        super::SESSION_STATE.store(
            super::SESSION_UNLOCKED,
            std::sync::atomic::Ordering::Release,
        );
        super::bw_auth_logout();
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
}
