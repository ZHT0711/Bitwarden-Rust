use std::{io::Write, path::Path, sync::OnceLock, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use bitwarden_api_base::AuthRequired;
use bitwarden_vault::{
    AttachmentFileUploadType, AttachmentView, Cipher, CipherCreateAttachmentError,
    CipherDeleteAttachmentError, CipherId, CipherView, CreateAttachmentRequest, CreatedAttachment,
    VaultClientExt,
};
use bw_sdk_core::Client;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use reqwest::{multipart, redirect::Policy, StatusCode};
use serde::Serialize;
use tokio::io::AsyncReadExt;
use url::Url;
use zeroize::Zeroizing;

use super::{VaultCache, VaultError, VaultMutationResult};
use crate::auth_request;

// Bitwarden Desktop allows 500 MiB of plaintext. Allow for encrypted framing and padding.
const MAX_FILE_BYTES: usize = 500 * 1024 * 1024;
const MAX_ENCRYPTED_BYTES: usize = MAX_FILE_BYTES + 1024 * 1024;
const MAX_SINGLE_BLOB_BYTES: usize = 256 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentSummary {
    id: String,
    name: String,
    size: Option<u64>,
    can_download: bool,
}

pub(crate) struct PendingUpload {
    original: Cipher,
    attachment_id: String,
}

pub(super) fn permissions(view: &CipherView) -> (bool, bool) {
    if view.partial || view.deleted_date.is_some() {
        return (false, false);
    }
    (
        view.edit,
        view.permissions
            .as_ref()
            .map_or(view.edit, |permissions| permissions.delete),
    )
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn displayable(attachment: &AttachmentView) -> bool {
    attachment.id.as_deref().is_some_and(valid_id)
        && attachment
            .file_name
            .as_ref()
            .is_some_and(|name| !name.trim().is_empty())
}

pub(super) fn summaries(view: &CipherView) -> (Vec<AttachmentSummary>, usize) {
    if view.partial {
        return (Vec::new(), 0);
    }
    let mut failures = 0;
    let mut files = Vec::new();
    let entries = view
        .attachments
        .iter()
        .flatten()
        .map(|entry| (entry, displayable(entry)))
        .chain(
            view.attachment_decryption_failures
                .iter()
                .flatten()
                .map(|entry| (entry, false)),
        );
    for (attachment, can_download) in entries {
        let Some(id) = attachment.id.as_deref().filter(|id| valid_id(id)) else {
            failures += 1;
            continue;
        };
        files.push(AttachmentSummary {
            id: id.to_owned(),
            name: attachment
                .file_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_default(),
            size: attachment.size.as_ref().and_then(|size| size.parse().ok()),
            can_download,
        });
    }
    (files, failures)
}

impl VaultCache {
    async fn attachment_item(
        &self,
        client: &Client,
        item_id: &str,
        revision: &str,
    ) -> Result<(Cipher, CipherView), VaultError> {
        self.ensure_synced()?;
        let cipher = self.find_cipher(item_id)?;
        if cipher.revision_date.to_rfc3339() != revision {
            return Err(VaultError::Conflict);
        }
        let view = client
            .vault()
            .ciphers()
            .decrypt(cipher.clone())
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        if view.partial {
            return Err(VaultError::PermissionDenied);
        }
        Ok((cipher, view))
    }

    pub(crate) async fn decrypt_attachment(
        &self,
        client: &Client,
        api_url: &str,
        item_id: &str,
        revision: &str,
        attachment_id: &str,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        if !valid_id(attachment_id) {
            return Err(VaultError::Validation);
        }
        let (cipher, view) = self.attachment_item(client, item_id, revision).await?;
        let attachment = view
            .attachments
            .into_iter()
            .flatten()
            .find(|attachment| {
                attachment.id.as_deref() == Some(attachment_id) && displayable(attachment)
            })
            .ok_or(VaultError::AttachmentDownloadFailed)?;
        let attachments = client.vault().attachments();
        let url = tokio::time::timeout(
            auth_request::TIMEOUT,
            attachments.get_attachment_download_url(
                cipher.id.ok_or(VaultError::NotFound)?,
                attachment_id.to_owned(),
                None,
            ),
        )
        .await
        .map_err(|_| VaultError::AttachmentDownloadFailed)?
        .map_err(|_| VaultError::AttachmentDownloadFailed)?;
        let encrypted = download_blob(api_url, &url).await?;
        attachments
            .decrypt_buffer(cipher, attachment, &encrypted)
            .map(Zeroizing::new)
            .map_err(|_| VaultError::DecryptionFailed)
    }

    pub(crate) async fn upload_attachment(
        &mut self,
        client: &Client,
        detail: (&str, &str),
        source: &Path,
        request_id: u64,
        pending: &mut Option<PendingUpload>,
    ) -> Result<VaultMutationResult, VaultError> {
        let (original, view) = self.attachment_item(client, detail.0, detail.1).await?;
        if !permissions(&view).0 {
            return Err(VaultError::PermissionDenied);
        }
        let cipher_id = original.id.ok_or(VaultError::NotFound)?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|_| source.is_absolute())
            .ok_or(VaultError::AttachmentFileRead)?;
        let contents = read_upload_file(source, MAX_FILE_BYTES).await?;
        self.activate(client);
        let attachments = client.vault().attachments();
        let encrypted = attachments
            .encrypt_buffer(
                original.clone(),
                AttachmentView {
                    file_name: Some(name.to_owned()),
                    id: None,
                    url: None,
                    size: None,
                    size_name: None,
                    key: None,
                },
                &contents,
            )
            .map_err(|_| VaultError::AttachmentUploadFailed)?;
        drop(contents);
        let request = CreateAttachmentRequest {
            key: encrypted
                .attachment
                .key
                .ok_or(VaultError::AttachmentUploadFailed)?,
            file_name: encrypted
                .attachment
                .file_name
                .ok_or(VaultError::AttachmentUploadFailed)?,
            file_size: encrypted.contents.len() as u64,
            last_known_revision_date: original.revision_date,
        };
        if !auth_request::is_current(request_id) {
            return Err(VaultError::Cancelled);
        }
        self.write_started = true;
        let created = tokio::time::timeout(
            auth_request::TIMEOUT,
            Box::pin(attachments.create_attachment(cipher_id, request)),
        )
        .await
        .map_err(|_| self.uncertain())?
        .map_err(|error| match error {
            CipherCreateAttachmentError::Api(error) => self.classify_api_error(error),
            _ => self.uncertain(),
        })?;
        if !valid_id(&created.attachment_id) || has_attachment(&original, &created.attachment_id) {
            return Err(self.uncertain());
        }
        *pending = Some(PendingUpload {
            original,
            attachment_id: created.attachment_id.clone(),
        });
        let original = &pending
            .as_ref()
            .expect("new attachment slot recorded")
            .original;
        if !attachment_change_matches(original, &created.cipher, &created.attachment_id, true) {
            return Err(self.uncertain());
        }
        self.secure_metadata_cache(original).await?;
        Box::pin(upload_payload(
            client,
            cipher_id,
            &created,
            Bytes::from(encrypted.contents),
        ))
        .await?;
        *pending = None;
        self.finish_write(client, Some(cipher_id.to_string())).await
    }

    pub(crate) async fn delete_attachment(
        &mut self,
        client: &Client,
        detail: (&str, &str),
        attachment_id: &str,
    ) -> Result<VaultMutationResult, VaultError> {
        if !valid_id(attachment_id) {
            return Err(VaultError::Validation);
        }
        let (original, view) = self.attachment_item(client, detail.0, detail.1).await?;
        if !permissions(&view).1 {
            return Err(VaultError::PermissionDenied);
        }
        if !has_attachment(&original, attachment_id) {
            return Err(VaultError::NotFound);
        }
        self.activate(client);
        self.write_started = true;
        self.remove_attachment(client, &original, attachment_id)
            .await
    }

    pub(crate) async fn rollback_upload(
        &mut self,
        client: &Client,
        pending: PendingUpload,
    ) -> bool {
        tokio::time::timeout(
            Duration::from_secs(4),
            self.remove_attachment(client, &pending.original, &pending.attachment_id),
        )
        .await
        .is_ok_and(|result| result.is_ok())
    }

    async fn remove_attachment(
        &mut self,
        client: &Client,
        original: &Cipher,
        attachment_id: &str,
    ) -> Result<VaultMutationResult, VaultError> {
        let id = original.id.ok_or_else(|| self.uncertain())?;
        let attachments = client.vault().attachments();
        let cipher = Box::pin(attachments.delete_attachment(id, attachment_id.to_owned()))
            .await
            .map_err(|error| match error {
                CipherDeleteAttachmentError::Api(error) => self.classify_api_error(error),
                _ => self.uncertain(),
            })?;
        if !attachment_change_matches(original, &cipher, attachment_id, false) {
            return Err(self.uncertain());
        }
        self.secure_metadata_cache(original).await?;
        self.finish_write(client, Some(id.to_string())).await
    }
}

pub(crate) async fn read_upload_file(
    path: &Path,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| VaultError::AttachmentFileRead)?;
    let meta = file
        .metadata()
        .await
        .map_err(|_| VaultError::AttachmentFileRead)?;
    if !meta.is_file() {
        return Err(VaultError::AttachmentFileRead);
    }
    if meta.len() > limit as u64 {
        return Err(VaultError::AttachmentTooLarge);
    }
    let mut contents = Zeroizing::new(Vec::new());
    contents
        .try_reserve_exact(meta.len() as usize)
        .map_err(|_| VaultError::AttachmentTooLarge)?;
    file.take(limit as u64 + 1)
        .read_to_end(&mut contents)
        .await
        .map_err(|_| VaultError::AttachmentFileRead)?;
    if contents.len() > limit {
        return Err(VaultError::AttachmentTooLarge);
    }
    Ok(contents)
}

