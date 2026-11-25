use std::{path::Path, sync::OnceLock, time::Duration};

use bitwarden_api_api::models::FileUploadType;
use bitwarden_api_base::AuthRequired;
use bitwarden_crypto::{EncString, OctetStreamBytes, PrimitiveEncryptable};
use bitwarden_send::SendFileView;
use bytes::Bytes;
use reqwest::{multipart, redirect::Policy, StatusCode};

use super::*;
use crate::vault::attachments::{allowed_url, read_upload_file};

const MAX_FILE_BYTES: usize = 500 * 1024 * 1024;
const MAX_SINGLE_BLOB_BYTES: usize = 256 * 1024 * 1024;
const BLOCK_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct PendingFile {
    id: SendId,
    key: EncString,
}

fn valid_file_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

pub(super) async fn create(
    session: &AuthSession,
    mut draft: Draft,
    source: Zeroizing<String>,
    started: &Cell<bool>,
    pending: &Cell<Option<PendingFile>>,
    request_id: u64,
) -> Result<Response, VaultError> {
    draft.validate(Utc::now(), None)?;
    if draft.text.is_some() || draft.hidden {
        return Err(VaultError::Validation);
    }
    let path = Path::new(source.as_str());
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| path.is_absolute() && !value.trim().is_empty())
        .ok_or(VaultError::AttachmentFileRead)?;
    let file = SendFileView {
        id: None,
        file_name: name.to_owned(),
        size: None,
        size_name: None,
    };
    let request = draft.into_requests(false, SendViewType::File(file)).0;
    let auth = request.auth.auth_type();
    let mut content = read_upload_file(path, MAX_FILE_BYTES).await?;
    let client = &session.client;
    let store = client.internal.get_key_store();
    // Use the SDK's own metadata encryption and file envelope. Keep the generated ciphertext
    // and its identity even if the server's upload-slot response cannot be fully validated.
    let mut model = store.encrypt(request).map_err(|_| VaultError::Internal)?;
    let key: EncString = model.key.parse().map_err(|_| VaultError::Internal)?;
    let encrypted_name = model
        .file
        .as_ref()
        .and_then(|file| file.file_name.clone())
        .ok_or(VaultError::Internal)?;
    let ciphertext = {
        let mut ctx = store.context();
        let slot = Send::get_key(&mut ctx, &key, SymmetricKeySlotId::User)
            .map_err(|_| VaultError::Internal)?;
        // Transfer ownership instead of cloning a potentially 500 MiB plaintext allocation.
        // The SDK's process-wide zeroizing allocator wipes the moved buffer on drop.
        let plaintext = OctetStreamBytes::from(std::mem::take(&mut *content));
        let encrypted = plaintext
            .encrypt(&mut ctx, slot)
            .map_err(|_| VaultError::Internal)?;
        drop(plaintext);
        Bytes::from(encrypted.to_buffer().map_err(|_| VaultError::Internal)?)
    };
    let encrypted_size = ciphertext.len();
    model.file_length = Some(encrypted_size as i64);
    if !auth_request::is_current(request_id) {
        return Err(VaultError::Cancelled);
    }

    let config = client.internal.get_api_configurations();
    let sends = config.api_client.sends_api();
    started.set(true);
    let slot = tokio::time::timeout(auth_request::TIMEOUT, sends.post_file(Some(model)))
        .await
        .map_err(|_| VaultError::WriteUncertain)?
        .map_err(|error| match &error {
            ApiError::Response(response) if response.status == StatusCode::PAYLOAD_TOO_LARGE => {
                VaultError::AttachmentTooLarge
            }
            ApiError::Response(response) if response.status == StatusCode::NOT_FOUND => {
                VaultError::Unsupported
            }
            _ => api_error(error, true),
        })?;
    let response = slot.send_response.ok_or(VaultError::WriteUncertain)?;
    let id = response
        .id
        .filter(|id| !id.is_nil())
        .map(SendId::new)
        .ok_or(VaultError::WriteUncertain)?;
    pending.set(Some(PendingFile {
        id,
        key: key.clone(),
    }));
    let send = Send::try_from(*response).map_err(|_| VaultError::WriteUncertain)?;
    let file = send
        .file
        .as_ref()
        .filter(|file| {
            file.file_name.to_string() == encrypted_name
                && file
                    .size
                    .as_ref()
                    .and_then(|size| size.parse::<usize>().ok())
                    == Some(encrypted_size)
        })
        .ok_or(VaultError::WriteUncertain)?;
    if send.key != key || send.r#type != SendType::File {
        return Err(VaultError::WriteUncertain);
    }
    let file_id = file
        .id
        .as_deref()
        .filter(|id| valid_file_id(id))
        .ok_or(VaultError::WriteUncertain)?;
    let expected = SendFileView {
        id: Some(file_id.to_owned()),
        file_name: name.to_owned(),
        size: file.size.clone(),
        size_name: file.size_name.clone(),
    };
    // Verify the file key and metadata before sending any ciphertext to an upload endpoint.
    confirm_write(
        decrypt_detail(client, &send)?,
        Some(id),
        auth,
        Some(&expected),
    )?;
    match slot.file_upload_type.ok_or(VaultError::WriteUncertain)? {
        FileUploadType::Direct => {
            let form = multipart::Form::new().part(
                "data",
                multipart::Part::stream_with_length(ciphertext, encrypted_size as u64)
                    .file_name(encrypted_name)
                    .mime_str("application/octet-stream")
                    .map_err(|_| VaultError::Internal)?,
            );
            let url = format!("{}/sends/{id}/file/{file_id}", config.api_config.base_path);
            let response = config
                .api_config
                .client
                .post(url)
                .with_extension(AuthRequired::Bearer)
                .multipart(form)
                .send()
                .await
                .map_err(|_| VaultError::Network)?;
            if !response.status().is_success() {
                return Err(VaultError::Network);
            }
        }
        FileUploadType::Azure => {
            let target = SignedUpload {
                session,
                id,
                file_id,
            };
            let url = target.parse(&slot.url.ok_or(VaultError::WriteUncertain)?)?;
            target.upload(url, ciphertext).await?;
        }
        FileUploadType::__Unknown(_) => return Err(VaultError::Unsupported),
    }
    let confirmed = tokio::time::timeout(auth_request::TIMEOUT, fetch(client, id))
        .await
        .map_err(|_| VaultError::WriteUncertain)??;
    if confirmed.key != key || confirmed.access_id != send.access_id {
        return Err(VaultError::WriteUncertain);
    }
    // Also validate that a usable link exists, without exposing it to the caller.
    share_link(session, &confirmed)?;
    confirm_write(
        decrypt_detail(client, &confirmed)?,
        Some(id),
        auth,
        Some(&expected),
    )
}

