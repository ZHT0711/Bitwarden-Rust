use std::collections::HashMap;

use bitwarden_state::repository::Repository;
use bitwarden_vault::{
    CipherCreateRequest, CipherEditRequest, CipherPartialEditRequest, CipherRepromptType,
    CipherViewType, FieldView, LoginUriView,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VaultEditorData {
    pub(super) id: Option<String>,
    pub(super) name: String,
    pub(super) item_type: u8,
    pub(super) folder_id: Option<String>,
    pub(super) favorite: bool,
    pub(super) reprompt: bool,
    pub(super) revision_date: Option<String>,
    pub(super) notes: Option<String>,
    pub(super) values: HashMap<String, String>,
    pub(super) uris: Vec<VaultUri>,
    pub(super) custom_fields: Vec<VaultCustomField>,
    pub(super) can_edit: bool,
    pub(super) can_delete: bool,
    pub(super) can_restore: bool,
    pub(super) is_deleted: bool,
    pub(super) is_archived: bool,
    pub(super) can_archive: bool,
    pub(super) edit_blocked_reason: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct VaultUri {
    pub(super) uri: Option<String>,
    pub(super) r#match: Option<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct VaultCustomField {
    pub(super) name: Option<String>,
    pub(super) value: Option<String>,
    pub(super) r#type: u8,
    pub(super) linked_id: Option<u32>,
}

#[derive(Serialize)]
pub(crate) struct VaultMutationResult {
    pub(super) overview: VaultOverview,
    pub(super) id: Option<String>,
}

impl VaultCache {
    fn ensure_synced(&self) -> Result<(), VaultError> {
        if self.overview.is_none() || self.write_uncertain {
            Err(VaultError::NotSynced)
        } else {
            Ok(())
        }
    }

    fn find_cipher(&self, id: &str) -> Result<Cipher, VaultError> {
        self.ciphers
            .find(id)
            .map_err(|_| VaultError::Internal)?
            .ok_or(VaultError::NotFound)
    }

    async fn check_reprompt(
        &self,
        client: &Client,
        cipher: &Cipher,
        password: &str,
    ) -> Result<(), VaultError> {
        if cipher.reprompt as u8 == 0 {
            return Ok(());
        }
        if password.is_empty() {
            return Err(VaultError::PasswordRequired);
        }
        let key = self
            .encrypted_user_key
            .as_ref()
            .ok_or(VaultError::PasswordRequired)?;
        client
            .auth()
            .validate_password_user_key(password.to_owned(), key.clone())
            .await
            .map_err(|_| VaultError::WrongPassword)?;
        Ok(())
    }

    pub(crate) async fn editor(
        &self,
        client: &Client,
        id: &str,
        password: &str,
    ) -> Result<VaultEditorData, VaultError> {
        self.ensure_synced()?;
        let cipher = self.find_cipher(id)?;
        self.check_reprompt(client, &cipher, password).await?;
        let view = client
            .vault()
            .ciphers()
            .decrypt(cipher.clone())
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        let allowed = view.view_password && !view.partial && cipher.partial_data.is_none();
        let damaged_attachments = view
            .attachment_decryption_failures
            .as_ref()
            .is_some_and(|items| !items.is_empty());
        let values = if allowed {
            project_values(&view)?
        } else {
            HashMap::new()
        };
        let uris = if allowed {
            view.login
                .as_ref()
                .and_then(|login| login.uris.as_ref())
                .into_iter()
                .flatten()
                .map(|uri| VaultUri {
                    uri: uri.uri.clone(),
                    r#match: uri.r#match.map(|mode| mode as u8),
                })
                .collect()
        } else {
            vec![]
        };
        let custom_fields = if allowed {
            view.fields
                .as_ref()
                .into_iter()
                .flatten()
                .map(|field| {
                    let linked_id = serde_json::to_value(field)
                        .ok()
                        .and_then(|value| value.get("linkedId").and_then(Value::as_u64))
                        .and_then(|value| u32::try_from(value).ok());
                    VaultCustomField {
                        name: field.name.clone(),
                        value: field.value.clone(),
                        r#type: field.r#type as u8,
                        linked_id,
                    }
                })
                .collect()
        } else {
            vec![]
        };
        Ok(VaultEditorData {
            id: Some(id.to_owned()),
            name: view.name,
            item_type: view.r#type as u8,
            folder_id: view.folder_id.map(|id| id.to_string()),
            favorite: view.favorite,
            reprompt: view.reprompt as u8 != 0,
            revision_date: Some(view.revision_date.to_rfc3339()),
            notes: if allowed { view.notes } else { None },
            values,
            uris,
            custom_fields,
            can_edit: allowed
                && cipher.edit
                && cipher.deleted_date.is_none()
                && !damaged_attachments,
            can_delete: cipher.partial_data.is_none()
                && cipher
                    .permissions
                    .as_ref()
                    .map_or(cipher.edit, |p| p.delete),
            can_restore: cipher.partial_data.is_none()
                && cipher
                    .permissions
                    .as_ref()
                    .map_or(cipher.edit, |p| p.restore),
            is_deleted: cipher.deleted_date.is_some(),
            is_archived: cipher.archived_date.is_some(),
            can_archive: self.can_archive,
            edit_blocked_reason: if damaged_attachments {
                Some("attachmentUnavailable".to_owned())
            } else {
                None
            },
        })
    }

    fn validate_input(&self, input: &VaultEditorData) -> Result<(), VaultError> {
        if input.name.trim().is_empty()
            || input
                .values
                .keys()
                .any(|key| !schema(input.item_type).contains(&key.as_str()))
            || !(1..=8).contains(&input.item_type)
        {
            return Err(VaultError::Validation);
        }
        if let Some(folder) = input.folder_id.as_deref().filter(|id| !id.is_empty()) {
            if !self
                .overview()?
                .folders
                .iter()
                .any(|candidate| candidate.id == folder)
            {
                return Err(VaultError::Validation);
            }
        }
        if input.item_type != 1 && !input.uris.is_empty() {
            return Err(VaultError::Validation);
        }
        Ok(())
    }

    fn check_revision(cipher: &Cipher, revision: Option<&str>) -> Result<(), VaultError> {
        let revision = revision
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .ok_or(VaultError::Validation)?;
        if revision != cipher.revision_date {
            return Err(VaultError::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn create(
        &mut self,
        client: &Client,
        input: VaultEditorData,
    ) -> Result<VaultMutationResult, VaultError> {
        self.write_started = false;
        self.ensure_synced()?;
        self.validate_input(&input)?;
        if input.id.is_some() || input.revision_date.is_some() {
            return Err(VaultError::Validation);
        }
        let mut view = new_view(input.item_type)?;
        apply_input(&mut view, &input)?;
        let request = CipherCreateRequest {
            organization_id: None,
            collection_ids: vec![],
            folder_id: view.folder_id,
            name: view.name.clone(),
            notes: view.notes.clone(),
            favorite: view.favorite,
            reprompt: view.reprompt,
            r#type: view_type(&view)?,
            fields: view.fields.unwrap_or_default(),
            archived_date: None,
        };
        self.activate(client);
        self.write_started = true;
        let response = client
            .vault()
            .ciphers()
            .create(request)
            .await
            .map_err(|error| self.classify_create_error(error))?;
        self.finish_write(client, response.id.map(|id| id.to_string()))
            .await
    }

    pub(crate) async fn update(
        &mut self,
        client: &Client,
        input: VaultEditorData,
        password: &str,
    ) -> Result<VaultMutationResult, VaultError> {
        self.write_started = false;
        self.ensure_synced()?;
        self.validate_input(&input)?;
        let id = input.id.as_deref().ok_or(VaultError::Validation)?;
        let cipher = self.find_cipher(id)?;
        Self::check_revision(&cipher, input.revision_date.as_deref())?;
        if !cipher.edit
            || !cipher.view_password
            || cipher.partial_data.is_some()
            || cipher.deleted_date.is_some()
        {
            return Err(VaultError::PermissionDenied);
        }
        if input.item_type != cipher.r#type as u8 {
            return Err(VaultError::Validation);
        }
        self.check_reprompt(client, &cipher, password).await?;
        let mut view = client
            .vault()
            .ciphers()
            .decrypt(cipher)
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        if view.partial {
            return Err(VaultError::PermissionDenied);
        }
        if view
            .attachment_decryption_failures
            .as_ref()
            .is_some_and(|items| !items.is_empty())
        {
            return Err(VaultError::AttachmentUnavailable);
        }
        apply_input(&mut view, &input)?;
        // Convert the complete native original; SDK edit maintains keys, attachments and history.
        let request = CipherEditRequest::try_from(view).map_err(|_| VaultError::Validation)?;
        self.activate(client);
        self.write_started = true;
        client
            .vault()
            .ciphers()
            .edit(request)
            .await
            .map_err(|error| self.classify_edit_error(error))?;
        self.finish_write(client, Some(id.to_owned())).await
    }

    pub(crate) async fn action(
        &mut self,
        client: &Client,
        id: &str,
        action: &str,
        revision: &str,
        password: &str,
    ) -> Result<VaultMutationResult, VaultError> {
        self.write_started = false;
        self.ensure_synced()?;
        let cipher = self.find_cipher(id)?;
        Self::check_revision(&cipher, Some(revision))?;
        let cipher_id = cipher.id.ok_or(VaultError::NotFound)?;
        let deleted = cipher.deleted_date.is_some();
        let restricted = cipher.partial_data.is_some();
        match action {
            "favorite" | "unfavorite" if !deleted => {}
            "archive" if !deleted && cipher.archived_date.is_none() => {
                if !self.can_archive {
                    return Err(VaultError::PremiumRequired);
                }
            }
            "unarchive" if !deleted && cipher.archived_date.is_some() => {}
            "trash"
                if !deleted
                    && !restricted
                    && cipher
                        .permissions
                        .as_ref()
                        .map_or(cipher.edit, |p| p.delete) => {}
            "delete"
                if deleted
                    && !restricted
                    && cipher
                        .permissions
                        .as_ref()
                        .map_or(cipher.edit, |p| p.delete) => {}
            "restore"
                if deleted
                    && !restricted
                    && cipher
                        .permissions
                        .as_ref()
                        .map_or(cipher.edit, |p| p.restore) => {}
            "favorite" | "unfavorite" | "archive" | "unarchive" | "trash" | "delete"
            | "restore" => return Err(VaultError::PermissionDenied),
            _ => return Err(VaultError::Validation),
        }
        if action != "favorite" && action != "unfavorite" {
            self.check_reprompt(client, &cipher, password).await?;
        }
        self.activate(client);
        self.write_started = true;
        match action {
            "favorite" | "unfavorite" => {
                let response = client
                    .vault()
                    .ciphers()
                    .edit_partial(CipherPartialEditRequest {
                        id: cipher_id,
                        folder_id: cipher.folder_id,
                        favorite: action == "favorite",
                    })
                    .await;
                if let Err(error) = response {
                    if restricted {
                        self.ciphers
                            .set(cipher_id, cipher.clone())
                            .await
                            .map_err(|_| self.uncertain())?;
                    }
                    return Err(self.classify_edit_error(error));
                }
                self.secure_metadata_cache(&cipher).await?;
            }
            "trash" => client
                .vault()
                .ciphers()
                .soft_delete(cipher_id)
                .await
                .map_err(|error| self.classify_delete_error(error))?,
            "delete" => client
                .vault()
                .ciphers()
                .delete(cipher_id)
                .await
                .map_err(|error| self.classify_delete_error(error))?,
            "restore" => {
                client
                    .vault()
                    .ciphers()
                    .restore(cipher_id)
                    .await
                    .map_err(|error| self.classify_restore_error(error))?;
                self.secure_metadata_cache(&cipher).await?;
            }
            "archive" | "unarchive" => {
                let config = client.internal.get_api_configurations();
                let api = config.api_client.ciphers_api();
                let response = if action == "archive" {
                    api.put_archive(cipher_id.into()).await
                } else {
                    api.put_unarchive(cipher_id.into()).await
                }
                .map_err(|error| self.classify_api_error(error))?;
                let (had_edit, had_view_password, had_reprompt, had_permissions) = (
                    response.edit.is_some(),
                    response.view_password.is_some(),
                    response.reprompt.is_some(),
                    response.permissions.is_some(),
                );
                let (had_key, had_org, had_favorite, had_folder, had_org_totp) = (
                    response.key.is_some(),
                    response.organization_id.is_some(),
                    response.favorite.is_some(),
                    response.folder_id.is_some(),
                    response.organization_use_totp.is_some(),
                );
                let mut updated = Cipher::try_from(response).map_err(|_| self.uncertain())?;
                if updated.id != cipher.id {
                    return Err(self.uncertain());
                }
                // Metadata writes must not persist a possibly lease-scoped full response over a
                // gated original. The SDK's full-edit path enforces the same storage invariant.
                if restricted && updated.partial_data.is_none() {
                    return Err(self.uncertain());
                }
                if !had_edit {
                    updated.edit = cipher.edit;
                }
                if !had_view_password {
                    updated.view_password = cipher.view_password;
                }
                if !had_reprompt {
                    updated.reprompt = cipher.reprompt;
                }
                if !had_permissions {
                    updated.permissions = cipher.permissions;
                }
                if !had_key {
                    updated.key = cipher.key.clone();
                }
                if !had_org {
                    updated.organization_id = cipher.organization_id;
                }
                if updated.organization_id != cipher.organization_id {
                    return Err(self.uncertain());
                }
                if !had_favorite {
                    updated.favorite = cipher.favorite;
                }
                if !had_folder {
                    updated.folder_id = cipher.folder_id;
                }
                if !had_org_totp {
                    updated.organization_use_totp = cipher.organization_use_totp;
                }
                // These two local fields are absent from CipherResponseModel; all server restriction
                // metadata remains authoritative, including a newly partial/gated response.
                updated.collection_ids = cipher.collection_ids.clone();
                updated.local_data = cipher.local_data.clone();
                let updated = self.preserve_metadata_restrictions(updated, &cipher)?;
                self.ciphers
                    .set(cipher_id, updated)
                    .await
                    .map_err(|_| self.uncertain())?;
            }
            _ => unreachable!(),
        }
        self.finish_write(
            client,
            if action == "delete" {
                None
            } else {
                Some(id.to_owned())
            },
        )
        .await
    }

    fn uncertain(&mut self) -> VaultError {
        self.write_uncertain = true;
        VaultError::WriteUncertain
    }

    async fn secure_metadata_cache(&mut self, previous: &Cipher) -> Result<(), VaultError> {
        let id = previous.id.ok_or_else(|| self.uncertain())?;
        let updated = self
            .find_cipher(&id.to_string())
            .map_err(|_| self.uncertain())?;
        match self.preserve_metadata_restrictions(updated, previous) {
            Ok(updated) => self
                .ciphers
                .set(id, updated)
                .await
                .map_err(|_| self.uncertain()),
            Err(error) => {
                // High-level SDK metadata writes may already have installed their response.
                // Never retain a lease-scoped full return over a gated stored original.
                self.ciphers
                    .set(id, previous.clone())
                    .await
                    .map_err(|_| self.uncertain())?;
                Err(error)
            }
        }
    }

    fn preserve_metadata_restrictions(
        &mut self,
        mut updated: Cipher,
        previous: &Cipher,
    ) -> Result<Cipher, VaultError> {
        if updated.id != previous.id
            || updated.organization_id != previous.organization_id
            || (previous.partial_data.is_some() && updated.partial_data.is_none())
        {
            return Err(self.uncertain());
        }
        // User-metadata actions cannot intentionally relax secret-access or reprompt controls.
        // A subsequent complete sync can authoritatively apply changed account permissions.
        updated.view_password &= previous.view_password;
        updated.edit &= previous.edit;
        if previous.reprompt as u8 != 0 {
            updated.reprompt = previous.reprompt;
        }
        if let Some(permissions) = updated.permissions.as_mut() {
            permissions.delete &= previous
                .permissions
                .as_ref()
                .map_or(previous.edit, |p| p.delete);
            permissions.restore &= previous
                .permissions
                .as_ref()
                .map_or(previous.edit, |p| p.restore);
        } else {
            updated.permissions = previous.permissions;
        }
        Ok(updated)
    }

    fn classify_api_error(&mut self, error: bw_sdk_core::ApiError) -> VaultError {
        if let bw_sdk_core::ApiError::Response(response) = error {
            let status = response.status.as_u16();
            let out_of_date = response
                .message
                .to_ascii_lowercase()
                .contains("the item cannot be saved because it is out of date");
            let error = match status {
                400 if out_of_date => VaultError::Conflict,
                409 => VaultError::Conflict,
                400 | 422 => VaultError::Validation,
                401 => VaultError::NotAuthenticated,
                402 => VaultError::PremiumRequired,
                403 => VaultError::PermissionDenied,
                404 => VaultError::NotFound,
                429 => VaultError::Network,
                _ => return self.uncertain(),
            };
            self.write_started = false;
            if matches!(error, VaultError::Conflict | VaultError::NotFound) {
                self.write_uncertain = true;
            }
            return error;
        }
        self.uncertain()
    }

    fn classify_create_error(&mut self, error: bitwarden_vault::CreateCipherError) -> VaultError {
        match error {
            bitwarden_vault::CreateCipherError::Api(error) => self.classify_api_error(error),
            bitwarden_vault::CreateCipherError::NotAuthenticated(_) => {
                self.write_started = false;
                VaultError::NotAuthenticated
            }
            _ => self.uncertain(),
        }
    }
    fn classify_edit_error(&mut self, error: bitwarden_vault::EditCipherError) -> VaultError {
        match error {
            bitwarden_vault::EditCipherError::Api(error) => self.classify_api_error(error),
            bitwarden_vault::EditCipherError::NotAuthenticated(_) => {
                self.write_started = false;
                VaultError::NotAuthenticated
            }
            _ => self.uncertain(),
        }
    }
    fn classify_delete_error(&mut self, error: bitwarden_vault::DeleteCipherError) -> VaultError {
        match error {
            bitwarden_vault::DeleteCipherError::Api(error) => self.classify_api_error(error),
            _ => self.uncertain(),
        }
    }
    fn classify_restore_error(&mut self, error: bitwarden_vault::RestoreCipherError) -> VaultError {
        match error {
            bitwarden_vault::RestoreCipherError::Api(error) => self.classify_api_error(error),
            _ => self.uncertain(),
        }
    }

    async fn finish_write(
        &mut self,
        client: &Client,
        id: Option<String>,
    ) -> Result<VaultMutationResult, VaultError> {
        let ciphers = self.ciphers.snapshot().map_err(|_| self.uncertain())?;
        let result = client
            .vault()
            .ciphers()
            .decrypt_list_with_failures(ciphers.values().cloned().collect::<Vec<_>>().into())
            .await
            .map_err(|_| self.uncertain())?;
        let failures = result.failures.len();
        let mut items = Vec::new();
        for view in result.successes {
            let Some(id) = view.id.map(|id| id.to_string()) else {
                continue;
            };
            items.push(VaultItem {
                id: id.clone(),
                name: view.name,
                subtitle: view.subtitle,
                item_type: ciphers[&id].r#type as u8,
                favorite: view.favorite,
                folder_id: view.folder_id.map(|id| id.to_string()),
                organization_id: view.organization_id.map(|id| id.to_string()),
                deleted: view.deleted_date.is_some(),
                archived: view.archived_date.is_some(),
                reprompt: view.reprompt as u8 != 0,
                restricted: view.partial,
                revision_date: view.revision_date.to_rfc3339(),
            });
        }
        items.sort_by_cached_key(|item| item.name.to_lowercase());
        if id
            .as_ref()
            .is_some_and(|id| !items.iter().any(|item| &item.id == id))
        {
            return Err(self.uncertain());
        }
        let overview = self.overview.as_mut().ok_or(VaultError::NotSynced)?;
        overview.items = items;
        overview.failures = self.base_failures + failures;
        let overview = overview.clone();
        self.write_started = false;
        Ok(VaultMutationResult { overview, id })
    }
}

fn schema(kind: u8) -> &'static [&'static str] {
    match kind {
        1 => &["username", "password", "totp"],
        2 => &[],
        3 => &[
            "cardholderName",
            "brand",
            "number",
            "expMonth",
            "expYear",
            "code",
        ],
        4 => &[
            "title",
            "firstName",
            "middleName",
            "lastName",
            "address1",
            "address2",
            "address3",
            "city",
            "state",
            "postalCode",
            "country",
            "company",
            "email",
            "phone",
            "ssn",
            "username",
            "passportNumber",
            "licenseNumber",
        ],
        5 => &["publicKey", "privateKey", "fingerprint"],
        6 => &[
            "bankName",
            "nameOnAccount",
            "accountType",
            "accountNumber",
            "routingNumber",
            "branchNumber",
            "pin",
            "swiftCode",
            "iban",
            "bankContactPhone",
        ],
        7 => &[
            "firstName",
            "middleName",
            "lastName",
            "dateOfBirth",
            "licenseNumber",
            "issuingCountry",
            "issuingState",
            "issueDate",
            "expirationDate",
            "issuingAuthority",
            "licenseClass",
        ],
        8 => &[
            "surname",
            "givenName",
            "dateOfBirth",
            "sex",
            "birthPlace",
            "nationality",
            "issuingCountry",
            "passportNumber",
            "passportType",
            "nationalIdentificationNumber",
            "issuingAuthority",
            "issueDate",
            "expirationDate",
        ],
        _ => &[],
    }
}

fn project_values(view: &CipherView) -> Result<HashMap<String, String>, VaultError> {
    if view.r#type as u8 == 1 {
        let Some(login) = &view.login else {
            return Err(VaultError::InvalidResponse);
        };
        return Ok([
            ("username", &login.username),
            ("password", &login.password),
            ("totp", &login.totp),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.as_ref().map(|value| (key.to_owned(), value.clone())))
        .collect());
    }
    let data = match view.r#type as u8 {
        2 => json!({}),
        3 => json!(&view.card),
        4 => json!(&view.identity),
        5 => json!(&view.ssh_key),
        6 => json!(&view.bank_account),
        7 => json!(&view.drivers_license),
        8 => json!(&view.passport),
        _ => return Err(VaultError::Unsupported),
    };
    Ok(schema(view.r#type as u8)
        .iter()
        .filter_map(|key| {
            data.get(*key)
                .and_then(Value::as_str)
                .map(|value| ((*key).to_owned(), value.to_owned()))
        })
        .collect())
}

fn new_view(kind: u8) -> Result<CipherView, VaultError> {
    let property = match kind {
        1 => "login",
        2 => "secureNote",
        3 => "card",
        4 => "identity",
        5 => "sshKey",
        6 => "bankAccount",
        7 => "driversLicense",
        8 => "passport",
        _ => return Err(VaultError::Validation),
    };
    let mut data = json!({"collectionIds":[],"name":"","type":kind,"favorite":false,"reprompt":0,"organizationUseTotp":false,"edit":true,"viewPassword":true,"creationDate":Utc::now(),"revisionDate":Utc::now()});
    data[property] = if kind == 2 {
        json!({"type":0})
    } else if kind == 5 {
        json!({"privateKey":"","publicKey":"","fingerprint":""})
    } else {
        json!({})
    };
    serde_json::from_value(data).map_err(|_| VaultError::Validation)
}

fn apply_input(view: &mut CipherView, input: &VaultEditorData) -> Result<(), VaultError> {
    view.name = input.name.clone();
    view.notes = input.notes.clone().filter(|value| !value.is_empty());
    view.folder_id = input
        .folder_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map(str::parse)
        .transpose()
        .map_err(|_| VaultError::Validation)?;
    view.favorite = input.favorite;
    view.reprompt = if input.reprompt {
        CipherRepromptType::Password
    } else {
        CipherRepromptType::None
    };
    let mut fields: Vec<FieldView> = Vec::new();
    for field in &input.custom_fields {
        if field.r#type > 3
            || (field.r#type == 3 && field.linked_id.is_none())
            || (field.r#type != 3 && field.linked_id.is_some())
        {
            return Err(VaultError::Validation);
        }
        if field.r#type == 2
            && field
                .value
                .as_deref()
                .is_some_and(|value| !matches!(value, "true" | "false" | ""))
        {
            return Err(VaultError::Validation);
        }
        fields.push(serde_json::from_value(json!({"name":field.name,"value":field.value,"type":field.r#type,"linkedId":field.linked_id})).map_err(|_| VaultError::Validation)?);
    }
    view.fields = Some(fields);
    if input.item_type == 1 {
        let login = view.login.as_mut().ok_or(VaultError::InvalidResponse)?;
        let value = |key: &str| {
            input
                .values
                .get(key)
                .filter(|value| !value.is_empty())
                .cloned()
        };
        login.username = value("username");
        login.password = value("password");
        login.totp = value("totp");
        let mut uris = Vec::new();
        for uri in &input.uris {
            if uri.r#match.is_some_and(|mode| mode > 5) {
                return Err(VaultError::Validation);
            }
            if uri.uri.as_deref().is_none_or(str::is_empty) {
                continue;
            }
            uris.push(
                serde_json::from_value::<LoginUriView>(json!({"uri":uri.uri,"match":uri.r#match}))
                    .map_err(|_| VaultError::Validation)?,
            );
        }
        login.uris = Some(uris);
        login.generate_checksums();
        return Ok(());
    }
    macro_rules! apply {
        ($field:ident) => {{
            let mut data = serde_json::to_value(&view.$field).map_err(|_| VaultError::Internal)?;
            let object = data.as_object_mut().ok_or(VaultError::InvalidResponse)?;
            for key in schema(input.item_type) {
                object.insert(
                    (*key).to_owned(),
                    if input.item_type == 5 {
                        json!(input.values.get(*key).cloned().unwrap_or_default())
                    } else {
                        input
                            .values
                            .get(*key)
                            .filter(|value| !value.is_empty())
                            .map_or(Value::Null, |value| json!(value))
                    },
                );
            }
            view.$field = Some(serde_json::from_value(data).map_err(|_| VaultError::Validation)?);
        }};
    }
    match input.item_type {
        2 => {}
        3 => apply!(card),
        4 => apply!(identity),
        5 => apply!(ssh_key),
        6 => apply!(bank_account),
        7 => apply!(drivers_license),
        8 => apply!(passport),
        _ => return Err(VaultError::Validation),
    }
    Ok(())
}

fn view_type(view: &CipherView) -> Result<CipherViewType, VaultError> {
    match view.r#type as u8 {
        1 => view.login.clone().map(CipherViewType::Login),
        2 => view.secure_note.clone().map(CipherViewType::SecureNote),
        3 => view.card.clone().map(CipherViewType::Card),
        4 => view.identity.clone().map(CipherViewType::Identity),
        5 => view.ssh_key.clone().map(CipherViewType::SshKey),
        6 => view.bank_account.clone().map(CipherViewType::BankAccount),
        7 => view
            .drivers_license
            .clone()
            .map(CipherViewType::DriversLicense),
        8 => view.passport.clone().map(CipherViewType::Passport),
        _ => None,
    }
    .ok_or(VaultError::Validation)
}

#[cfg(test)]
#[path = "mutations_tests.rs"]
mod tests;