fn has_attachment(cipher: &Cipher, id: &str) -> bool {
    cipher
        .attachments
        .iter()
        .flatten()
        .any(|entry| entry.id.as_deref() == Some(id))
}

fn attachment_change_matches(original: &Cipher, updated: &Cipher, id: &str, added: bool) -> bool {
    original.id == updated.id
        && original.organization_id == updated.organization_id
        && (!added || original.key == updated.key)
        && has_attachment(updated, id) == added
        && original.attachments.iter().flatten().all(|entry| {
            entry.id.as_deref() == Some(id)
                || updated.attachments.iter().flatten().any(|other| {
                    other.id == entry.id
                        && other.file_name == entry.file_name
                        && other.key == entry.key
                })
        })
}

async fn upload_payload(
    client: &Client,
    cipher_id: CipherId,
    slot: &CreatedAttachment,
    contents: Bytes,
) -> Result<(), VaultError> {
    let config = client.internal.get_api_configurations();
    let api = &config.api_config;
    let response = match slot.file_upload_type {
        AttachmentFileUploadType::Direct => {
            let url = format!(
                "{}/ciphers/{cipher_id}/attachment/{}",
                api.base_path, slot.attachment_id
            );
            let length = contents.len() as u64;
            let part = multipart::Part::stream_with_length(contents, length).file_name("data");
            let form = multipart::Form::new().part("data", part);
            api.client
                .post(url)
                .with_extension(AuthRequired::Bearer)
                .multipart(form)
                .send()
                .await
                .map_err(|_| VaultError::AttachmentUploadFailed)?
        }
        AttachmentFileUploadType::Azure => {
            return upload_azure(client, cipher_id, slot, &api.base_path, contents).await
        }
    };
    if !response.status().is_success() {
        return Err(VaultError::AttachmentUploadFailed);
    }
    Ok(())
}

