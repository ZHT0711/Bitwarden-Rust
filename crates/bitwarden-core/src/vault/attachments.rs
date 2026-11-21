use std::{io::Write, path::Path, sync::OnceLock, time::Duration};

use bitwarden_vault::{AttachmentView, CipherView, VaultClientExt};
use bw_sdk_core::Client;
use reqwest::{redirect::Policy, StatusCode};
use serde::Serialize;
use url::Url;
use zeroize::Zeroizing;

use super::{VaultCache, VaultError};
use crate::auth_request;

// Bitwarden Desktop allows 500 MiB of plaintext. Allow for encrypted framing and padding.
const MAX_ENCRYPTED_BYTES: usize = 501 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentSummary {
    id: String,
    name: String,
    size: Option<u64>,
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
    let mut failures = view
        .attachment_decryption_failures
        .as_ref()
        .map_or(0, Vec::len);
    let mut files = Vec::new();
    for attachment in view.attachments.iter().flatten() {
        if !displayable(attachment) {
            failures += 1;
            continue;
        }
        files.push(AttachmentSummary {
            id: attachment.id.clone().unwrap_or_default(),
            name: attachment.file_name.clone().unwrap_or_default(),
            size: attachment.size.as_ref().and_then(|size| size.parse().ok()),
        });
    }
    (files, failures)
}

impl VaultCache {
    pub(crate) async fn decrypt_attachment(
        &self,
        client: &Client,
        api_url: &str,
        item_id: &str,
        revision: &str,
        attachment_id: &str,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        self.ensure_synced()?;
        if !valid_id(attachment_id) {
            return Err(VaultError::Validation);
        }
        let cipher = self
            .ciphers
            .find(item_id)
            .map_err(|_| VaultError::Internal)?
            .ok_or(VaultError::NotFound)?;
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
}

fn allowed_url(url: &Url, origin: &Url) -> bool {
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
        .prefix(".bitwarden-")
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
