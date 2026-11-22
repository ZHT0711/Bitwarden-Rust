//! Bitwarden-format personal JSON import. Files are parsed and encrypted entirely in Rust before
//! the single SDK bulk operation; old IDs never select rows to modify on the destination.
use std::{num::NonZeroU32, path::Path};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use bitwarden_api_api::models::{
    CipherRequestModel, ImportCiphersRequestModel, Int32Int32KeyValuePair,
};
use bitwarden_crypto::{
    CompositeEncryptable, Decryptable, EncString, IdentifyKey, Kdf, KeyDecryptable, PinKey,
};
use bitwarden_vault::{
    should_use_blob_encryption, CipherType, EncryptMode, Fido2CredentialFullView, FolderView,
};
use bw_sdk_core::key_management::SymmetricKeySlotId;
use serde_json::{json, Map};
use zeroize::Zeroizing;

use super::*;
use crate::auth_request;

const MAX_FILE: usize = 32 * 1024 * 1024;
const MAX_ROWS: usize = 7_000;

pub(crate) struct ExpectedImport {
    cipher_names: HashSet<String>,
    cipher_keys: HashSet<String>,
    folder_names: HashSet<String>,
}

impl ExpectedImport {
    pub(crate) fn from_request(body: &ImportCiphersRequestModel) -> Result<Self, VaultError> {
        let mut expected = Self {
            cipher_names: HashSet::new(),
            cipher_keys: HashSet::new(),
            folder_names: HashSet::new(),
        };
        for cipher in body.ciphers.iter().flatten() {
            if let Some(name) = &cipher.name {
                if !expected.cipher_names.insert(name.clone()) {
                    return Err(VaultError::Internal);
                }
            } else {
                let key = cipher.key.as_ref().ok_or(VaultError::Internal)?;
                if !expected.cipher_keys.insert(key.clone()) {
                    return Err(VaultError::Internal);
                }
            }
        }
        for folder in body.folders.iter().flatten() {
            if !expected.folder_names.insert(folder.name.clone()) {
                return Err(VaultError::Internal);
            }
        }
        Ok(expected)
    }

    pub(crate) fn confirm(mut self, cache: &VaultCache) -> Result<VaultMutationResult, VaultError> {
        for cipher in cache
            .ciphers
            .snapshot()
            .map_err(|_| VaultError::WriteUncertain)?
        {
            if cipher.organization_id.is_some() || cipher.deleted_date.is_some() {
                continue;
            }
            if let Some(name) = cipher.name {
                self.cipher_names.remove(&name.to_string());
            } else if let Some(key) = cipher.key {
                self.cipher_keys.remove(&key.to_string());
            }
        }
        for folder in cache
            .folders
            .snapshot()
            .map_err(|_| VaultError::WriteUncertain)?
        {
            self.folder_names.remove(&folder.name.to_string());
        }
        if !self.cipher_names.is_empty()
            || !self.cipher_keys.is_empty()
            || !self.folder_names.is_empty()
        {
            return Err(VaultError::WriteUncertain);
        }
        Ok(VaultMutationResult {
            overview: cache.overview()?.clone(),
            id: None,
        })
    }
}

impl VaultCache {
    pub(crate) async fn submit_import(
        &mut self,
        client: &Client,
        body: ImportCiphersRequestModel,
        request_id: u64,
    ) -> Result<(), VaultError> {
        self.ensure_synced()?;
        if !auth_request::is_current(request_id) {
            return Err(VaultError::Cancelled);
        }
        self.write_started = true;
        let config = client.internal.get_api_configurations();
        tokio::time::timeout(
            auth_request::TIMEOUT,
            config
                .api_client
                .import_ciphers_api()
                .post_import(Some(body)),
        )
        .await
        .map_err(|_| self.uncertain())?
        .map_err(|error| match &error {
            bw_sdk_core::ApiError::Response(response)
                if matches!(response.status.as_u16(), 404 | 405 | 501) =>
            {
                self.write_started = false;
                VaultError::Unsupported
            }
            // A bulk write can partially commit even if a self-hosted server returns a 4xx.
            _ => self.uncertain(),
        })
    }
}