async fn upload_azure(
    client: &Client,
    cipher_id: CipherId,
    slot: &CreatedAttachment,
    api: &str,
    contents: Bytes,
) -> Result<(), VaultError> {
    let url = fresh_blob_url(client, cipher_id, slot, api, &slot.upload_url).await?;
    if contents.len() <= MAX_SINGLE_BLOB_BYTES {
        return put_blob(url, contents, true).await;
    }
    // The desktop service uses Put Block + Put Block List above the single-blob threshold.
    let block_id = STANDARD.encode([0; 16]);
    let mut block = url.clone();
    block
        .query_pairs_mut()
        .append_pair("comp", "block")
        .append_pair("blockid", &block_id);
    put_blob(block, contents, false).await?;
    let mut list = fresh_blob_url(client, cipher_id, slot, api, url.as_str()).await?;
    list.query_pairs_mut().append_pair("comp", "blocklist");
    let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList><Latest>{block_id}</Latest></BlockList>");
    put_blob(list, Bytes::from(xml), false).await
}

async fn fresh_blob_url(
    client: &Client,
    cipher_id: CipherId,
    slot: &CreatedAttachment,
    api: &str,
    signed: &str,
) -> Result<Url, VaultError> {
    let api = Url::parse(api).map_err(|_| VaultError::Internal)?;
    let mut url = Url::parse(signed).map_err(|_| VaultError::AttachmentUploadFailed)?;
    if url
        .query_pairs()
        .find(|(key, _)| key == "se")
        .and_then(|(_, expiry)| DateTime::parse_from_rfc3339(&expiry).ok())
        .is_some_and(|expiry| expiry <= Utc::now() + chrono::Duration::seconds(30))
    {
        let renewed = tokio::time::timeout(
            auth_request::TIMEOUT,
            client
                .vault()
                .attachments()
                .renew_file_upload_url(cipher_id, slot.attachment_id.clone()),
        )
        .await
        .map_err(|_| VaultError::AttachmentUploadFailed)?
        .map_err(|_| VaultError::AttachmentUploadFailed)?;
        url = Url::parse(&renewed).map_err(|_| VaultError::AttachmentUploadFailed)?;
    }
    if !allowed_url(&url, &api) {
        return Err(VaultError::AttachmentUploadFailed);
    }
    Ok(url)
}

