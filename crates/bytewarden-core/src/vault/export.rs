//! Official personal vault JSON shape. Deliberately project only portable fields, not the
//! SDK's cache structs: decrypted views also contain cipher/attachment keys and permissions.
use std::path::Path;

use bitwarden_api_api::models::PolicyType;
use bitwarden_crypto::{
    generate_random_bytes, EncString, Kdf, KeyEncryptable, PinKey, PrimitiveEncryptable,
};
use bitwarden_vault::{CipherType, Fido2CredentialFullView};
use bw_sdk_core::key_management::SymmetricKeySlotId;
use serde_json::{json, Map};
use zeroize::Zeroizing;

use super::*;
use crate::auth_request;

#[derive(Default)]
pub(crate) struct ExportPolicy {
    denied: bool,
    unknown_restrictions: bool,
    restricted: HashSet<u8>,
}

impl ExportPolicy {
    pub(super) fn from_sync(sync: &SyncResponseModel) -> Self {
        let mut result = Self::default();
        // Prefer policiesNew (accepted and confirmed memberships), like the official client.
        for policy in sync
            .policies_new
            .as_ref()
            .or(sync.policies.as_ref())
            .into_iter()
            .flatten()
        {
            if policy.enabled == Some(false) {
                continue;
            }
            match policy.r#type {
                Some(PolicyType::DisablePersonalVaultExport) => result.denied = true,
                Some(PolicyType::RestrictedItemTypesPolicy) => match policy.data.as_ref() {
                    None | Some(Value::Null) => {
                        result.restricted.insert(CipherType::Card as u8);
                    }
                    Some(Value::Array(types)) => {
                        for value in types {
                            if let Some(kind) = value.as_u64().filter(|v| (1..=8).contains(v)) {
                                result.restricted.insert(kind as u8);
                            } else {
                                result.unknown_restrictions = true;
                            }
                        }
                    }
                    _ => result.unknown_restrictions = true,
                },
                _ => (),
            }
        }
        result
    }

    pub(super) fn restricts(&self, kind: CipherType) -> bool {
        self.unknown_restrictions || self.restricted.contains(&(kind as u8))
    }
}

pub(crate) enum Format {
    Plain,
    Csv,
    Account,
    Password(Zeroizing<String>),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    items: usize,
    folders: usize,
    skipped_restricted: usize,
    omitted_attachments: usize,
    omitted_unsupported: usize,
}