pub(crate) async fn prepare(
    cache: &VaultCache,
    client: &Client,
    source: &Path,
    password: &str,
    request_id: u64,
) -> Result<ImportCiphersRequestModel, VaultError> {
    cache.ensure_synced()?;
    let bytes = attachments::read_upload_file(source, MAX_FILE)
        .await
        .map_err(|_| VaultError::FileAccess)?;
    let input = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let mut doc: Value = if input.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{') {
        serde_json::from_slice(input).map_err(|_| VaultError::ImportFormat)?
    } else {
        transfer_csv::parse(input, MAX_ROWS)?
    };
    drop(bytes);
    let encrypted = doc
        .get("encrypted")
        .and_then(Value::as_bool)
        .ok_or(VaultError::ImportFormat)?;
    let portable = doc
        .get("passwordProtected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let store = client.internal.get_key_store();
    let mut ctx = store.context();
    if portable && !encrypted {
        return Err(VaultError::ImportFormat);
    }
    if portable {
        if password.is_empty() {
            return Err(VaultError::ImportPassword);
        }
        let salt = doc
            .get("salt")
            .and_then(Value::as_str)
            .filter(|v| v.len() <= 64)
            .filter(|v| {
                STANDARD
                    .decode(v)
                    .is_ok_and(|bytes| (16..=32).contains(&bytes.len()))
            })
            .ok_or(VaultError::ImportFormat)?;
        let key = PinKey::derive(password.as_bytes(), salt.as_bytes(), &kdf(&doc)?)
            .map_err(|_| VaultError::ImportPassword)?;
        let marker: String = enc(&doc, "encKeyValidation_DO_NOT_EDIT")?
            .decrypt_with_key(&key)
            .map_err(|_| VaultError::ImportPassword)?;
        Uuid::parse_str(&marker).map_err(|_| VaultError::ImportPassword)?;
        let clear = Zeroizing::new(
            enc(&doc, "data")?
                .decrypt_with_key(&key)
                .map_err(|_| VaultError::ImportPassword)?,
        );
        doc = serde_json::from_str(&clear).map_err(|_| VaultError::ImportFormat)?;
        if doc.get("encrypted").and_then(Value::as_bool) != Some(false) {
            return Err(VaultError::ImportFormat);
        }
    } else if encrypted {
        let marker: String = enc(&doc, "encKeyValidation_DO_NOT_EDIT")?
            .decrypt(&mut ctx, SymmetricKeySlotId::User)
            .map_err(|_| VaultError::ImportPassword)?;
        Uuid::parse_str(&marker).map_err(|_| VaultError::ImportPassword)?;
    }
    let account_encrypted = encrypted && !portable;
    let doc = doc.as_object_mut().ok_or(VaultError::ImportFormat)?;
    let items = rows(doc, "items")?;
    let folders = rows(doc, "folders")?;
    if items.is_empty() && folders.is_empty() || items.len() + folders.len() > MAX_ROWS {
        return Err(VaultError::ImportFormat);
    }
    let now = Utc::now();
    let mut index = HashMap::with_capacity(folders.len());
    let mut folder_models = Vec::with_capacity(folders.len());
    for (i, row) in folders.into_iter().enumerate() {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or(VaultError::ImportFormat)?;
        let name = if account_encrypted {
            enc(&row, "name")?
                .decrypt(&mut ctx, SymmetricKeySlotId::User)
                .map_err(|_| VaultError::DecryptionFailed)?
        } else {
            row.get("name")
                .and_then(Value::as_str)
                .ok_or(VaultError::ImportFormat)?
                .to_owned()
        };
        if name.trim().is_empty() || index.insert(id, i as i32).is_some() {
            return Err(VaultError::ImportFormat);
        }
        let view = FolderView {
            id: None,
            name,
            revision_date: now,
        };
        let folder: Folder = view
            .encrypt_composite(&mut ctx, view.key_identifier())
            .map_err(|_| VaultError::Internal)?;
        folder_models.push((&folder).into());
    }
    let user = client
        .internal
        .get_user_id()
        .ok_or(VaultError::NotAuthenticated)?;
    let encrypted_by_key_id = ctx
        .get_symmetric_key_id(SymmetricKeySlotId::User)
        .map(|id| id.to_string());
    let mut models = Vec::with_capacity(items.len());
    let mut relationships = Vec::new();
    for (i, row) in items.into_iter().enumerate() {
        if !auth_request::is_current(request_id) {
            return Err(VaultError::Cancelled);
        }
        let row = row.as_object().ok_or(VaultError::ImportFormat)?;
        if [
            "organizationId",
            "partialData",
            "attachments",
            "attachments2",
            "deletedDate",
        ]
        .into_iter()
        .any(|name| row.get(name).is_some_and(|v| !v.is_null()))
            || row
                .get("collectionIds")
                .is_some_and(|v| !v.is_null() && !v.as_array().is_some_and(Vec::is_empty))
        {
            return Err(VaultError::ImportFormat);
        }
        if let Some(folder) = row.get("folderId").filter(|value| !value.is_null()) {
            let id = folder
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or(VaultError::ImportFormat)?;
            let position = index.get(&id).ok_or(VaultError::ImportFormat)?;
            relationships.push(Int32Int32KeyValuePair {
                key: Some(i as i32),
                value: Some(*position),
            });
        }
        let ctype: CipherType = row
            .get("type")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or(VaultError::ImportFormat)?;
        if cache.export_policy.restricts(ctype) {
            return Err(VaultError::PermissionDenied);
        }
        let mut fields = allowed_fields(row, ctype, now);
        let view: CipherView = if account_encrypted {
            for field in ["key", "data"] {
                if let Some(value) = row.get(field) {
                    fields.insert(field.to_owned(), value.clone());
                }
            }
            let cipher: Cipher = parse(fields)?;
            cipher
                .decrypt(&mut ctx, SymmetricKeySlotId::User)
                .map_err(|_| VaultError::DecryptionFailed)?
        } else {
            if row.get("key").is_some_and(|v| !v.is_null())
                || row.get("data").is_some_and(|v| !v.is_null())
            {
                return Err(VaultError::ImportFormat);
            }
            let passkeys = match fields
                .get_mut("login")
                .and_then(Value::as_object_mut)
                .and_then(|login| {
                    // An older export may omit match, and URI checksums are regenerated by the SDK.
                    if let Some(uris) = login.get_mut("uris").and_then(Value::as_array_mut) {
                        for uri in uris {
                            if let Some(uri) = uri.as_object_mut() {
                                uri.remove("uriChecksum");
                            }
                        }
                    }
                    login.remove("fido2Credentials")
                }) {
                Some(keys) if !keys.is_null() => {
                    serde_json::from_value::<Vec<Fido2CredentialFullView>>(keys)
                        .map_err(|_| VaultError::ImportFormat)?
                }
                _ => vec![],
            };
            let mut view: CipherView = parse(fields)?;
            if !passkeys.is_empty() {
                view.set_new_fido2_credentials(passkeys)
                    .map_err(|_| VaultError::ImportFormat)?;
            }
            view
        };
        let mut view = view;
        if view.name.is_empty()
            || view.r#type != ctype
            || view.partial
            || !view.view_password
            || !has_content(&view)
        {
            return Err(VaultError::ImportFormat);
        }
        // The decrypted data of a supplied blob can carry state; never import server metadata.
        view.id = None;
        view.folder_id = None;
        view.organization_id = None;
        view.collection_ids.clear();
        view.attachments = None;
        view.attachment_decryption_failures = None;
        view.key = None;
        view.local_data = None;
        view.permissions = None;
        view.organization_use_totp = false;
        view.deleted_date = None;
        let mode = if should_use_blob_encryption(&ctx, None) {
            EncryptMode::Blob(view)
        } else {
            EncryptMode::Legacy(view)
        };
        let cipher: Cipher = mode
            .encrypt_composite(&mut ctx, SymmetricKeySlotId::User)
            .map_err(|_| VaultError::ImportFormat)?;
        let mut model =
            CipherRequestModel::try_from(cipher).map_err(|_| VaultError::ImportFormat)?;
        model.encrypted_for = Some(Uuid::from(user));
        model.encrypted_by_key_id = encrypted_by_key_id.clone();
        model.last_known_revision_date = None;
        models.push(model);
    }
    Ok(ImportCiphersRequestModel {
        ciphers: Some(models),
        folders: Some(folder_models),
        folder_relationships: Some(relationships),
    })
}

fn rows(doc: &mut Map<String, Value>, name: &str) -> Result<Vec<Value>, VaultError> {
    match doc.remove(name) {
        Some(Value::Array(rows)) => Ok(rows),
        _ => Err(VaultError::ImportFormat),
    }
}

fn enc(doc: &Value, name: &str) -> Result<EncString, VaultError> {
    doc.get(name)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or(VaultError::ImportFormat)
}

fn kdf(doc: &Value) -> Result<Kdf, VaultError> {
    let parameter = |name, max| {
        doc.get(name)
            .and_then(Value::as_u64)
            .filter(|v| *v <= max)
            .and_then(|v| NonZeroU32::new(v as u32))
            .ok_or(VaultError::ImportFormat)
    };
    match doc.get("kdfType").and_then(Value::as_u64) {
        Some(0) => Ok(Kdf::PBKDF2 {
            iterations: parameter("kdfIterations", 10_000_000)?,
        }),
        Some(1) => Ok(Kdf::Argon2id {
            iterations: parameter("kdfIterations", 10)?,
            memory: parameter("kdfMemory", 512)?,
            parallelism: parameter("kdfParallelism", 16)?,
        }),
        _ => Err(VaultError::ImportFormat),
    }
}

fn parse<T: serde::de::DeserializeOwned>(fields: Map<String, Value>) -> Result<T, VaultError> {
    serde_json::from_value(Value::Object(fields)).map_err(|_| VaultError::ImportFormat)
}

fn allowed_fields(
    row: &Map<String, Value>,
    kind: CipherType,
    now: chrono::DateTime<Utc>,
) -> Map<String, Value> {
    let content_name = match kind {
        CipherType::Login => "login",
        CipherType::SecureNote => "secureNote",
        CipherType::Card => "card",
        CipherType::Identity => "identity",
        CipherType::SshKey => "sshKey",
        CipherType::BankAccount => "bankAccount",
        CipherType::DriversLicense => "driversLicense",
        CipherType::Passport => "passport",
    };
    let mut fields = Map::new();
    for name in [
        "name",
        "notes",
        "fields",
        "passwordHistory",
        "archivedDate",
        content_name,
    ] {
        if let Some(value) = row.get(name) {
            fields.insert(name.to_owned(), value.clone());
        }
    }
    fields.extend(
        [
            ("type", json!(kind)),
            ("collectionIds", json!([])),
            ("edit", json!(true)),
            ("viewPassword", json!(true)),
            ("organizationUseTotp", json!(false)),
            ("revisionDate", json!(now)),
            (
                "creationDate",
                row.get("creationDate")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!(now)),
            ),
            (
                "favorite",
                row.get("favorite")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(json!(false)),
            ),
            (
                "reprompt",
                row.get("reprompt")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value)),
    );
    fields
}

fn has_content(view: &CipherView) -> bool {
    match view.r#type {
        CipherType::Login => view.login.is_some(),
        CipherType::SecureNote => view.secure_note.is_some(),
        CipherType::Card => view.card.is_some(),
        CipherType::Identity => view.identity.is_some(),
        CipherType::SshKey => view.ssh_key.is_some(),
        CipherType::BankAccount => view.bank_account.is_some(),
        CipherType::DriversLicense => view.drivers_license.is_some(),
        CipherType::Passport => view.passport.is_some(),
    }
}