async fn put_blob(url: Url, contents: Bytes, single: bool) -> Result<(), VaultError> {
    let version = url
        .query_pairs()
        .find(|(key, _)| key == "sv")
        .map(|(_, value)| value.into_owned());
    let mut request = blob_client()?
        .put(url)
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .header(
            "x-ms-date",
            Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        )
        .body(contents);
    if let Some(version) = version {
        request = request.header("x-ms-version", version);
    }
    if single {
        request = request.header("x-ms-blob-type", "BlockBlob");
    }
    let response = request
        .send()
        .await
        .map_err(|_| VaultError::AttachmentUploadFailed)?;
    if response.status() != StatusCode::CREATED {
        return Err(VaultError::AttachmentUploadFailed);
    }
    Ok(())
}

pub(crate) fn allowed_url(url: &Url, origin: &Url) -> bool {
    url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && url.origin() == origin.origin()))
}

fn blob_client() -> Result<&'static reqwest::Client, VaultError> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(auth_request::TIMEOUT)
                .redirect(Policy::custom(|attempt| {
                    if attempt.previous().len() < 6
                        && attempt
                            .previous()
                            .last()
                            .is_some_and(|previous| allowed_url(attempt.url(), previous))
                    {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or(VaultError::Internal)
}

async fn download_blob(api_url: &str, blob_url: &str) -> Result<Vec<u8>, VaultError> {
    let api = Url::parse(api_url).map_err(|_| VaultError::Internal)?;
    let url = Url::parse(blob_url).map_err(|_| VaultError::AttachmentDownloadFailed)?;
    if !allowed_url(&url, &api) {
        return Err(VaultError::AttachmentDownloadFailed);
    }

    // Presigned blob URLs do not receive the account client's Authorization or device headers.
    let mut response = blob_client()?
        .get(url)
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .send()
        .await
        .map_err(|_| VaultError::AttachmentDownloadFailed)?;
    if response.status() != StatusCode::OK {
        return Err(VaultError::AttachmentDownloadFailed);
    }
    let declared = response.content_length().unwrap_or(0);
    if declared > MAX_ENCRYPTED_BYTES as u64 {
        return Err(VaultError::AttachmentTooLarge);
    }
    let mut contents = Vec::new();
    contents
        .try_reserve_exact(declared as usize)
        .map_err(|_| VaultError::AttachmentTooLarge)?;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| VaultError::AttachmentDownloadFailed)?
    {
        if chunk.len() > MAX_ENCRYPTED_BYTES - contents.len() {
            return Err(VaultError::AttachmentTooLarge);
        }
        contents
            .try_reserve(chunk.len())
            .map_err(|_| VaultError::AttachmentTooLarge)?;
        contents.extend_from_slice(&chunk);
    }
    Ok(contents)
}

pub(crate) fn save_download(
    contents: &[u8],
    destination: &Path,
    replace_existing: bool,
    request_id: u64,
) -> Result<(), VaultError> {
    let directory = destination.parent().ok_or(VaultError::FileAccess)?;
    if !auth_request::is_current(request_id) {
        return Err(VaultError::Cancelled);
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".bytewarden-")
        .tempfile_in(directory)
        .map_err(|_| VaultError::FileAccess)?;
    for block in contents.chunks(1024 * 1024) {
        if !auth_request::is_current(request_id) {
            return Err(VaultError::Cancelled);
        }
        temporary
            .write_all(block)
            .map_err(|_| VaultError::FileAccess)?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| VaultError::FileAccess)?;
    if !auth_request::is_current(request_id) {
        return Err(VaultError::Cancelled);
    }
    let result = if replace_existing {
        temporary.persist(destination)
    } else {
        temporary.persist_noclobber(destination)
    };
    result.map_err(|_| VaultError::FileAccess)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_urls_allow_https_and_only_the_original_http_origin() {
        let local = Url::parse("http://127.0.0.1:48767/api").unwrap();
        let cloud = Url::parse("https://api.bitwarden.example/api").unwrap();
        let local_blob = Url::parse("http://127.0.0.1:48767/attachments/file").unwrap();
        assert!(allowed_url(&local_blob, &local));
        assert!(!allowed_url(&local_blob, &cloud));
        for rejected in [
            "http://127.0.0.1:8080/file",
            "http://example.com/file",
            "file:///vault/attachment",
            "https://user:password@example.com/file",
        ] {
            assert!(!allowed_url(&Url::parse(rejected).unwrap(), &local));
        }
        let cloud_blob = Url::parse("https://blob.example.com/file?signature=synthetic").unwrap();
        assert!(allowed_url(&cloud_blob, &cloud));
    }
}
