//! SDK FIDO operations. Approval and transaction verification happen before entry.
use super::{mutations, VaultCache, VaultError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bitwarden_fido::{
    CheckUserOptions, CheckUserResult, Fido2Authenticator, Fido2CallbackError,
    Fido2CredentialStore, Fido2UserInterface, GetAssertionRequest, MakeCredentialRequest,
    PublicKeyCredentialRpEntity, PublicKeyCredentialUserEntity, UiHint, Verification,
};
use bitwarden_vault::{
    CipherCreateRequest, CipherEditRequest, CipherListView, CipherView, EncryptionContext,
    Fido2CredentialNewView, VaultClientExt,
};
use bw_sdk_core::Client;
use serde::Serialize;
use std::sync::Mutex;

#[derive(Clone)]
pub(crate) struct PasskeyOperation {
    pub creating: bool,
    pub rp_id: String,
    pub user_name: Option<String>,
    pub user_display_name: Option<String>,
    pub user_handle: Vec<u8>,
    pub client_data_hash: Vec<u8>,
    pub credential_ids: Vec<Vec<u8>>,
    pub algorithms: Vec<i32>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PasskeyChoice {
    pub id: String,
    pub revision: String,
    pub credential_index: usize,
    pub name: String,
    pub user_name: Option<String>,
    pub reprompt: bool,
}
pub(crate) enum PasskeyResult {
    Created {
        authenticator_data: Vec<u8>,
        credential_id: Vec<u8>,
    },
    Assertion {
        credential_id: Vec<u8>,
        authenticator_data: Vec<u8>,
        signature: Vec<u8>,
        user_handle: Vec<u8>,
    },
}
impl PasskeyOperation {
    pub fn validate(&self) -> Result<(), VaultError> {
        if self.client_data_hash.len() != 32
            || self.rp_id.is_empty()
            || self.rp_id.len() > 253
            || self
                .rp_id
                .chars()
                .any(|c| c.is_control() || "/:@ ".contains(c) || c as u32 == 92)
            || (self.creating
                && (self.user_handle.is_empty()
                    || self.user_handle.len() > 64
                    || !self.algorithms.contains(&-7)))
        {
            return Err(VaultError::Validation);
        }
        Ok(())
    }
    fn matches(&self, credential: &bitwarden_vault::Fido2CredentialView) -> bool {
        credential.rp_id == self.rp_id
            && bitwarden_fido::string_to_guid_bytes(&credential.credential_id)
                .is_ok_and(|id| self.credential_ids.is_empty() || self.credential_ids.contains(&id))
    }
}
impl VaultCache {
    pub(crate) async fn passkey_views(
        &self,
        client: &Client,
    ) -> Result<Vec<CipherView>, VaultError> {
        self.ensure_synced()?;
        let mut views = Vec::new();
        for cipher in self
            .ciphers
            .snapshot()
            .map_err(|_| VaultError::Internal)?
            .into_values()
        {
            if cipher.r#type as u8 != 1
                || cipher.deleted_date.is_some()
                || cipher.archived_date.is_some()
                || cipher.partial_data.is_some()
                || !cipher.view_password
            {
                continue;
            }
            let Ok(view) = client.vault().ciphers().decrypt(cipher).await else {
                continue;
            };
            if !view.partial && view.view_password {
                views.push(view);
            }
        }
        Ok(views)
    }
    pub(crate) async fn passkey_choices(
        &self,
        client: &Client,
        op: &PasskeyOperation,
    ) -> Result<Vec<PasskeyChoice>, VaultError> {
        op.validate()?;
        let mut choices = Vec::new();
        for view in self.passkey_views(client).await? {
            let Some(id) = view.id else {
                continue;
            };
            let credentials = view.get_fido2_credentials();
            if op.creating {
                if !view.edit
                    || !credentials.is_empty()
                    || view
                        .attachment_decryption_failures
                        .as_ref()
                        .is_some_and(|v| !v.is_empty())
                {
                    continue;
                }
                choices.push(PasskeyChoice {
                    id: id.to_string(),
                    revision: view.revision_date.to_rfc3339(),
                    credential_index: 0,
                    name: view.name,
                    user_name: view.login.and_then(|v| v.username),
                    reprompt: view.reprompt as u8 != 0,
                });
            } else {
                for (index, credential) in credentials
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| op.matches(c))
                {
                    choices.push(PasskeyChoice {
                        id: id.to_string(),
                        revision: view.revision_date.to_rfc3339(),
                        credential_index: index,
                        name: view.name.clone(),
                        user_name: credential.user_name.clone(),
                        reprompt: view.reprompt as u8 != 0,
                    });
                }
            }
        }
        choices.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then(a.id.cmp(&b.id))
                .then(a.credential_index.cmp(&b.credential_index))
        });
        Ok(choices)
    }
    pub(crate) async fn perform_passkey(
        &mut self,
        client: &Client,
        op: &PasskeyOperation,
        id: &str,
        revision: &str,
        credential_index: usize,
        password: &str,
    ) -> Result<PasskeyResult, VaultError> {
        op.validate()?;
        self.ensure_synced()?;
        self.write_started = false;
        let original = if id.is_empty() && op.creating {
            let mut view = mutations::new_view(1)?;
            view.name = op.rp_id.clone();
            let login = view.login.as_mut().ok_or(VaultError::Internal)?;
            login.username = op.user_name.clone();
            login.uris = Some(vec![bitwarden_vault::LoginUriView {
                uri: Some(format!("https://{}", op.rp_id)),
                r#match: None,
                uri_checksum: None,
            }]);
            view
        } else {
            let choices = self.passkey_choices(client, op).await?;
            if !choices
                .iter()
                .any(|c| c.id == id && c.credential_index == credential_index)
            {
                return Err(VaultError::Conflict);
            }
            let cipher = self.find_cipher(id)?;
            Self::check_revision(&cipher, Some(revision))?;
            self.check_reprompt(client, &cipher, password).await?;
            client
                .vault()
                .ciphers()
                .decrypt(cipher)
                .await
                .map_err(|_| VaultError::DecryptionFailed)?
        };
        let mut selected = original.clone();
        if !op.creating {
            let credentials = original.get_fido2_credentials();
            let credential = credentials
                .get(credential_index)
                .ok_or(VaultError::NotFound)?;
            selected
                .set_new_fido2_credentials(vec![credential.clone().into()])
                .map_err(|_| VaultError::DecryptionFailed)?;
        }
        let views = self.passkey_views(client).await?;
        if op.creating
            && !op.credential_ids.is_empty()
            && views
                .iter()
                .any(|v| v.get_fido2_credentials().iter().any(|c| op.matches(c)))
        {
            return Err(VaultError::CredentialExcluded);
        }
        let callbacks = Callbacks {
            selected,
            views,
            staged: Mutex::new(None),
        };
        let mut authenticator = Fido2Authenticator::new(client, &callbacks, &callbacks);
        let options = CheckUserOptions {
            require_presence: true,
            require_verification: Verification::Required,
        }
        .into();
        let descriptors: Vec<bitwarden_api_api::models::PublicKeyCredentialDescriptor> = op
            .credential_ids
            .iter()
            .map(|id| {
                serde_json::from_value(
                    serde_json::json!({"type":"public-key", "id":URL_SAFE_NO_PAD.encode(id)}),
                )
                .map_err(|_| VaultError::Validation)
            })
            .collect::<Result<_, _>>()?;
        let result = if op.creating {
            let params: bitwarden_api_api::models::PubKeyCredParam =
                serde_json::from_value(serde_json::json!({"type":"public-key", "alg":-7}))
                    .map_err(|_| VaultError::Internal)?;
            let response = authenticator
                .make_credential(MakeCredentialRequest {
                    client_data_hash: op.client_data_hash.clone(),
                    rp: PublicKeyCredentialRpEntity {
                        id: op.rp_id.clone(),
                        name: Some(op.rp_id.clone()),
                    },
                    user: PublicKeyCredentialUserEntity {
                        id: op.user_handle.clone(),
                        name: op.user_name.clone().unwrap_or_default(),
                        display_name: op.user_display_name.clone().unwrap_or_default(),
                    },
                    pub_key_cred_params: vec![(&params)
                        .try_into()
                        .map_err(|_| VaultError::Internal)?],
                    exclude_list: Some(
                        descriptors
                            .iter()
                            .map(TryInto::try_into)
                            .collect::<Result<_, _>>()
                            .map_err(|_| VaultError::Validation)?,
                    ),
                    options,
                    extensions: None,
                })
                .await
                .map_err(|_| VaultError::Unsupported)?;
            PasskeyResult::Created {
                authenticator_data: response.authenticator_data,
                credential_id: response.credential_id,
            }
        } else {
            let response = authenticator
                .get_assertion(GetAssertionRequest {
                    rp_id: op.rp_id.clone(),
                    client_data_hash: op.client_data_hash.clone(),
                    allow_list: Some(
                        descriptors
                            .iter()
                            .map(TryInto::try_into)
                            .collect::<Result<_, _>>()
                            .map_err(|_| VaultError::Validation)?,
                    ),
                    options,
                    extensions: None,
                })
                .await
                .map_err(|_| VaultError::Unsupported)?;
            PasskeyResult::Assertion {
                credential_id: response.credential_id,
                authenticator_data: response.authenticator_data,
                signature: response.signature,
                user_handle: response.user_handle,
            }
        };
        let staged = callbacks
            .staged
            .into_inner()
            .map_err(|_| VaultError::Internal)?;
        if let Some(context) = staged {
            let mut updated = client
                .vault()
                .ciphers()
                .decrypt(context.cipher)
                .await
                .map_err(|_| VaultError::DecryptionFailed)?;
            if !op.creating {
                let mut credentials: Vec<bitwarden_vault::Fido2CredentialFullView> = original
                    .get_fido2_credentials()
                    .into_iter()
                    .map(Into::into)
                    .collect();
                let changed = updated
                    .get_fido2_credentials()
                    .into_iter()
                    .next()
                    .ok_or(VaultError::Internal)?;
                let old = credentials
                    .get_mut(credential_index)
                    .ok_or(VaultError::Internal)?;
                if old.counter == changed.counter {
                    return Ok(result);
                }
                if !original.edit {
                    return Err(VaultError::PermissionDenied);
                }
                *old = changed.into();
                updated
                    .set_new_fido2_credentials(credentials)
                    .map_err(|_| VaultError::Internal)?;
            }
            self.activate(client);
            let expected_key = updated
                .get_fido2_credentials()
                .into_iter()
                .find(|c| {
                    let expected_id = match &result {
                        PasskeyResult::Created { credential_id, .. }
                        | PasskeyResult::Assertion { credential_id, .. } => credential_id,
                    };
                    bitwarden_fido::string_to_guid_bytes(&c.credential_id)
                        .is_ok_and(|id| &id == expected_id)
                })
                .ok_or(VaultError::Internal)?
                .key_value;
            self.write_started = true;
            let confirmed = if op.creating && original.id.is_none() {
                let request = CipherCreateRequest {
                    organization_id: None,
                    collection_ids: vec![],
                    folder_id: updated.folder_id,
                    name: updated.name.clone(),
                    notes: updated.notes.clone(),
                    favorite: updated.favorite,
                    reprompt: updated.reprompt,
                    r#type: mutations::view_type(&updated)?,
                    fields: updated.fields.unwrap_or_default(),
                    archived_date: None,
                };
                client
                    .vault()
                    .ciphers()
                    .create(request)
                    .await
                    .map_err(|e| self.classify_create_error(e))?
            } else {
                client
                    .vault()
                    .ciphers()
                    .edit(CipherEditRequest::try_from(updated).map_err(|_| VaultError::Validation)?)
                    .await
                    .map_err(|e| self.classify_edit_error(e))?
            };
            let expected = match &result {
                PasskeyResult::Created { credential_id, .. }
                | PasskeyResult::Assertion { credential_id, .. } => credential_id,
            };
            if !confirmed.get_fido2_credentials().iter().any(|c| {
                bitwarden_fido::string_to_guid_bytes(&c.credential_id)
                    .is_ok_and(|id| &id == expected)
                    && c.key_value == expected_key
            }) {
                return Err(self.uncertain());
            }
            self.finish_write(client, confirmed.id.map(|v| v.to_string()))
                .await?;
        } else if op.creating {
            return Err(VaultError::Internal);
        }
        Ok(result)
    }
}
struct Callbacks {
    selected: CipherView,
    views: Vec<CipherView>,
    staged: Mutex<Option<EncryptionContext>>,
}
#[async_trait::async_trait]
impl Fido2UserInterface for Callbacks {
    async fn check_user<'a>(
        &self,
        _: CheckUserOptions,
        _: UiHint<'a, CipherView>,
    ) -> Result<CheckUserResult, Fido2CallbackError> {
        Ok(CheckUserResult {
            user_present: true,
            user_verified: true,
        })
    }
    async fn pick_credential_for_authentication(
        &self,
        available: Vec<CipherView>,
    ) -> Result<CipherView, Fido2CallbackError> {
        if available.iter().any(|v| v.id == self.selected.id) {
            Ok(self.selected.clone())
        } else {
            Err(Fido2CallbackError::OperationCancelled)
        }
    }
    async fn check_user_and_pick_credential_for_creation(
        &self,
        _: CheckUserOptions,
        _: Fido2CredentialNewView,
    ) -> Result<(CipherView, CheckUserResult), Fido2CallbackError> {
        Ok((
            self.selected.clone(),
            CheckUserResult {
                user_present: true,
                user_verified: true,
            },
        ))
    }
    fn is_verification_enabled(&self) -> bool {
        true
    }
}
#[async_trait::async_trait]
impl Fido2CredentialStore for Callbacks {
    async fn find_credentials(
        &self,
        ids: Option<Vec<Vec<u8>>>,
        rp_id: String,
        user_handle: Option<Vec<u8>>,
    ) -> Result<Vec<CipherView>, Fido2CallbackError> {
        let mut found = Vec::new();
        for view in &self.views {
            let credentials: Vec<_> = view
                .get_fido2_credentials()
                .into_iter()
                .filter(|c| {
                    c.rp_id == rp_id
                        && bitwarden_fido::string_to_guid_bytes(&c.credential_id).is_ok_and(|id| {
                            ids.as_ref()
                                .is_none_or(|ids| ids.is_empty() || ids.contains(&id))
                        })
                        && user_handle.as_ref().is_none_or(|handle| {
                            c.user_handle
                                .as_ref()
                                .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
                                .as_ref()
                                == Some(handle)
                        })
                })
                .map(Into::into)
                .collect();
            if !credentials.is_empty() {
                let mut view = view.clone();
                view.set_new_fido2_credentials(credentials)
                    .map_err(|_| Fido2CallbackError::OperationCancelled)?;
                found.push(view);
            }
        }
        Ok(found)
    }
    async fn all_credentials(&self) -> Result<Vec<CipherListView>, Fido2CallbackError> {
        Err(Fido2CallbackError::UserInterfaceRequired)
    }
    async fn save_credential(&self, context: EncryptionContext) -> Result<(), Fido2CallbackError> {
        *self
            .staged
            .lock()
            .map_err(|_| Fido2CallbackError::OperationCancelled)? = Some(context);
        Ok(())
    }
}
