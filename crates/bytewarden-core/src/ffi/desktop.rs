//! Safe in-process adapter for the native Rust desktop frontend.
//!
//! All session transitions, authorization and protocol validation still pass
//! through the same implementation as the WinUI DLL. Blocking calls must be
//! dispatched off the UI thread. Do not log passwords or detail payloads.

use std::{
    ffi::{c_char, CStr},
    fmt,
    path::Path,
    sync::OnceLock,
    time::Duration,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

pub use super::desktop_transfer::{
    ExportFormat, ExportReport, SendDetail, SendDraft, SendSummary, Sends,
};
use super::{accounts, *};
pub use crate::vault::{
    VaultFolder as Folder, VaultItem as Item, VaultMutationResult as Mutation,
    VaultOverview as Overview,
};

#[derive(Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub(super) fn from_code(code: &str) -> Self {
        Self(code.into())
    }

    pub fn code(&self) -> &str {
        &self.0
    }
    fn internal() -> Self {
        Self("internal".into())
    }
    pub(super) fn validation() -> Self {
        Self("validation".into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    SignedOut,
    Unlocked,
    Locked,
}

impl Session {
    fn from_raw(raw: u8) -> Self {
        match raw {
            SESSION_UNLOCKED => Self::Unlocked,
            SESSION_LOCKED => Self::Locked,
            _ => Self::SignedOut,
        }
    }
}

pub fn session() -> Session {
    Session::from_raw(bw_auth_session_state())
}

#[cfg(windows)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyChoice {
    pub id: String,
    pub revision: String,
    pub credential_index: usize,
    pub name: String,
    pub user_name: Option<String>,
    pub reprompt: bool,
}

#[cfg(windows)]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PasskeyRequest {
    pub id: String,
    pub creating: bool,
    pub relying_party: Option<String>,
    pub user_name: Option<String>,
    pub session_state: u8,
    pub choices: Vec<PasskeyChoice>,
    pub needs_sync: bool,
    pub blocked_reason: Option<String>,
}

#[cfg(windows)]
impl PasskeyRequest {
    pub fn session(&self) -> Session {
        Session::from_raw(self.session_state)
    }
}

#[cfg(windows)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PasskeyApproval<'a> {
    id: &'a str,
    item_id: &'a str,
    revision: &'a str,
    credential_index: usize,
}

#[cfg(windows)]
pub fn start_passkey_provider() -> Result<(), Error> {
    read(super::passkeys::bw_passkey_start()).map(|_: bool| ())
}

#[cfg(windows)]
pub fn register_passkey_provider() -> Result<(), Error> {
    read(super::passkeys::bw_passkey_register()).map(|_: bool| ())
}

#[cfg(windows)]
pub fn passkey_provider_state() -> Result<String, Error> {
    read(super::passkeys::bw_passkey_state())
}

#[cfg(windows)]
pub fn has_pending_passkey() -> bool {
    super::passkeys::bw_passkey_has_pending() != 0
}

#[cfg(windows)]
pub fn pending_passkey() -> Result<PasskeyRequest, Error> {
    read(super::passkeys::bw_passkey_pending())
}

#[cfg(windows)]
pub fn sync_passkey_metadata() -> Result<bool, Error> {
    read(super::passkeys::bw_passkey_sync_metadata())
}

#[cfg(windows)]
pub fn passkey_is_active(id: &str) -> Result<bool, Error> {
    let id = argument(id)?;
    Ok(unsafe { super::passkeys::bw_passkey_active(ptr(&id)) } != 0)
}

#[cfg(windows)]
pub fn cancel_passkey(id: &str) -> Result<(), Error> {
    let id = argument(id)?;
    unsafe { super::passkeys::bw_passkey_cancel(ptr(&id)) };
    Ok(())
}

#[cfg(windows)]
pub fn dismiss_passkey_refusal(id: &str) -> Result<(), Error> {
    let id = argument(id)?;
    unsafe { super::passkeys::bw_passkey_dismiss_refusal(ptr(&id)) };
    Ok(())
}

#[cfg(windows)]
pub fn respond_to_passkey(
    request_id: &str,
    choice: Option<&PasskeyChoice>,
    password: &str,
    window: usize,
) -> Result<(), Error> {
    let (item_id, revision, credential_index) = choice.map_or(("", "", 0), |choice| {
        (
            choice.id.as_str(),
            choice.revision.as_str(),
            choice.credential_index,
        )
    });
    let json = serial(&PasskeyApproval {
        id: request_id,
        item_id,
        revision,
        credential_index,
    })?;
    let password = argument(password)?;
    read(unsafe { super::passkeys::bw_passkey_respond(ptr(&json), ptr(&password), window) })
}

#[derive(Clone, Copy)]
pub enum Server<'a> {
    Us,
    Eu,
    SelfHosted(&'a str),
}

impl<'a> Server<'a> {
    fn parts(self) -> (u32, &'a str) {
        match self {
            Self::Us => (0, ""),
            Self::Eu => (1, ""),
            Self::SelfHosted(value) => (2, value),
        }
    }