pub(super) async fn rollback(client: &Client, pending: PendingFile) -> bool {
    tokio::time::timeout(auth_request::TIMEOUT, async {
        match fetch(client, pending.id).await {
            Err(VaultError::NotFound) => return true,
            Ok(send) if send.r#type == SendType::File && send.key == pending.key => (),
            _ => return false,
        }
        let config = client.internal.get_api_configurations();
        match config
            .api_client
            .sends_api()
            .delete(&pending.id.to_string())
            .await
        {
            Ok(()) => true,
            Err(error) => matches!(api_error(error, true), VaultError::NotFound),
        }
    })
    .await
    .unwrap_or(false)
}

struct SignedUpload<'a> {
    session: &'a AuthSession,
    id: SendId,
    file_id: &'a str,
}

impl SignedUpload<'_> {
    fn parse(&self, signed: &str) -> Result<Url, VaultError> {
        let api = Url::parse(&self.session.server.api_url).map_err(|_| VaultError::Internal)?;
        let url = Url::parse(signed).map_err(|_| VaultError::InvalidResponse)?;
        if !allowed_url(&url, &api) {
            return Err(VaultError::InvalidResponse);
        }
        Ok(url)
    }

    // NodeWarden advertises Azure but uses a signed same-origin PUT that expects the entire
    // ciphertext, not Azure's block protocol. Never send account credentials to a signed URL.
    fn is_self_hosted_upload(&self, url: &Url) -> bool {
        let Ok(api) = Url::parse(&self.session.server.api_url) else {
            return false;
        };
        self.session.server.self_hosted
            && url.origin() == api.origin()
            && url.path()
                == format!(
                    "{}/sends/{}/file/{}",
                    api.path().trim_end_matches('/'),
                    self.id,
                    self.file_id
                )
            && url
                .query_pairs()
                .any(|(key, value)| key == "token" && !value.is_empty())
    }

    async fn renew(&self) -> Result<Url, VaultError> {
        let url = tokio::time::timeout(
            auth_request::TIMEOUT,
            self.session
                .client
                .sends()
                .renew_file_upload_url(self.id, self.file_id.to_owned()),
        )
        .await
        .map_err(|_| VaultError::Network)?
        .map_err(|_| VaultError::Network)?;
        self.parse(&url)
    }

    async fn upload(&self, mut url: Url, ciphertext: Bytes) -> Result<(), VaultError> {
        if expiring(&url) {
            url = self.renew().await?;
        }
        if ciphertext.len() <= MAX_SINGLE_BLOB_BYTES || self.is_self_hosted_upload(&url) {
            return self.put(&mut url, &[], ciphertext, true).await;
        }
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList>");
        for (i, start) in (0..ciphertext.len()).step_by(BLOCK_BYTES).enumerate() {
            use base64::engine::general_purpose::STANDARD;
            let id = STANDARD.encode((i as u32).to_be_bytes());
            let block = ciphertext.slice(start..(start + BLOCK_BYTES).min(ciphertext.len()));
            self.put(
                &mut url,
                &[("comp", "block"), ("blockid", &id)],
                block,
                false,
            )
            .await?;
            xml.push_str(&format!("<Latest>{id}</Latest>"));
        }
        xml.push_str("</BlockList>");
        self.put(&mut url, &[("comp", "blocklist")], Bytes::from(xml), false)
            .await
    }

    async fn put(
        &self,
        url: &mut Url,
        args: &[(&str, &str)],
        bytes: Bytes,
        single: bool,
    ) -> Result<(), VaultError> {
        let status = put_blob(url, args, bytes.clone(), single).await?;
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            *url = self.renew().await?;
            if put_blob(url, args, bytes, single).await? == StatusCode::CREATED {
                return Ok(());
            }
        } else if status == StatusCode::CREATED {
            return Ok(());
        }
        Err(VaultError::Network)
    }
}