impl VaultCache {
    pub(crate) async fn export_personal(
        &self,
        client: &Client,
        format: Format,
        path: &Path,
        request_id: u64,
    ) -> Result<Report, VaultError> {
        self.ensure_synced()?;
        if self.export_policy.denied || self.export_policy.unknown_restrictions {
            return Err(VaultError::PermissionDenied);
        }
        // A malformed sync record may be personal and must not silently disappear from a backup.
        if self.parse_failures != 0 {
            return Err(VaultError::InvalidResponse);
        }
        let account_key = matches!(format, Format::Account);
        let csv = matches!(format, Format::Csv);
        let store = client.internal.get_key_store();
        let mut folders = self.folders.snapshot().map_err(|_| VaultError::Internal)?;
        folders.sort_unstable_by_key(|f| f.id.map(Uuid::from));
        let folders = folders
            .into_iter()
            .map(|folder| {
                if account_key {
                    Ok(json!({ "id": folder.id, "name": folder.name.to_string() }))
                } else {
                    use bitwarden_crypto::Decryptable;
                    let mut ctx = store.context();
                    let name: String = folder
                        .name
                        .decrypt(&mut ctx, SymmetricKeySlotId::User)
                        .map_err(|_| VaultError::DecryptionFailed)?;
                    Ok(json!({ "id": folder.id, "name": name }))
                }
            })
            .collect::<Result<Vec<_>, VaultError>>()?;

        let mut ciphers = self.ciphers.snapshot().map_err(|_| VaultError::Internal)?;
        ciphers.sort_unstable_by_key(|c| c.id.map(Uuid::from));
        let mut items = Vec::new();
        let mut skipped_restricted = 0;
        let mut omitted_attachments = 0;
        let mut omitted_unsupported = 0;
        for cipher in ciphers {
            if !auth_request::is_current(request_id) {
                return Err(VaultError::Cancelled);
            }
            if cipher.organization_id.is_some() || cipher.deleted_date.is_some() {
                continue;
            }
            if cipher.partial_data.is_some()
                || !cipher.view_password
                || self.export_policy.restricts(cipher.r#type)
            {
                skipped_restricted += 1;
                continue;
            }
            omitted_attachments += cipher.attachments.as_ref().map_or(0, Vec::len);
            if csv && !matches!(cipher.r#type, CipherType::Login | CipherType::SecureNote) {
                omitted_unsupported += 1;
                continue;
            }
            let mut item = common(&cipher);
            if account_key {
                encrypted_fields(&cipher, &mut item)?;
            } else {
                let view = client
                    .vault()
                    .ciphers()
                    .decrypt(cipher)
                    .await
                    .map_err(|_| VaultError::DecryptionFailed)?;
                if view.partial || !view.view_password {
                    return Err(VaultError::PermissionDenied);
                }
                clear_fields(view, &mut item)?;
            }
            items.push(Value::Object(item));
        }
        let report = Report {
            items: items.len(),
            folders: if csv {
                transfer_csv::represented_folders(&folders, &items)
            } else {
                folders.len()
            },
            skipped_restricted,
            omitted_attachments,
            omitted_unsupported,
        };
        if csv {
            let output = Zeroizing::new(transfer_csv::emit(folders, items)?);
            attachments::save_download(&output, path, false, request_id)?;
            return Ok(report);
        }
        let mut doc = json!({ "encrypted": account_key, "folders": folders, "items": items });
        if account_key {
            let mut ctx = store.context();
            let validation: EncString = Uuid::new_v4()
                .to_string()
                .encrypt(&mut ctx, SymmetricKeySlotId::User)
                .map_err(|_| VaultError::Internal)?;
            doc["encKeyValidation_DO_NOT_EDIT"] = Value::String(validation.to_string());
        }
        let bytes =
            Zeroizing::new(serde_json::to_vec_pretty(&doc).map_err(|_| VaultError::Internal)?);
        drop(doc);
        match format {
            Format::Password(password) => {
                let kdf = client
                    .internal
                    .get_kdf()
                    .await
                    .map_err(|_| VaultError::NotAuthenticated)?;
                let clear = std::str::from_utf8(&bytes).map_err(|_| VaultError::Internal)?;
                let encrypted = password_envelope(clear, &password, kdf)?;
                attachments::save_download(encrypted.as_bytes(), path, false, request_id)?;
            }
            _ => attachments::save_download(&bytes, path, false, request_id)?,
        }
        Ok(report)
    }
}

fn common(cipher: &Cipher) -> Map<String, Value> {
    // Keep archive/re-prompt/history compatibility with the official desktop exporter.
    json!({
        "id": cipher.id, "folderId": cipher.folder_id, "organizationId": null,
        "collectionIds": null, "type": cipher.r#type, "favorite": cipher.favorite,
        "reprompt": cipher.reprompt, "creationDate": cipher.creation_date,
        "revisionDate": cipher.revision_date, "deletedDate": null, "archivedDate": cipher.archived_date
    }).as_object_mut().map(std::mem::take).expect("an object literal")
}

fn encrypted_fields(cipher: &Cipher, item: &mut Map<String, Value>) -> Result<(), VaultError> {
    if let Some(key) = &cipher.key {
        item.insert("key".to_owned(), json!(key));
    }
    // Blob ciphers have no per-field name; legacy ciphers may also carry a redundant data marker.
    if cipher.is_blob_encrypted() {
        let data = cipher.data.as_ref().ok_or(VaultError::InvalidResponse)?;
        item.insert("data".to_owned(), json!(data));
        return Ok(());
    }
    let name = cipher.name.as_ref().ok_or(VaultError::InvalidResponse)?;
    item.insert("name".to_owned(), json!(name));
    item.insert("notes".to_owned(), json!(cipher.notes));
    item.insert("fields".to_owned(), json!(cipher.fields));
    item.insert("passwordHistory".to_owned(), json!(cipher.password_history));
    macro_rules! project {
        ($key:literal, $value:expr) => {{
            let value = $value.as_ref().ok_or(VaultError::InvalidResponse)?;
            item.insert($key.to_owned(), json!(value));
        }};
    }
    match cipher.r#type {
        CipherType::Login => project!("login", cipher.login),
        CipherType::SecureNote => project!("secureNote", cipher.secure_note),
        CipherType::Card => project!("card", cipher.card),
        CipherType::Identity => project!("identity", cipher.identity),
        CipherType::SshKey => project!("sshKey", cipher.ssh_key),
        CipherType::BankAccount => project!("bankAccount", cipher.bank_account),
        CipherType::DriversLicense => project!("driversLicense", cipher.drivers_license),
        CipherType::Passport => project!("passport", cipher.passport),
    };
    Ok(())
}

