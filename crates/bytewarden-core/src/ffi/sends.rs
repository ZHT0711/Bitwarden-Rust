//! Online Send management. Only selected content or an explicitly requested link crosses the ABI.
use std::{cell::Cell, collections::HashSet};

mod files;

/// The Send SDK omits unset fields. A Send edit is a full replacement, but some
/// self-hosted servers treat absent options as "keep" and explicit null as "clear".
/// Leave authentication fields omitted: an ordinary edit must preserve them.
pub(super) struct SendUpdateCompatibility;

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for SendUpdateCompatibility {
    async fn handle(
        &self,
        mut request: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        if request.method() == reqwest::Method::PUT
            && request.url().path_segments().is_some_and(|parts| {
                let mut parts = parts.rev();
                parts.next().is_some_and(|id| Uuid::parse_str(id).is_ok())
                    && parts.next() == Some("sends")
            })
        {
            if let Some(bytes) = request.body().and_then(reqwest::Body::as_bytes) {
                if let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_slice(bytes) {
                    for optional in ["notes", "maxAccessCount", "expirationDate"] {
                        fields.entry(optional).or_insert(serde_json::Value::Null);
                    }
                    request.headers_mut().remove(http::header::CONTENT_LENGTH);
                    *request.body_mut() =
                        Some(serde_json::Value::Object(fields).to_string().into());
                }
            }
        }
        next.run(request, extensions).await
    }
}

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bitwarden_crypto::Decryptable;
use bitwarden_send::{
    AuthEdit, AuthType, CreateSendError, EditSendError, Send, SendAddRequest, SendAuthType,
    SendClientExt, SendEditRequest, SendId, SendListView, SendTextView, SendType, SendView,
    SendViewType,
};
use bw_sdk_core::{key_management::SymmetricKeySlotId, ApiError, Client};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    account_id: Uuid,
    command: Command,
}

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Command {
    List,
    Detail {
        id: SendId,
    },
    Link {
        id: SendId,
    },
    Create {
        data: Draft,
    },
    CreateFile {
        data: Draft,
        source: String,
    },
    Edit {
        id: SendId,
        revision: DateTime<Utc>,
        data: Draft,
    },
    Delete {
        id: SendId,
        revision: DateTime<Utc>,
    },
    RemoveAuth {
        id: SendId,
        revision: DateTime<Utc>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Draft {
    name: String,
    text: Option<String>,
    notes: Option<String>,
    hidden: bool,
    disabled: bool,
    hide_email: bool,
    max_access_count: Option<u32>,
    deletion_date: DateTime<Utc>,
    expiration_date: Option<DateTime<Utc>>,
    password: Option<String>,
    emails: Option<Vec<String>>,
}

impl Drop for Draft {
    fn drop(&mut self) {
        self.text.zeroize();
        self.notes.zeroize();
        self.password.zeroize();
        self.emails.zeroize();
    }
}

impl Draft {
    fn validate(&mut self, now: DateTime<Utc>, existing: Option<&Send>) -> Result<(), VaultError> {
        self.name = self.name.trim().to_owned();
        let deletion_changed = existing.is_none_or(|send| send.deletion_date != self.deletion_date);
        let expiration_changed =
            existing.is_none_or(|send| send.expiration_date != self.expiration_date);
        if self.name.is_empty()
            || self.name.encode_utf16().count() > 1_000
            || self
                .notes
                .as_ref()
                .is_some_and(|notes| notes.encode_utf16().count() > 1_000)
            || self.password.as_ref().is_some_and(|password| {
                password.is_empty() || password.encode_utf16().count() > 1_000
            })
            || self
                .max_access_count
                .is_some_and(|count| !(1..=i32::MAX as u32).contains(&count))
            || (deletion_changed
                && (self.deletion_date <= now || self.deletion_date > now + TimeDelta::days(31)))
            || ((deletion_changed || expiration_changed)
                && self.expiration_date.is_some_and(|date| {
                    (expiration_changed && date <= now) || date > self.deletion_date
                }))
            || self
                .emails
                .as_ref()
                .is_some_and(|emails| !valid_emails(emails))
            || (self.password.is_some() && self.emails.is_some())
        {
            return Err(VaultError::Validation);
        }
        Ok(())
    }

    fn take_text(&mut self) -> Result<SendViewType, VaultError> {
        if self
            .text
            .as_ref()
            .is_none_or(|text| text.is_empty() || text.encode_utf16().count() > 1_000)
        {
            return Err(VaultError::Validation);
        }
        Ok(SendViewType::Text(SendTextView {
            text: self.text.take(),
            hidden: self.hidden,
        }))
    }

    fn into_requests(mut self, existing: bool, view_type: SendViewType) -> (SendAddRequest, bool) {
        let auth = if let Some(password) = self.password.take() {
            SendAuthType::from_plaintext_password(password)
        } else if let Some(emails) = self.emails.take() {
            SendAuthType::Emails { emails }
        } else {
            SendAuthType::None
        };
        let replace_auth = !existing || !matches!(auth, SendAuthType::None);
        (
            SendAddRequest {
                name: std::mem::take(&mut self.name),
                notes: self.notes.take().filter(|notes| !notes.is_empty()),
                view_type,
                max_access_count: self.max_access_count,
                disabled: self.disabled,
                hide_email: self.hide_email,
                deletion_date: self.deletion_date,
                expiration_date: self.expiration_date,
                auth,
            },
            replace_auth,
        )
    }
}

fn valid_emails(emails: &[String]) -> bool {
    let mut remaining = 2499usize;
    !emails.is_empty()
        && emails.iter().all(|email| {
            let Some((local, domain)) = email.split_once('@') else {
                return false;
            };
            if local.is_empty()
                || domain.is_empty()
                || email.len() > 254
                || domain.contains('@')
                || email
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || ",;<>\"".contains(c))
            {
                return false;
            }
            let Some(left) = remaining.checked_sub(email.len() + 1) else {
                return false;
            };
            remaining = left;
            true
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    id: SendId,
    name: String,
    kind: SendType,
    disabled: bool,
    deletion_date: DateTime<Utc>,
    expiration_date: Option<DateTime<Utc>>,
    revision: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    items: Vec<Summary>,
    failures: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    #[serde(flatten)]
    summary: Summary,
    notes: Option<String>,
    text: Option<String>,
    file_name: Option<String>,
    hidden: bool,
    hide_email: bool,
    max_access_count: Option<u32>,
    access_count: u32,
    auth_type: AuthType,
    has_password: bool,
    emails: Vec<String>,
}

impl Drop for Detail {
    fn drop(&mut self) {
        self.notes.zeroize();
        self.text.zeroize();
        self.emails.zeroize();
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Response {
    List(Overview),
    Detail(Detail),
    Link(#[serde(serialize_with = "serialize_link")] Zeroizing<String>),
    Written(bool),
}

fn serialize_link<S: serde::Serializer>(link: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(link)
}

fn api_error(error: ApiError, writing: bool) -> VaultError {
    if let ApiError::Response(response) = error {
        match response.status.as_u16() {
            400 | 422 => VaultError::Validation,
            401 => VaultError::NotAuthenticated,
            403 => VaultError::PermissionDenied,
            404 => VaultError::NotFound,
            409 | 412 => VaultError::Conflict,
            501 => VaultError::Unsupported,
            _ if writing => VaultError::WriteUncertain,
            _ => VaultError::Network,
        }
    } else if writing {
        VaultError::WriteUncertain
    } else {
        VaultError::Network
    }
}

async fn list(client: &Client) -> Result<Overview, VaultError> {
    let config = client.internal.get_api_configurations();
    let response = config
        .api_client
        .sends_api()
        .get_all()
        .await
        .map_err(|error| api_error(error, false))?;
    if response
        .continuation_token
        .is_some_and(|token| !token.is_empty())
    {
        return Err(VaultError::InvalidResponse);
    }
    let data = response.data.ok_or(VaultError::InvalidResponse)?;
    let total = data.len();
    let mut items = Vec::with_capacity(total);
    let mut seen = HashSet::with_capacity(total);
    let store = client.internal.get_key_store();
    for model in data {
        if let Some(id) = model.id {
            if !seen.insert(id) {
                return Err(VaultError::InvalidResponse);
            }
        }
        let Ok(send) = Send::try_from(model) else {
            continue;
        };
        let Ok(view): Result<SendListView, _> = store.decrypt(&send) else {
            continue;
        };
        let Some(id) = view.id else {
            continue;
        };
        items.push((
            view.name.to_lowercase(),
            Summary {
                id,
                name: view.name,
                kind: view.r#type,
                disabled: view.disabled,
                deletion_date: view.deletion_date,
                expiration_date: view.expiration_date,
                revision: view.revision_date,
            },
        ));
    }
    items.sort_unstable_by(|(left_name, left), (right_name, right)| {
        left_name
            .cmp(right_name)
            .then_with(|| Uuid::from(left.id).cmp(&Uuid::from(right.id)))
    });
    Ok(Overview {
        failures: total - items.len(),
        items: items.into_iter().map(|(_, item)| item).collect(),
    })
}

async fn fetch(client: &Client, id: SendId) -> Result<Send, VaultError> {
    let config = client.internal.get_api_configurations();
    let model = config
        .api_client
        .sends_api()
        .get(&id.to_string())
        .await
        .map_err(|error| api_error(error, false))?;
    if model.id != Some(id.into()) {
        return Err(VaultError::InvalidResponse);
    }
    model.try_into().map_err(|_| VaultError::InvalidResponse)
}

fn decrypt_detail(client: &Client, send: &Send) -> Result<SendView, VaultError> {
    let mut view: SendView = client
        .internal
        .get_key_store()
        .decrypt(send)
        .map_err(|_| VaultError::DecryptionFailed)?;
    view.key.zeroize();
    // SDK detail decryption tolerates damaged optional fields; never edit them away silently.
    if view.name.is_empty()
        || (send.notes.is_some() && view.notes.is_none())
        || (send.r#type == SendType::Text
            && view
                .text
                .as_ref()
                .and_then(|text| text.text.as_ref())
                .is_none())
        || (send.r#type == SendType::File
            && view
                .file
                .as_ref()
                .is_none_or(|file| file.file_name.trim().is_empty()))
    {
        return Err(VaultError::DecryptionFailed);
    }
    Ok(view)
}

fn detail(client: &Client, id: SendId, send: &Send) -> Result<Detail, VaultError> {
    let view = decrypt_detail(client, send)?;
    let hidden = view.text.as_ref().is_some_and(|text| text.hidden);
    Ok(Detail {
        summary: Summary {
            id,
            name: view.name,
            kind: view.r#type,
            disabled: view.disabled,
            deletion_date: view.deletion_date,
            expiration_date: view.expiration_date,
            revision: view.revision_date,
        },
        notes: view.notes,
        text: view.text.and_then(|text| text.text),
        file_name: view.file.map(|file| file.file_name),
        hidden,
        hide_email: view.hide_email,
        max_access_count: view.max_access_count,
        access_count: view.access_count,
        auth_type: view.auth_type,
        has_password: view.has_password,
        emails: view.emails,
    })
}

fn share_link(session: &AuthSession, send: &Send) -> Result<Zeroizing<String>, VaultError> {
    let access_id = send
        .access_id
        .as_deref()
        .ok_or(VaultError::InvalidResponse)?;
    // Both link segments are fixed-size URL-safe base64, not arbitrary server-supplied paths.
    if URL_SAFE_NO_PAD
        .decode(access_id)
        .map_or(true, |id| id.len() != 16)
    {
        return Err(VaultError::InvalidResponse);
    }
    let store = session.client.internal.get_key_store();
    let mut ctx = store.context();
    let key: Vec<u8> = send
        .key
        .decrypt(&mut ctx, SymmetricKeySlotId::User)
        .map_err(|_| VaultError::DecryptionFailed)?;
    let key = Zeroizing::new(key);
    if key.len() != 16 {
        return Err(VaultError::DecryptionFailed);
    }
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*key));
    let base = match session.server_mode {
        0 => "https://vault.bitwarden.com",
        1 => "https://vault.bitwarden.eu",
        _ => session.server_url.trim().trim_end_matches('/'),
    };
    Ok(Zeroizing::new(format!(
        "{base}/#/send/{access_id}/{}",
        encoded.as_str()
    )))
}

fn require_revision(send: &Send, revision: DateTime<Utc>) -> Result<(), VaultError> {
    if send.revision_date != revision {
        Err(VaultError::Conflict)
    } else {
        Ok(())
    }
}

fn confirm_write(
    mut view: SendView,
    id: Option<SendId>,
    auth: AuthType,
    expected_file: Option<&bitwarden_send::SendFileView>,
) -> Result<Response, VaultError> {
    let has_text = view.text.as_mut().is_some_and(|text| {
        let present = text.text.as_ref().is_some_and(|value| !value.is_empty());
        text.text.zeroize();
        present
    });
    view.key.zeroize();
    view.notes.zeroize();
    let has_emails = !view.emails.is_empty();
    view.emails.zeroize();
    let valid_content = match expected_file {
        Some(file) => view.r#type == SendType::File && view.file.as_ref() == Some(file),
        None => view.r#type == SendType::Text && has_text,
    };
    if view.id.is_none()
        || id.is_some_and(|id| view.id != Some(id))
        || view.name.is_empty()
        || !valid_content
        || view.auth_type != auth
        || (auth == AuthType::Password && !view.has_password)
        || (auth == AuthType::Email && !has_emails)
        || (auth == AuthType::None && (has_emails || view.has_password))
    {
        return Err(VaultError::WriteUncertain);
    }
    Ok(Response::Written(true))
}

async fn execute(
    session: &AuthSession,
    command: Command,
    started: &Cell<bool>,
    pending_file: &Cell<Option<files::PendingFile>>,
    request_id: u64,
) -> Result<Response, VaultError> {
    let client = &session.client;
    match command {
        Command::List => Ok(Response::List(list(client).await?)),
        Command::Detail { id } => Ok(Response::Detail(detail(
            client,
            id,
            &fetch(client, id).await?,
        )?)),
        Command::Link { id } => Ok(Response::Link(share_link(
            session,
            &fetch(client, id).await?,
        )?)),
        _ if session.send_write_uncertain => Err(VaultError::WriteUncertain),
        Command::Create { mut data } => {
            data.validate(Utc::now(), None)?;
            let text = data.take_text()?;
            let request = data.into_requests(false, text).0;
            let auth = request.auth.auth_type();
            started.set(true);
            let view = client
                .sends()
                .create(request)
                .await
                .map_err(|error| match error {
                    CreateSendError::Api(error) => api_error(error, true),
                    _ => VaultError::WriteUncertain,
                })?;
            confirm_write(view, None, auth, None)
        }
        Command::CreateFile { data, source } => {
            files::create(
                session,
                data,
                Zeroizing::new(source),
                started,
                pending_file,
                request_id,
            )
            .await
        }
        Command::Edit {
            id,
            revision,
            mut data,
        } => {
            let current = fetch(client, id).await?;
            require_revision(&current, revision)?;
            if !matches!(current.r#type, SendType::Text | SendType::File) {
                return Err(VaultError::Unsupported);
            }
            let mut original = decrypt_detail(client, &current)?;
            let previous_auth = original.auth_type;
            let has_password = original.has_password;
            original.notes.zeroize();
            original.emails.zeroize();
            if let Some(text) = &mut original.text {
                text.text.zeroize();
            }
            // A server returning inconsistent auth fields must be repaired explicitly rather than
            // silently dropping a previously applied password during an ordinary content edit.
            if previous_auth == AuthType::None && has_password {
                return Err(VaultError::InvalidResponse);
            }
            data.validate(Utc::now(), Some(&current))?;
            let expected_file = original.file;
            let content = match &expected_file {
                Some(file) if data.text.is_none() && !data.hidden => {
                    SendViewType::File(file.clone())
                }
                Some(_) => return Err(VaultError::Validation),
                None => data.take_text()?,
            };
            let repository = client
                .platform()
                .state()
                .get::<Send>()
                .map_err(|_| VaultError::Internal)?;
            repository
                .set(id, current)
                .await
                .map_err(|_| VaultError::Internal)?;
            let (data, replace_auth) = data.into_requests(true, content);
            let auth = if replace_auth {
                data.auth.auth_type()
            } else {
                previous_auth
            };
            let edit = SendEditRequest {
                name: data.name,
                notes: data.notes,
                view_type: data.view_type,
                max_access_count: data.max_access_count,
                disabled: data.disabled,
                hide_email: data.hide_email,
                deletion_date: data.deletion_date,
                expiration_date: data.expiration_date,
                auth: if replace_auth {
                    AuthEdit::Set { auth: data.auth }
                } else {
                    AuthEdit::Preserve
                },
            };
            started.set(true);
            let view = client
                .sends()
                .edit(id, edit)
                .await
                .map_err(|error| match error {
                    EditSendError::Api(error) => api_error(error, true),
                    _ => VaultError::WriteUncertain,
                })?;
            confirm_write(view, Some(id), auth, expected_file.as_ref())
        }
        Command::RemoveAuth { id, revision } => {
            let current = fetch(client, id).await?;
            require_revision(&current, revision)?;
            if current.auth_type == AuthType::None
                && current.password.is_none()
                && current.emails.is_none()
            {
                return Ok(Response::Written(true));
            }
            let config = client.internal.get_api_configurations();
            started.set(true);
            // This is the route used by the official desktop, including legacy self-hosted servers.
            let sends = config.api_client.sends_api();
            let id_text = id.to_string();
            let response = if current.auth_type == AuthType::Email {
                sends.put_remove_auth(&id_text).await
            } else {
                sends.put_remove_password(&id_text).await
            }
            .map_err(|error| api_error(error, true))?;
            let updated = Send::try_from(response).map_err(|_| VaultError::WriteUncertain)?;
            if updated.id != Some(id)
                || updated.key.to_string() != current.key.to_string()
                || updated.r#type != current.r#type
                || updated.auth_type != AuthType::None
                || updated.password.is_some()
                || updated.emails.is_some()
            {
                return Err(VaultError::WriteUncertain);
            }
            Ok(Response::Written(true))
        }
        Command::Delete { id, revision } => {
            require_revision(&fetch(client, id).await?, revision)?;
            started.set(true);
            let config = client.internal.get_api_configurations();
            // Do not use the SDK's background delete queue: preview writes are never retried silently.
            config
                .api_client
                .sends_api()
                .delete(&id.to_string())
                .await
                .map_err(|error| api_error(error, true))?;
            if let Ok(repository) = client.platform().state().get::<Send>() {
                let _ = repository.remove(id).await;
            }
            Ok(Response::Written(true))
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn bw_send_command(json: *const c_char, request_id: u64) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(json) = (unsafe { read_utf8(json) })
            .map(Zeroizing::new)
            .filter(|json| json.len() <= 32 * 1024)
        else {
            return vault_error("validation");
        };
        let Ok(Request {
            account_id,
            command,
        }) = serde_json::from_str(&json)
        else {
            return vault_error("validation");
        };
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
        if session.account_id != account_id {
            return vault_error("cancelled");
        }
        if session.offline_only {
            return vault_error("signInRequired");
        }
        let started = Cell::new(false);
        let pending_file = Cell::new(None);
        let transfer = matches!(command, Command::CreateFile { .. });
        let result = catch_unwind(AssertUnwindSafe(|| {
            let operation = execute(session, command, &started, &pending_file, request_id);
            if transfer {
                runtime.block_on(auth_request::run_transfer(request_id, operation))
            } else {
                runtime.block_on(auth_request::run(request_id, operation))
            }
        }));
        if let Some(pending) = pending_file.take() {
            if !matches!(result, Ok(Ok(Ok(_)))) || !auth_request::is_current(request_id) {
                let rolled_back = catch_unwind(AssertUnwindSafe(|| {
                    runtime.block_on(files::rollback(&session.client, pending))
                }))
                .unwrap_or(false);
                if !rolled_back {
                    session.send_write_uncertain = true;
                    return vault_error("writeUncertain");
                }
                return match result {
                    Ok(Err(RequestError::Cancelled)) => vault_error("cancelled"),
                    Ok(Err(RequestError::TimedOut)) => vault_error("timedOut"),
                    _ if !auth_request::is_current(request_id) => vault_error("cancelled"),
                    _ => vault_error("sendFileUploadFailed"),
                };
            }
        }
        match result {
            Ok(Ok(Ok(response))) => {
                if matches!(response, Response::List(_)) {
                    session.send_write_uncertain = false;
                }
                if !auth_request::is_current(request_id) && !started.get() {
                    return vault_error("cancelled");
                }
                vault_success(&response)
            }
            Ok(Ok(Err(error))) => {
                if started.get() && matches!(error, VaultError::WriteUncertain) {
                    session.send_write_uncertain = true;
                }
                vault_error(&error.to_string())
            }
            _ if started.get() => {
                session.send_write_uncertain = true;
                vault_error("writeUncertain")
            }
            Ok(Err(RequestError::Cancelled)) => vault_error("cancelled"),
            Ok(Err(RequestError::TimedOut)) => vault_error("timedOut"),
            Err(_) => vault_error("internal"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}