fn expiring(url: &Url) -> bool {
    url.query_pairs()
        .find(|(key, _)| key == "se")
        .and_then(|(_, value)| DateTime::parse_from_rfc3339(&value).ok())
        .is_some_and(|date| date <= Utc::now() + TimeDelta::seconds(30))
}

async fn put_blob(
    url: &Url,
    args: &[(&str, &str)],
    bytes: Bytes,
    single: bool,
) -> Result<StatusCode, VaultError> {
    static LOCAL: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    static HTTPS: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    let loopback = url.scheme() == "http"; // Validated against the configured loopback API origin.
    let client = (if loopback { &LOCAL } else { &HTTPS })
        .get_or_init(|| {
            let builder = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(auth_request::TIMEOUT)
                .redirect(Policy::none());
            let builder = if loopback {
                builder.no_proxy()
            } else {
                builder.https_only(true)
            };
            builder.build().ok()
        })
        .as_ref()
        .ok_or(VaultError::Internal)?;
    let version = url
        .query_pairs()
        .find(|(name, _)| name == "sv")
        .map(|(_, value)| value.into_owned());
    let mut target = url.clone();
    if !args.is_empty() {
        target.query_pairs_mut().extend_pairs(args);
    }
    let content_type = if args == [("comp", "blocklist")] {
        "application/xml"
    } else {
        "application/octet-stream"
    };
    let mut request = client
        .put(target)
        .header(http::header::CONTENT_TYPE, content_type)
        .body(bytes);
    if single {
        request = request.header("x-ms-blob-type", "BlockBlob");
    }
    if let Some(version) = version {
        request = request.header("x-ms-version", version);
    }
    request
        .send()
        .await
        .map(|response| response.status())
        .map_err(|_| VaultError::Network)
}