fn clear_fields(view: CipherView, item: &mut Map<String, Value>) -> Result<(), VaultError> {
    item.insert("name".to_owned(), json!(view.name));
    item.insert("notes".to_owned(), json!(view.notes));
    item.insert("fields".to_owned(), json!(view.fields));
    item.insert("passwordHistory".to_owned(), json!(view.password_history));
    macro_rules! project {
        ($key:literal, $value:expr) => {{
            let value = $value.ok_or(VaultError::DecryptionFailed)?;
            item.insert($key.to_owned(), json!(value));
        }};
    }
    match view.r#type {
        CipherType::Login => {
            let login = view.login.ok_or(VaultError::DecryptionFailed)?;
            let uris = login
                .uris
                .unwrap_or_default()
                .into_iter()
                .map(|u| json!({ "uri": u.uri, "match": u.r#match }))
                .collect::<Vec<_>>();
            let passkeys = login
                .fido2_credentials
                .unwrap_or_default()
                .into_iter()
                .map(Fido2CredentialFullView::from)
                .collect::<Vec<_>>();
            item.insert("login".to_owned(), json!({"username": login.username, "password": login.password,
                "uris": uris, "totp": login.totp, "fido2Credentials": passkeys,
                "passwordRevisionDate": login.password_revision_date, "autofillOnPageLoad": login.autofill_on_page_load,
                "httpAuthenticationEnabled": login.http_authentication_enabled}));
        }
        CipherType::SecureNote => project!("secureNote", view.secure_note),
        CipherType::Card => project!("card", view.card),
        CipherType::Identity => project!("identity", view.identity),
        CipherType::SshKey => project!("sshKey", view.ssh_key),
        CipherType::BankAccount => project!("bankAccount", view.bank_account),
        CipherType::DriversLicense => project!("driversLicense", view.drivers_license),
        CipherType::Passport => project!("passport", view.passport),
    };
    Ok(())
}

fn password_envelope(clear: &str, password: &str, kdf: Kdf) -> Result<String, VaultError> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let salt = STANDARD.encode(generate_random_bytes::<[u8; 16]>());
    let key = PinKey::derive(password.as_bytes(), salt.as_bytes(), &kdf)
        .map_err(|_| VaultError::Internal)?;
    let mut doc = json!({ "encrypted": true, "passwordProtected": true, "salt": salt,
        "encKeyValidation_DO_NOT_EDIT": Uuid::new_v4().to_string().encrypt_with_key(&key).map_err(|_| VaultError::Internal)?.to_string(),
        "data": clear.to_owned().encrypt_with_key(&key).map_err(|_| VaultError::Internal)?.to_string() });
    match kdf {
        Kdf::PBKDF2 { iterations } => {
            doc["kdfType"] = json!(0);
            doc["kdfIterations"] = json!(iterations);
        }
        Kdf::Argon2id {
            iterations,
            memory,
            parallelism,
        } => {
            doc["kdfType"] = json!(1);
            doc["kdfIterations"] = json!(iterations);
            doc["kdfMemory"] = json!(memory);
            doc["kdfParallelism"] = json!(parallelism);
        }
    }
    serde_json::to_string_pretty(&doc).map_err(|_| VaultError::Internal)
}