    pub fn is_valid(self) -> bool {
        let (mode, value) = self.parts();
        auth_server(mode, value).is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Authenticator,
    Email,
    YubiKey,
}

impl Provider {
    fn raw(self) -> u32 {
        match self {
            Self::Authenticator => 0,
            Self::Email => 1,
            Self::YubiKey => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Providers(u32);
impl Providers {
    pub fn supports(self, provider: Provider) -> bool {
        self.0 & (1 << provider.raw()) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    Authenticated,
    TwoFactor(Providers),
    DeviceVerification,
    InvalidDeviceCode,
    InvalidTwoFactor,
    AlreadySignedIn,
    AccountLimit,
    TimedOut,
    Cancelled,
    RateLimited,
    NetworkError,
    ServerError,
    AuthenticationRejected,
    Failed,
}

impl AuthStatus {
    fn from_raw(raw: u32) -> Self {
        match raw & 0xff {
            AUTHENTICATED => Self::Authenticated,
            TWO_FACTOR_REQUIRED => Self::TwoFactor(Providers(raw >> TWO_FACTOR_MASK_SHIFT)),
            DEVICE_VERIFICATION_REQUIRED => Self::DeviceVerification,
            DEVICE_VERIFICATION_INVALID => Self::InvalidDeviceCode,
            INVALID_TWO_FACTOR_CODE => Self::InvalidTwoFactor,
            ACCOUNT_ALREADY_SIGNED_IN => Self::AlreadySignedIn,
            ACCOUNT_LIMIT_REACHED => Self::AccountLimit,
            AUTH_TIMED_OUT => Self::TimedOut,
            AUTH_CANCELLED => Self::Cancelled,
            AUTH_RATE_LIMITED => Self::RateLimited,
            AUTH_NETWORK_ERROR => Self::NetworkError,
            AUTH_SERVER_ERROR => Self::ServerError,
            AUTH_REJECTED => Self::AuthenticationRejected,
            _ => Self::Failed,
        }
    }
}

pub struct Login<'a> {
    pub email: &'a str,
    pub password: &'a str,
    pub server: Server<'a>,
    pub device: uuid::Uuid,
    pub two_factor: Option<(Provider, &'a str)>,
    pub device_code: &'a str,
    pub remember_2fa: bool,
}

/// A generation token reserved on the GUI thread before scheduling a worker.
/// Cancellation is idempotent and cannot cancel a newer request.
#[derive(Clone, Copy)]
pub struct Request(u64);

impl Request {
    pub(super) fn raw(self) -> u64 {
        self.0
    }

    pub fn begin() -> Self {
        Self(bw_auth_begin_request())
    }
    pub fn cancel(self) {
        bw_auth_cancel_request(self.0);
    }
    pub fn is_current(self) -> bool {
        auth_request::is_current(self.0)
    }

    pub fn login(self, input: Login<'_>) -> Result<AuthStatus, Error> {
        let email = argument(input.email)?;
        let password = argument(input.password)?;
        let (mode, server) = input.server.parts();
        let server = argument(server)?;
        let device = argument(&input.device.to_string())?;
        let code = argument(input.device_code)?;
        let (provider, token) = input.two_factor.unwrap_or((Provider::Authenticator, ""));
        let token = argument(token)?;
        // The buffers are UTF-8, terminated, and live until the shared operation returns.
        let result = unsafe {
            bw_auth_login_with_remember(
                ptr(&email),
                ptr(&password),
                provider.raw(),
                ptr(&token),
                mode,
                ptr(&server),
                ptr(&device),
                ptr(&code),
                u8::from(input.remember_2fa),
                self.0,
            )
        };
        Ok(AuthStatus::from_raw(result))
    }

    pub fn send_email_code(
        self,
        email: &str,
        password: &str,
        server: Server<'_>,
    ) -> Result<AuthStatus, Error> {
        let email = argument(email)?;
        let password = argument(password)?;
        let (mode, server) = server.parts();
        let server = argument(server)?;
        Ok(AuthStatus::from_raw(u32::from(unsafe {
            bw_auth_send_two_factor_email(ptr(&email), ptr(&password), mode, ptr(&server), self.0)
        })))
    }

    pub fn resend_device_code(
        self,
        email: &str,
        password: &str,
        device: uuid::Uuid,
        server: Server<'_>,
    ) -> Result<AuthStatus, Error> {
        let email = argument(email)?;
        let password = argument(password)?;
        let device = argument(&device.to_string())?;
        let (mode, server) = server.parts();
        let server = argument(server)?;
        Ok(AuthStatus::from_raw(u32::from(unsafe {
            bw_auth_resend_new_device_verification(
                ptr(&email),
                ptr(&password),
                ptr(&device),
                mode,
                ptr(&server),
                self.0,
            )
        })))
    }

    pub fn unlock(self, password: &str) -> Result<(), Error> {
        let password = argument(password)?;
        read::<bool>(unsafe { bw_auth_unlock(ptr(&password), self.0) }).map(|_| ())
    }

    pub fn enable_hello(self, account: &str, password: &str, hwnd: usize) -> Result<(), Error> {
        #[cfg(windows)]
        {
            let account = argument(account)?;
            let password = argument(password)?;
            let window =
                hello_window::window_id(hwnd).map_err(|_| Error::from_code("helloUnavailable"))?;
            read::<bool>(unsafe {
                hello::bw_hello_enable_for_window(ptr(&account), ptr(&password), window, self.0)
            })
            .map(|_| ())
        }
        #[cfg(not(windows))]
        {
            let _ = (self, account, password, hwnd);
            Err(Error::from_code("helloUnavailable"))
        }
    }

    pub fn unlock_hello(self, account: &str, hwnd: usize) -> Result<(), Error> {
        #[cfg(windows)]
        {
            let account = argument(account)?;
            let window =
                hello_window::window_id(hwnd).map_err(|_| Error::from_code("helloUnavailable"))?;
            read::<bool>(unsafe {
                hello::bw_hello_unlock_for_window(ptr(&account), window, self.0)
            })
            .map(|_| ())
        }
        #[cfg(not(windows))]
        {
            let _ = (self, account, hwnd);
            Err(Error::from_code("helloUnavailable"))
        }
    }

    pub fn sync(self) -> Result<Overview, Error> {
        vault_sync(self.0, Overview::clone).map_err(Error)
    }

    pub fn create_folder(self, name: &str) -> Result<Mutation, Error> {
        self.write(VaultWriteCommand::CreateFolder(
            checked_argument(name)?.into(),
        ))
    }

    pub fn rename_folder(self, id: &str, name: &str) -> Result<Mutation, Error> {
        self.write(VaultWriteCommand::RenameFolder {
            id: checked_argument(id)?.into(),
            name: checked_argument(name)?.into(),
        })
    }

    pub fn delete_folders(self, ids: &[String]) -> Result<Mutation, Error> {
        self.write(VaultWriteCommand::DeleteFolders(parse_ids(ids)?))
    }

    pub fn move_items(self, ids: &[String], folder_id: Option<&str>) -> Result<Mutation, Error> {
        let folder_id = folder_id.map(parse_id).transpose()?;
        self.write(VaultWriteCommand::MoveItems(VaultItemMove {
            ids: parse_ids(ids)?,
            folder_id,
        }))
    }

    pub fn bulk_action(
        self,
        ids: &[String],
        action: ItemAction,
        password: &str,
    ) -> Result<Mutation, Error> {
        self.write(VaultWriteCommand::BulkAction {
            ids: parse_ids(ids)?,
            action: action.code().into(),
            password: Zeroizing::new(checked_argument(password)?.into()),
        })
    }

    pub fn editor(self, id: &str, password: &str) -> Result<Draft, Error> {
        let id = argument(id)?;
        let password = argument(password)?;
        read(unsafe { bw_vault_editor(ptr(&id), ptr(&password), self.0) })
    }

    pub fn save_item(self, draft: &Draft, password: &str) -> Result<Mutation, Error> {
        let data = serial(draft)?;
        let password = checked_argument(password)?;
        let data = parse_editor_json(&data[..data.len() - 1]).ok_or_else(Error::validation)?;
        if draft.id.is_some() {
            self.write(VaultWriteCommand::Update(
                data,
                Zeroizing::new(password.into()),
            ))
        } else {
            self.write(VaultWriteCommand::Create(data))
        }
    }

    pub fn detail(self, id: &str, password: &str) -> Result<Detail, Error> {
        let id = argument(id)?;
        let password = argument(password)?;
        read(unsafe { bw_vault_detail(ptr(&id), ptr(&password), self.0) })
    }

    pub fn action(
        self,
        id: &str,
        action: ItemAction,
        revision: &str,
        password: &str,
    ) -> Result<Overview, Error> {
        self.write(VaultWriteCommand::Action {
            id: checked_argument(id)?.into(),
            action: action.code().into(),
            revision: checked_argument(revision)?.into(),
            password: Zeroizing::new(checked_argument(password)?.into()),
        })
        .map(|result| result.overview)
    }

    pub(super) fn write(self, command: VaultWriteCommand) -> Result<Mutation, Error> {
        write_vault(command, self.0).map_err(Error)
    }
}

fn parse_ids<T: std::str::FromStr>(ids: &[String]) -> Result<Vec<T>, Error> {
    ids.iter().map(|id| parse_id(id)).collect()
}

fn parse_id<T: std::str::FromStr>(id: &str) -> Result<T, Error> {
    id.parse().map_err(|_| Error::validation())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ItemAction {
    Favorite,
    Unfavorite,
    Archive,
    Unarchive,
    Trash,
    Restore,
    Delete,
}

impl ItemAction {
    fn code(self) -> &'static str {
        match self {
            Self::Favorite => "favorite",
            Self::Unfavorite => "unfavorite",
            Self::Archive => "archive",
            Self::Unarchive => "unarchive",
            Self::Trash => "trash",
            Self::Restore => "restore",
            Self::Delete => "delete",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub email: String,
    pub server_mode: u32,
    pub server_url: String,
    state: u8,
    pub active: bool,
    pub needs_sign_in: bool,
    #[serde(default)]
    pub hello_enabled: bool,
    #[serde(default)]
    pub pin_enabled: bool,
}

impl Account {
    /// Stable, non-sensitive preference key matching WinUI account settings scope.
    /// Runtime account IDs can change after logout and must not key preferences.
    pub fn settings_scope(&self) -> Result<String, Error> {
        use sha2::{Digest, Sha256};
        let server = match self.server_mode {
            0 => "https://vault.bitwarden.com".to_owned(),
            1 => "https://vault.bitwarden.eu".to_owned(),
            2 => {
                if auth_server(2, &self.server_url).is_none() {
                    return Err(Error::validation());
                }
                url::Url::parse(self.server_url.trim())
                    .map_err(|_| Error::validation())?
                    .as_str()
                    .trim_end_matches('/')
                    .to_owned()
            }
            _ => return Err(Error::validation()),
        };
        let identity = format!(
            "{}\n{}\n{}",
            self.server_mode,
            server,
            self.email.trim().to_lowercase()
        );
        let digest = Sha256::digest(identity.as_bytes());
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut scope = String::with_capacity(digest.len() * 2);
        for byte in digest {
            scope.push(char::from(HEX[usize::from(byte >> 4)]));
            scope.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Ok(scope)
    }

    pub fn session(&self) -> Session {
        Session::from_raw(self.state)
    }
}

// Keep the entire editable projection so an edit never drops unrelated fields,
// URI matching modes, or linked-field associations. Passkey private material
// is not part of this projection; the core retains it unless told to remove it.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub id: Option<String>,
    pub name: String,
    pub item_type: u8,
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub reprompt: bool,
    pub revision_date: Option<String>,
    pub notes: Option<String>,
    pub values: std::collections::HashMap<String, String>,
    pub uris: Vec<DraftUri>,
    pub custom_fields: Vec<DraftField>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_restore: bool,
    pub is_deleted: bool,
    pub is_archived: bool,
    pub can_archive: bool,
    pub edit_blocked_reason: Option<String>,
    pub passkeys: Option<Vec<Passkey>>,
    pub remove_saved_passkey: bool,
}

impl Drop for Draft {
    fn drop(&mut self) {
        if let Some(notes) = &mut self.notes {
            notes.zeroize();
        }
        for value in self.values.values_mut() {
            value.zeroize();
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftUri {
    pub uri: Option<String>,
    pub r#match: Option<u8>,
}
impl Drop for DraftUri {
    fn drop(&mut self) {
        self.uri.zeroize();
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftField {
    pub name: Option<String>,
    pub value: Option<String>,
    pub r#type: u8,
    pub linked_id: Option<u32>,
}
impl Drop for DraftField {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Passkey {
    pub relying_party: String,
    pub user_name: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub name: String,
    pub fields: Vec<Field>,
    pub item_type: u8,
    pub revision_date: String,
    pub creation_date: String,
    pub restricted: bool,
    pub password_hidden: bool,
    pub totp_request_id: Option<u64>,
    pub attachment_request_id: Option<u64>,
    pub totp: Option<Totp>,
    pub passkeys: Vec<Passkey>,
    pub attachment_files: Vec<Attachment>,
    pub can_upload_attachments: bool,
    pub can_delete_attachments: bool,
    pub attachment_failures: usize,
    pub password_revision_date: Option<String>,
    pub password_strength: Option<u8>,
    #[serde(default)]
    pub password_history: Vec<PasswordHistory>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub label: String,
    pub value: String,
    pub sensitive: bool,
    pub launch_uri: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum Totp {
    Ready { value: TotpCode },
    PremiumRequired,
    Invalid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpCode {
    pub code: String,
    pub period: u32,
    pub valid_from: i64,
    pub valid_until: i64,
}
impl Drop for TotpCode {
    fn drop(&mut self) {
        self.code.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
    pub can_download: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordHistory {
    pub password: String,
    pub last_used_date: String,
}
impl Drop for PasswordHistory {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl Detail {
    pub fn attachment_lease(&self) -> Option<AttachmentLease> {
        self.attachment_request_id
            .map(|n| AttachmentLease(Request(n)))
    }
    pub fn totp_lease(&self) -> Option<TotpLease> {
        self.totp_request_id.map(TotpLease)
    }

    pub fn revoke(&self) {
        if let Some(token) = self.totp_request_id.or(self.attachment_request_id) {
            bw_auth_cancel_request(token);
        }
    }
}

#[derive(Clone, Copy)]
pub struct TotpLease(u64);
impl TotpLease {
    pub fn refresh(self) -> Result<TotpCode, Error> {
        read(bw_vault_totp_refresh(self.0))
    }
}

#[derive(Clone, Copy)]
pub struct AttachmentLease(Request);
impl AttachmentLease {
    pub fn request(self) -> Request {
        self.0
    }
    pub fn download(self, id: &str, destination: &Path) -> Result<(), Error> {
        let id = argument(id)?;
        let path = argument(destination.to_str().ok_or_else(Error::validation)?)?;
        read::<bool>(unsafe { bw_vault_download_attachment(ptr(&id), ptr(&path), self.0 .0) })
            .map(|_| ())
    }
    pub fn upload(self, source: &Path) -> Result<Mutation, Error> {
        let path = checked_argument(source.to_str().ok_or_else(Error::validation)?)?;
        self.0
            .write(VaultWriteCommand::UploadAttachment(path.into()))
    }
    pub fn delete(self, id: &str) -> Result<Mutation, Error> {
        self.0.write(VaultWriteCommand::DeleteAttachment(
            checked_argument(id)?.into(),
        ))
    }
}

impl Drop for Field {
    fn drop(&mut self) {
        self.value.zeroize();
        if let Some(uri) = &mut self.launch_uri {
            uri.zeroize();
        }
    }
}

/// Fetch a bounded, unauthenticated website icon from the configured service.
/// Only a DNS hostname is included in the path; requests never follow redirects
/// and do not carry account credentials, cookies, or vault data.
pub fn fetch_website_icon(
    host: &str,
    server_mode: u32,
    server_url: &str,
) -> Result<Option<(String, Vec<u8>)>, Error> {
    let url = website_icon_url(host, server_mode, server_url).ok_or_else(Error::validation)?;
    static CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(6))
                .build()
                .map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| Error::internal())?;
    let runtime = super::runtime().ok_or_else(Error::internal)?;
    let response = runtime.block_on(fetch_icon_response(client, url));
    Ok(response)
}

async fn fetch_icon_response(client: &reqwest::Client, url: url::Url) -> Option<(String, Vec<u8>)> {
    const MAX_BYTES: usize = 128 * 1024;
    let mut response = client.get(url).send().await.ok()?;
    if !response.status().is_success()
        || response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > MAX_BYTES)
    {
        return None;
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        mime.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/gif"
            | "image/bmp"
            | "image/tiff"
            | "image/tif"
    ) {
        return None;
    }
    let mut bytes = Vec::with_capacity(8 * 1024);
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().checked_add(chunk.len())? > MAX_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    (!bytes.is_empty()).then_some((mime, bytes))
}

fn website_icon_url(host: &str, server_mode: u32, server_url: &str) -> Option<url::Url> {
    let host = normalized_icon_host(host)?;
    let mut endpoint = if server_mode == 2 {
        super::auth_server(server_mode, server_url)?;
        let mut base = url::Url::parse(server_url.trim()).ok()?;
        base.path_segments_mut().ok()?.pop_if_empty().push("icons");
        base
    } else if server_mode <= 1 {
        url::Url::parse("https://icons.bitwarden.net/").ok()?
    } else {
        return None;
    };
    endpoint
        .path_segments_mut()
        .ok()?
        .pop_if_empty()
        .push(&host)
        .push("icon.png");
    Some(endpoint)
}

fn normalized_icon_host(value: &str) -> Option<String> {
    let value = value.trim().strip_suffix('.').unwrap_or(value.trim());
    if value.is_empty() || value.len() > 253 || !value.is_ascii() {
        return None;
    }
    let host = value.to_ascii_lowercase();
    if !host.contains('.')
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.rsplit_once('.').is_some_and(|(_, suffix)| {
            matches!(
                suffix,
                "onion"
                    | "i2p"
                    | "local"
                    | "localhost"
                    | "internal"
                    | "lan"
                    | "test"
                    | "invalid"
                    | "example"
            )
        })
    {
        return None;
    }
    if host.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return None;
    }
    Some(host)
}

pub fn configure_cache(path: &Path) -> Result<(), Error> {
    #[cfg(windows)]
    {
        let path = argument(path.to_str().ok_or_else(Error::validation)?)?;
        if unsafe { crate::local_cache::bw_local_cache_set_root(ptr(&path)) } == 0 {
            return Err(Error("localStorage".into()));
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

/// Account-specific timeout transition. Never changes the selected account.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTimeout {
    pub id: String,
    pub log_out: bool,
}

/// Apply account transitions on a worker thread; the core owns all state changes.
pub fn apply_timeouts(actions: &[AccountTimeout], interrupt: bool) -> Result<Vec<Account>, Error> {
    let payload = serde_json::to_string(actions).map_err(|_| Error::validation())?;
    let payload = argument(&payload)?;
    read(unsafe { accounts::bw_auth_apply_timeouts(ptr(&payload), u8::from(interrupt)) })
}

pub fn accounts() -> Result<Vec<Account>, Error> {
    read(accounts::bw_auth_accounts())
}
pub fn disable_hello(account: &str) -> Result<(), Error> {
    #[cfg(windows)]
    {
        let account = argument(account)?;
        read::<bool>(unsafe { hello::bw_hello_disable(ptr(&account)) }).map(|_| ())
    }
    #[cfg(not(windows))]
    {
        let _ = account;
        Err(Error::from_code("helloUnavailable"))
    }
}

pub fn overview() -> Result<Overview, Error> {
    vault_overview(Overview::clone).map_err(Error)
}
pub fn switch_account(id: &str) -> Result<Vec<Account>, Error> {
    let id = argument(id)?;
    read(unsafe { accounts::bw_auth_switch_account(ptr(&id)) })
}
pub fn lock() -> Result<(), Error> {
    if accounts::bw_auth_lock() == 0 {
        return Err(Error("lockUnavailable".into()));
    }
    Ok(())
}
pub fn logout() -> Result<(), Error> {
    if accounts::bw_auth_logout() == 0 {
        return Err(Error("cacheRevocationFailed".into()));
    }
    Ok(())
}
pub fn cancel_login() {
    accounts::bw_auth_cancel_login();
}
pub fn shutdown() {
    accounts::bw_auth_shutdown();
}

pub fn password(
    length: u32,
    options: u32,
    min_numbers: u32,
    min_special: u32,
) -> Result<Zeroizing<String>, Error> {
    generate_password(length, options, min_numbers, min_special)
        .map(Zeroizing::new)
        .ok_or_else(Error::validation)
}
pub fn passphrase(
    words: u32,
    separator: &str,
    capitalize: bool,
    include_number: bool,
) -> Result<Zeroizing<String>, Error> {
    generate_passphrase(words, separator, capitalize, include_number)
        .map(Zeroizing::new)
        .ok_or_else(Error::validation)
}

/// Local username types supported without handing an API key to the SDK's
/// non-zeroizing forwarding-provider request types. No network requests are made.
pub enum Username<'a> {
    Word {
        capitalize: bool,
        include_number: bool,
    },
    Subaddress {
        email: &'a str,
        website: Option<&'a str>,
    },
    Catchall {
        domain: &'a str,
        website: Option<&'a str>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum UsernamePayload<'a> {
    Word {
        capitalize: bool,
        include_number: bool,
    },
    Subaddress {
        r#type: UsernameAppend<'a>,
        email: &'a str,
    },
    Catchall {
        r#type: UsernameAppend<'a>,
        domain: &'a str,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum UsernameAppend<'a> {
    Random,
    WebsiteName { website: &'a str },
}

fn username_append(website: Option<&str>) -> Result<UsernameAppend<'_>, Error> {
    match website {
        None => Ok(UsernameAppend::Random),
        Some(website)
            if !website.is_empty()
                && website.len() <= 64
                && !website.starts_with('.')
                && !website.ends_with('.')
                && !website.contains("..")
                && website
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) =>
        {
            Ok(UsernameAppend::WebsiteName { website })
        }
        Some(_) => Err(Error::validation()),
    }
}

fn username_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

fn username_payload(options: Username<'_>) -> Result<Zeroizing<Vec<u8>>, Error> {
    let payload = match options {
        Username::Word {
            capitalize,
            include_number,
        } => UsernamePayload::Word {
            capitalize,
            include_number,
        },
        Username::Subaddress { email, website } => {
            let (local, domain) = email.split_once('@').ok_or_else(Error::validation)?;
            let appended_length = website.map_or(8, str::len);
            if email.len() + 1 + appended_length > 254
                || local.is_empty()
                || local.len() + 1 + appended_length > 64
                || local.starts_with('.')
                || local.ends_with('.')
                || local.contains("..")
                || !local
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&c))
                || !username_domain(domain)
            {
                return Err(Error::validation());
            }
            UsernamePayload::Subaddress {
                r#type: username_append(website)?,
                email,
            }
        }
        Username::Catchall { domain, website } => {
            let local_length = website.map_or(8, str::len);
            if !username_domain(domain)
                || local_length > 64
                || local_length + 1 + domain.len() > 254
            {
                return Err(Error::validation());
            }
            UsernamePayload::Catchall {
                r#type: username_append(website)?,
                domain,
            }
        }
    };
    serial(&payload)
}

impl Request {
    /// Uses the shared cancellable FFI operation; reserve this token on the UI
    /// thread and run the call on a worker. This typed subset cannot use a provider.
    pub fn username(self, options: Username<'_>) -> Result<Zeroizing<String>, Error> {
        if !self.is_current() {
            return Err(Error("cancelled".into()));
        }
        let json = username_payload(options)?;
        let value = read::<String>(unsafe { bw_generate_username(ptr(&json), self.0) })
            .map(Zeroizing::new)?;
        if !self.is_current() {
            return Err(Error("cancelled".into()));
        }
        Ok(value)
    }
}

/// Local strength estimation only (the existing FFI disables exposed-password
/// lookup). Run off the UI thread; argument copies are wiped on all exits.
pub fn password_strength(value: &str) -> Result<Option<u8>, Error> {
    let password = argument(value)?;
    let username = argument("")?;
    let score = unsafe { bw_password_strength(ptr(&password), ptr(&username)) };
    Ok(u8::try_from(score).ok().filter(|score| *score <= 4))
}

pub(super) fn checked_argument(value: &str) -> Result<&str, Error> {
    if value.as_bytes().contains(&0) {
        return Err(Error::validation());
    }
    Ok(value)
}

pub(super) fn argument(value: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    let value = checked_argument(value)?;
    let mut buffer = Zeroizing::new(Vec::with_capacity(value.len() + 1));
    buffer.extend_from_slice(value.as_bytes());
    buffer.push(0);
    Ok(buffer)
}

pub(super) fn serial(value: &(impl Serialize + ?Sized)) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, value).map_err(|_| Error::validation())?;
    bytes.push(0);
    Ok(bytes)
}

pub(super) fn ptr(buffer: &[u8]) -> *const c_char {
    buffer.as_ptr().cast()
}

// The constructor is private. Only take ownership of results allocated by
// the functions in this module's sibling FFI implementation.
struct NativeResponse(*mut c_char);
impl Drop for NativeResponse {
    fn drop(&mut self) {
        unsafe { bw_string_free(self.0) };
    }
}

pub(super) fn read<T: DeserializeOwned>(raw: *mut c_char) -> Result<T, Error> {
    #[derive(Deserialize)]
    struct Response<T> {
        ok: bool,
        data: Option<T>,
        error: Option<String>,
    }
    if raw.is_null() {
        return Err(Error::internal());
    }
    let data = NativeResponse(raw);
    // Parse the original UTF-8 in place; the native destructor wipes it even on error.
    let response: Response<T> =
        serde_json::from_slice(unsafe { CStr::from_ptr(data.0) }.to_bytes())
            .map_err(|_| Error::internal())?;
    if response.ok {
        response.data.ok_or_else(Error::internal)
    } else {
        Err(response.error.map(Error).unwrap_or_else(Error::internal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve_icon_once(
        status: &'static str,
        headers: &'static str,
        body: Vec<u8>,
        declared_length: Option<usize>,
    ) -> std::io::Result<(
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<std::io::Result<()>>,
    )> {
        use std::io::{BufRead, BufReader, Write};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let endpoint = format!("http://{address}/vault");
        let (request_sender, request_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept()?;
            let mut stream = BufReader::new(stream);
            let mut request_line = String::new();
            stream.read_line(&mut request_line)?;
            request_sender
                .send(request_line)
                .map_err(|_| std::io::Error::other("request receiver dropped"))?;
            let response = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                declared_length.unwrap_or(body.len())
            );
            stream.get_mut().write_all(response.as_bytes())?;
            stream.get_mut().write_all(&body)
        });
        Ok((endpoint, request_receiver, worker))
    }

    fn local_icon_client() -> Result<reqwest::Client, reqwest::Error> {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
    }

    #[test]
    fn website_icon_fetch_uses_only_the_host_and_preserves_custom_server_path(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = b"png!".to_vec();
        let (server, request, worker) =
            serve_icon_once("200 OK", "Content-Type: image/png\r\n", body.clone(), None)?;
        let url = website_icon_url(" EXAMPLE.com. ", 2, &server)
            .ok_or_else(|| std::io::Error::other("valid local server required"))?;
        let runtime =
            super::runtime().ok_or_else(|| std::io::Error::other("core runtime unavailable"))?;
        let result = runtime.block_on(fetch_icon_response(&local_icon_client()?, url));
        assert_eq!(result, Some(("image/png".into(), body)));
        assert_eq!(
            request.recv_timeout(Duration::from_secs(2))?.trim_end(),
            "GET /vault/icons/example.com/icon.png HTTP/1.1"
        );
        worker
            .join()
            .map_err(|_| std::io::Error::other("icon fixture server panicked"))??;
        Ok(())
    }

    #[test]
    fn website_icon_fetch_rejects_redirects_and_oversized_responses(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (server, request, worker) = serve_icon_once(
            "302 Found",
            "Content-Type: image/png\r\nLocation: /redirected\r\n",
            Vec::new(),
            None,
        )?;
        let url = website_icon_url("example.com", 2, &server)
            .ok_or_else(|| std::io::Error::other("valid local server required"))?;
        let runtime =
            super::runtime().ok_or_else(|| std::io::Error::other("core runtime unavailable"))?;
        assert!(runtime
            .block_on(fetch_icon_response(&local_icon_client()?, url))
            .is_none());
        assert_eq!(
            request.recv_timeout(Duration::from_secs(2))?.trim_end(),
            "GET /vault/icons/example.com/icon.png HTTP/1.1"
        );
        worker
            .join()
            .map_err(|_| std::io::Error::other("redirect fixture server panicked"))??;

        let (server, request, worker) = serve_icon_once(
            "200 OK",
            "Content-Type: image/png\r\n",
            Vec::new(),
            Some(128 * 1024 + 1),
        )?;
        let url = website_icon_url("example.com", 2, &server)
            .ok_or_else(|| std::io::Error::other("valid local server required"))?;
        assert!(runtime
            .block_on(fetch_icon_response(&local_icon_client()?, url))
            .is_none());
        assert!(request.recv_timeout(Duration::from_secs(2)).is_ok());
        worker
            .join()
            .map_err(|_| std::io::Error::other("oversized fixture server panicked"))??;
        Ok(())
    }

    #[test]
    fn desktop_username_payload_matches_sdk_schema_and_rejects_malformed_addresses() {
        let bytes = username_payload(Username::Subaddress {
            email: "user@example.com",
            website: Some("shop.example.com"),
        })
        .unwrap();
        let payload: bitwarden_generators::UsernameGeneratorRequest =
            serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
        assert!(matches!(
            payload,
            bitwarden_generators::UsernameGeneratorRequest::Subaddress { .. }
        ));
        for email in [
            "",
            "user",
            "@example.com",
            "a@@example.com",
            "user@https://example.com",
            "a\n@example.com",
            ".a@example.com",
            "a..b@example.com",
        ] {
            assert!(username_payload(Username::Subaddress {
                email,
                website: None
            })
            .is_err());
        }
        let overlong_local = format!("{}@example.com", "a".repeat(56));
        assert!(username_payload(Username::Subaddress {
            email: &overlong_local,
            website: None
        })
        .is_err());
        let overlong_website = "a".repeat(65);
        assert!(username_payload(Username::Catchall {
            domain: "example.com",
            website: Some(&overlong_website)
        })
        .is_err());
        for domain in [
            "",
            "https://example.com",
            "example.com/path",
            "user@example.com",
            "-example.com",
            "example..com",
        ] {
            assert!(username_payload(Username::Catchall {
                domain,
                website: None
            })
            .is_err());
        }
        for website in [
            "",
            ".",
            ".example",
            "example.",
            "a..b",
            "https://example.com",
            "a@b",
            "a\nb",
            "a\0b",
        ] {
            assert!(username_payload(Username::Catchall {
                domain: "example.com",
                website: Some(website)
            })
            .is_err());
        }
    }

    #[test]
    fn desktop_username_uses_shared_request_and_rejects_cancelled_generation() {
        let _guard = super::super::AUTH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let request = Request::begin();
        let word = request
            .username(Username::Word {
                capitalize: true,
                include_number: true,
            })
            .unwrap();
        assert!(word
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase()));
        assert_eq!(word.chars().filter(char::is_ascii_digit).count(), 4);
        let address = request
            .username(Username::Subaddress {
                email: "user@example.com",
                website: Some("shop"),
            })
            .unwrap();
        assert_eq!(address.as_str(), "user+shop@example.com");
        let catchall = request
            .username(Username::Catchall {
                domain: "example.com",
                website: None,
            })
            .unwrap();
        let (local, domain) = catchall.split_once('@').unwrap();
        assert_eq!((local.len(), domain), (8, "example.com"));
        request.cancel();
        assert_eq!(
            request
                .username(Username::Word {
                    capitalize: false,
                    include_number: false
                })
                .err()
                .unwrap()
                .code(),
            "cancelled"
        );
        let stale = Request::begin();
        let current = Request::begin();
        assert_eq!(
            stale
                .username(Username::Word {
                    capitalize: false,
                    include_number: false
                })
                .err()
                .unwrap()
                .code(),
            "cancelled"
        );
        current.cancel();
    }

    #[test]
    fn desktop_password_strength_is_local_and_rejects_nul() {
        assert_eq!(password_strength("").unwrap(), None);
        assert!(password_strength("123456")
            .unwrap()
            .is_some_and(|score| score <= 1));
        assert_eq!(
            password_strength("Correct-Horse-Battery-Staple-99!@").unwrap(),
            Some(4)
        );
        assert_eq!(password_strength("a\0b").unwrap_err().code(), "validation");
    }

    #[test]
    fn rust_adapter_preserves_server_and_auth_challenge_rules() {
        assert!(Server::Us.is_valid());
        assert!(Server::Eu.is_valid());
        assert!(Server::SelfHosted("http://127.0.0.1:9999").is_valid());
        assert!(!Server::SelfHosted("http://example.invalid").is_valid());
        assert_eq!(argument("a\0b").unwrap_err().code(), "validation");
        let AuthStatus::TwoFactor(providers) = AuthStatus::from_raw(
            TWO_FACTOR_REQUIRED
                | ((TWO_FACTOR_EMAIL | TWO_FACTOR_YUBIKEY) << TWO_FACTOR_MASK_SHIFT),
        ) else {
            panic!("challenge lost")
        };
        assert!(providers.supports(Provider::Email));
        assert!(providers.supports(Provider::YubiKey));
        assert!(!providers.supports(Provider::Authenticator));
    }

    #[test]
    fn rust_adapter_preserves_response_errors_and_typed_metadata() {
        assert_eq!(
            read::<Overview>(vault_error("locked"))
                .err()
                .unwrap()
                .code(),
            "locked"
        );
        let response = into_raw_string(r#"{"ok":true,"data":{"items":[],"folders":[{"id":"1","name":"Test"}],"lastSync":"","failures":0,"readOnly":true}}"#.into());
        let overview: Overview = read(response).unwrap();
        assert!(overview.read_only);
        assert_eq!(overview.folders[0].name, "Test");
        assert!(overview.items.is_empty());
    }

    #[test]
    fn website_icon_urls_are_host_only_and_server_scoped() -> Result<(), &'static str> {
        let us = website_icon_url(" ExAmPlE.com. ", 0, "").ok_or("US endpoint missing")?;
        assert_eq!(
            us.as_str(),
            "https://icons.bitwarden.net/example.com/icon.png"
        );
        let eu = website_icon_url("example.com", 1, "").ok_or("EU endpoint missing")?;
        assert_eq!(eu.as_str(), us.as_str());
        let custom = website_icon_url("example.com", 2, "https://vault.example.net/bitwarden/")
            .ok_or("custom endpoint missing")?;
        assert_eq!(
            custom.as_str(),
            "https://vault.example.net/bitwarden/icons/example.com/icon.png"
        );
        assert!(website_icon_url("example.com/path?token=secret", 0, "").is_none());
        assert!(website_icon_url("127.0.0.1", 0, "").is_none());
        assert!(
            website_icon_url("example.com", 2, "https://user:secret@vault.example.net").is_none()
        );
        assert!(
            website_icon_url("example.com", 2, "https://vault.example.net/?token=secret").is_none()
        );
        Ok(())
    }

    #[test]
    fn account_preference_scope_is_stable_private_and_server_specific() {
        let account = |id: &str, mode, server: &str, email: &str| Account {
            id: id.into(),
            email: email.into(),
            server_mode: mode,
            server_url: server.into(),
            state: SESSION_LOCKED,
            active: false,
            needs_sign_in: false,
            hello_enabled: false,
            pin_enabled: false,
        };
        let first = account("temporary", 0, "", " Person@Example.invalid ")
            .settings_scope()
            .unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            first,
            account("new-session", 0, "", "person@example.invalid")
                .settings_scope()
                .unwrap()
        );
        assert_ne!(
            first,
            account("temporary", 1, "", "person@example.invalid")
                .settings_scope()
                .unwrap()
        );
        assert_eq!(
            account(
                "a",
                2,
                "https://EXAMPLE.invalid/vault/",
                "person@example.invalid"
            )
            .settings_scope()
            .unwrap(),
            account(
                "b",
                2,
                "https://example.invalid/vault",
                "person@example.invalid"
            )
            .settings_scope()
            .unwrap()
        );
        assert!(account(
            "a",
            2,
            "https://example.invalid/?secret=invalid",
            "person@example.invalid"
        )
        .settings_scope()
        .is_err());
    }

    #[test]
    fn timeout_adapter_serializes_targets_and_rejects_invalid_ids() {
        let action = AccountTimeout {
            id: "11111111-1111-4111-8111-111111111111".into(),
            log_out: true,
        };
        assert_eq!(
            serde_json::to_string(&[action]).unwrap(),
            r#"[{"id":"11111111-1111-4111-8111-111111111111","logOut":true}]"#
        );
        let error = match apply_timeouts(
            &[AccountTimeout {
                id: "invalid".into(),
                log_out: false,
            }],
            false,
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid account identifiers must be rejected"),
        };
        assert_eq!(error.code(), "validation");
    }
}
