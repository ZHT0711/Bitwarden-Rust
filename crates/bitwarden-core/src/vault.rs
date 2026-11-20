//! The account vault uses official SDK ciphertext and crypto, never the local snapshot prototype.
//! Only display metadata is cached in plaintext. Secrets are decrypted for one selected item.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

mod mutations;
mod repository;
pub(crate) use mutations::{VaultEditorData, VaultMutationResult};
use repository::CipherRepository;

use bitwarden_api_api::models::SyncResponseModel;
use bitwarden_crypto::UnsignedSharedKey;
use bitwarden_crypto_sync_handler::{CryptoSyncData, CryptoSyncHandlerClientExt};
use bitwarden_vault::{Cipher, CipherView, Folder, VaultClientExt};
use bw_sdk_core::{
    auth::login::PasswordLoginUnlockData,
    key_management::account_cryptographic_state::WrappedAccountCryptographicState, Client,
    OrganizationId,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub(crate) enum VaultError {
    #[error("notAuthenticated")]
    NotAuthenticated,
    #[error("network")]
    Network,
    #[error("invalidResponse")]
    InvalidResponse,
    #[error("notFound")]
    NotFound,
    #[error("decryptionFailed")]
    DecryptionFailed,
    #[error("passwordRequired")]
    PasswordRequired,
    #[error("wrongPassword")]
    WrongPassword,
    #[error("validation")]
    Validation,
    #[error("permissionDenied")]
    PermissionDenied,
    #[error("conflict")]
    Conflict,
    #[error("writeUncertain")]
    WriteUncertain,
    #[error("notSynced")]
    NotSynced,
    #[error("unsupported")]
    Unsupported,
    #[error("premiumRequired")]
    PremiumRequired,
    #[error("attachmentUnavailable")]
    AttachmentUnavailable,
    #[error("internal")]
    Internal,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VaultItem {
    id: String,
    name: String,
    subtitle: String,
    item_type: u8,
    favorite: bool,
    folder_id: Option<String>,
    organization_id: Option<String>,
    deleted: bool,
    archived: bool,
    reprompt: bool,
    restricted: bool,
    revision_date: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct VaultFolder {
    id: String,
    name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VaultOverview {
    items: Vec<VaultItem>,
    folders: Vec<VaultFolder>,
    last_sync: String,
    failures: usize,
}

#[derive(Default)]
pub(crate) struct VaultCache {
    ciphers: Arc<CipherRepository>,
    folders: Vec<Folder>,
    org_keys: Vec<(OrganizationId, UnsignedSharedKey)>,
    encrypted_user_key: Option<String>,
    overview: Option<VaultOverview>,
    last_sync: Option<String>,
    parse_failures: usize,
    base_failures: usize,
    can_archive: bool,
    write_uncertain: bool,
    write_started: bool,
}

#[derive(Serialize)]
pub(crate) struct DisplayField {
    label: String,
    value: String,
    sensitive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VaultDetail {
    name: String,
    item_type: u8,
    fields: Vec<DisplayField>,
    revision_date: String,
    restricted: bool,
    password_hidden: bool,
    attachments: usize,
}

impl VaultCache {
    /// The same encrypted map is used by this cache and the SDK's high-level write clients.
    /// Registration is synchronous, so a cancelled sync cannot partially replace SDK storage.
    pub(crate) fn activate(&self, client: &Client) {
        client
            .platform()
            .state()
            .register_client_managed(self.ciphers.clone());
    }

    pub(crate) fn mark_write_uncertain(&mut self) {
        self.write_uncertain = true;
    }
    pub(crate) fn write_in_flight(&self) -> bool {
        self.write_started
    }
    pub(crate) fn needs_reconciliation(&self) -> bool {
        self.write_uncertain
    }
    pub(crate) fn reset_write_attempt(&mut self) {
        self.write_started = false;
    }

    pub(crate) fn overview(&self) -> Result<&VaultOverview, VaultError> {
        self.overview.as_ref().ok_or(VaultError::NotFound)
    }

    /// Build a replacement before touching the last successful cache. Failed syncs retain it.
    pub(crate) async fn fetch(
        client: &Client,
        unlock_data: &mut Option<PasswordLoginUnlockData>,
    ) -> Result<Self, VaultError> {
        let response = client
            .internal
            .get_api_configurations()
            .api_client
            .sync_api()
            .get(Some(true))
            .await
            .map_err(|_| VaultError::Network)?;
        Self::from_response(client, response, unlock_data).await
    }

    async fn from_response(
        client: &Client,
        response: SyncResponseModel,
        unlock_data: &mut Option<PasswordLoginUnlockData>,
    ) -> Result<Self, VaultError> {
        let api_ciphers = response
            .ciphers
            .as_ref()
            .ok_or(VaultError::InvalidResponse)?;
        let api_folders = response
            .folders
            .as_ref()
            .ok_or(VaultError::InvalidResponse)?;
        let profile = response
            .profile
            .as_deref()
            .ok_or(VaultError::InvalidResponse)?;
        // The SDK's legacy password-login helper initializes crypto/tokens but not its user id.
        // Bind it from this authenticated sync response before high-level writes need encrypted_for.
        // init_user_id accepts the same id and rejects every later cross-account reassignment.
        let user_id = profile.id.ok_or(VaultError::InvalidResponse)?;
        client
            .internal
            .init_user_id(bw_sdk_core::UserId::new(user_id))
            .await
            .map_err(|_| VaultError::InvalidResponse)?;
        let crypto =
            CryptoSyncData::try_from(&response).map_err(|_| VaultError::InvalidResponse)?;
        // A local unlock may only use the wrapped keys from this login. If sync reports new
        // key material, require a fresh login so the old master password cannot unlock it.
        if unlock_data.as_ref().is_some_and(|keys| {
            let account_changed = match (&crypto.account_cryptographic_state, &keys.account) {
                (Some(account), original) => account != original,
                (None, WrappedAccountCryptographicState::V1 { private_key }) => profile
                    .private_key
                    .as_ref()
                    .is_some_and(|key| key != &private_key.to_string()),
                (None, _) => false,
            };
            let wrapper_changed = match crypto.user_decryption.as_ref() {
                Some(decryption) => {
                    decryption
                        .master_password_unlock
                        .as_ref()
                        .is_none_or(|current| {
                            current.kdf != keys.master_password.kdf
                                || current.salt != keys.master_password.salt
                                || current.master_key_wrapped_user_key
                                    != keys.master_password.master_key_wrapped_user_key
                                || decryption.v2_upgrade_token.is_some()
                        })
                }
                None => profile.key.as_ref().is_some_and(|key| {
                    key != &keys.master_password.master_key_wrapped_user_key.to_string()
                }),
            };
            account_changed || wrapper_changed
        }) {
            *unlock_data = None;
        }
        // The SDK sync handler checks for a crypto-state downgrade through the host's
        // state bridge. A native client can authenticate and sync without that bridge.
        if client.km_state_bridge().is_bridge_registered() {
            client.crypto_sync_handler().on_sync(crypto).await;
        }
        let mut org_keys = Vec::new();
        for org in profile.organizations.iter().flatten() {
            if let (Some(id), Some(key)) = (org.id, org.key.as_ref()) {
                org_keys.push((
                    OrganizationId::new(id),
                    key.parse().map_err(|_| VaultError::InvalidResponse)?,
                ));
            }
        }
        client
            .internal
            .initialize_org_crypto(org_keys.clone())
            .map_err(|_| VaultError::DecryptionFailed)?;

        let mut failures = 0;
        let mut ciphers = HashMap::new();
        for raw in api_ciphers {
            match Cipher::try_from(raw.clone()) {
                Ok(cipher) => {
                    let Some(id) = cipher.id.map(|id| id.to_string()) else {
                        failures += 1;
                        continue;
                    };
                    if ciphers.insert(id, cipher).is_some() {
                        return Err(VaultError::InvalidResponse);
                    }
                }
                Err(_) => failures += 1,
            }
        }
        // This client must report corrupted encrypted fields, rather than silently replacing
        // them with empty strings. Preserve unrelated SDK flags when enabling strict decoding.
        let mut flags: HashMap<String, bool> = serde_json::from_value(
            serde_json::to_value(client.flags().get().await)
                .map_err(|_| VaultError::InvalidResponse)?,
        )
        .map_err(|_| VaultError::InvalidResponse)?;
        flags.insert("strict-cipher-decryption".to_owned(), true);
        client.flags().load(flags).await;
        let folders = api_folders
            .iter()
            .filter_map(|raw| Folder::try_from(raw.clone()).ok())
            .collect::<Vec<_>>();
        let mut cache = Self {
            ciphers: Arc::new(CipherRepository::from(ciphers)),
            org_keys,
            parse_failures: failures + api_folders.len() - folders.len(),
            folders,
            encrypted_user_key: profile.key.clone(),
            can_archive: profile.premium.unwrap_or(false)
                || profile
                    .organizations
                    .iter()
                    .flatten()
                    .any(|org| org.users_get_premium.unwrap_or(false)),
            last_sync: Some(Utc::now().to_rfc3339()),
            ..Self::default()
        };
        cache.rebuild_overview(client).await?;
        Ok(cache)
    }

    pub(crate) fn discard_overview(&mut self) {
        if let Some(mut overview) = self.overview.take() {
            use zeroize::Zeroize;
            for item in &mut overview.items {
                item.name.zeroize();
                item.subtitle.zeroize();
            }
            for folder in &mut overview.folders {
                folder.name.zeroize();
            }
        }
    }

    pub(crate) async fn restore_overview(&mut self, client: &Client) -> Result<(), VaultError> {
        if self.last_sync.is_some() && !self.write_uncertain {
            client
                .internal
                .initialize_org_crypto(self.org_keys.clone())
                .map_err(|_| VaultError::DecryptionFailed)?;
            self.rebuild_overview(client).await?;
        }
        Ok(())
    }

    async fn project_items(&self, client: &Client) -> Result<(Vec<VaultItem>, usize), VaultError> {
        let ciphers = self
            .ciphers
            .snapshot()
            .map_err(|_| VaultError::Internal)?
            .into_values()
            .collect::<Vec<_>>();
        let types: HashMap<_, _> = ciphers
            .iter()
            .filter_map(|cipher| cipher.id.map(|id| (id, cipher.r#type as u8)))
            .collect();
        let result = client
            .vault()
            .ciphers()
            .decrypt_list_with_failures(ciphers.into())
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        let failures = result.failures.len();
        let mut items = Vec::new();
        for view in result.successes {
            let Some(id) = view.id else {
                continue;
            };
            items.push(VaultItem {
                id: id.to_string(),
                name: view.name,
                subtitle: view.subtitle,
                item_type: *types.get(&id).ok_or(VaultError::InvalidResponse)?,
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
        Ok((items, failures))
    }

    async fn rebuild_overview(&mut self, client: &Client) -> Result<(), VaultError> {
        let last_sync = self.last_sync.clone().ok_or(VaultError::NotSynced)?;
        let (items, decryption_failures) = self.project_items(client).await?;
        let mut failures = self.parse_failures;
        let mut folders = Vec::new();
        let mut ids = HashSet::new();
        for folder in &self.folders {
            #[allow(deprecated)]
            if let Ok(view) = client.vault().folders().decrypt(folder.clone()) {
                if let Some(id) = view.id {
                    if !ids.insert(id) {
                        return Err(VaultError::InvalidResponse);
                    }
                    folders.push(VaultFolder {
                        id: id.to_string(),
                        name: view.name,
                    });
                } else {
                    failures += 1;
                }
            } else {
                failures += 1;
            }
        }
        folders.sort_by_cached_key(|folder| folder.name.to_lowercase());
        self.base_failures = failures;
        self.overview = Some(VaultOverview {
            items,
            folders,
            failures: failures + decryption_failures,
            last_sync,
        });
        Ok(())
    }

    pub(crate) async fn detail(
        &self,
        client: &Client,
        id: &str,
        password: &str,
    ) -> Result<VaultDetail, VaultError> {
        if self.write_uncertain {
            return Err(VaultError::NotSynced);
        }
        let cipher = self
            .ciphers
            .find(id)
            .map_err(|_| VaultError::Internal)?
            .ok_or(VaultError::NotFound)?;
        if cipher.reprompt as u8 != 0 {
            if password.is_empty() {
                return Err(VaultError::PasswordRequired);
            }
            let encrypted_key = self
                .encrypted_user_key
                .as_ref()
                .ok_or(VaultError::PasswordRequired)?;
            client
                .auth()
                .validate_password_user_key(password.to_owned(), encrypted_key.clone())
                .await
                .map_err(|_| VaultError::WrongPassword)?;
        }
        let view = client
            .vault()
            .ciphers()
            .decrypt(cipher.clone())
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        Ok(display_detail(view))
    }
}

fn display_detail(view: CipherView) -> VaultDetail {
    let mut fields = Vec::new();
    // Explicit projections keep cipher keys, passkey private material and password history out of FFI.
    if let Some(login) = &view.login {
        add_field(&mut fields, "用户名", login.username.as_deref(), false);
        if view.view_password {
            add_field(&mut fields, "密码", login.password.as_deref(), true);
        }
        for uri in login.uris.iter().flatten() {
            add_field(&mut fields, "网站", uri.uri.as_deref(), false);
        }
    }
    append_object(
        &mut fields,
        &view.card,
        &[
            ("cardholderName", "持卡人", false),
            ("brand", "卡组织", false),
            ("number", "卡号", true),
            ("expMonth", "到期月份", false),
            ("expYear", "到期年份", false),
            ("code", "安全码", true),
        ],
    );
    append_object(
        &mut fields,
        &view.identity,
        &[
            ("title", "称谓", false),
            ("firstName", "名", false),
            ("middleName", "中间名", false),
            ("lastName", "姓", false),
            ("username", "用户名", false),
            ("company", "公司", false),
            ("email", "邮箱", false),
            ("phone", "电话", false),
            ("address1", "地址 1", false),
            ("address2", "地址 2", false),
            ("address3", "地址 3", false),
            ("city", "城市", false),
            ("state", "省 / 州", false),
            ("postalCode", "邮编", false),
            ("country", "国家", false),
            ("ssn", "社会保障号码", true),
            ("passportNumber", "护照号码", true),
            ("licenseNumber", "驾照号码", true),
        ],
    );
    append_object(
        &mut fields,
        &view.ssh_key,
        &[
            ("publicKey", "公钥", false),
            ("privateKey", "私钥", true),
            ("fingerprint", "指纹", false),
        ],
    );
    append_object(
        &mut fields,
        &view.bank_account,
        &[
            ("bankName", "银行名称", false),
            ("nameOnAccount", "账户持有人", false),
            ("accountType", "账户类型", false),
            ("accountNumber", "账号", true),
            ("routingNumber", "路由号码", true),
            ("branchNumber", "分行号码", false),
            ("pin", "PIN", true),
            ("iban", "IBAN", true),
            ("swiftCode", "SWIFT", false),
            ("bankContactPhone", "银行联系电话", false),
        ],
    );
    append_object(
        &mut fields,
        &view.drivers_license,
        &[
            ("firstName", "名", false),
            ("middleName", "中间名", false),
            ("lastName", "姓", false),
            ("dateOfBirth", "出生日期", true),
            ("licenseNumber", "驾照号码", true),
            ("issuingCountry", "签发国家", false),
            ("issuingState", "签发省 / 州", false),
            ("issueDate", "签发日期", false),
            ("expirationDate", "到期日期", false),
            ("issuingAuthority", "签发机构", false),
            ("licenseClass", "准驾类型", false),
        ],
    );
    append_object(
        &mut fields,
        &view.passport,
        &[
            ("surname", "姓", false),
            ("givenName", "名", false),
            ("passportNumber", "护照号码", true),
            ("dateOfBirth", "出生日期", true),
            ("nationality", "国籍", false),
            ("issuingCountry", "签发国家", false),
            ("issueDate", "签发日期", false),
            ("expirationDate", "到期日期", false),
            ("sex", "性别", false),
            ("birthPlace", "出生地", false),
            ("passportType", "护照类型", false),
            ("nationalIdentificationNumber", "国民身份号码", true),
            ("issuingAuthority", "签发机构", false),
        ],
    );
    add_field(&mut fields, "备注", view.notes.as_deref(), false);
    for field in view.fields.iter().flatten() {
        add_field(
            &mut fields,
            field.name.as_deref().unwrap_or("自定义字段"),
            field.value.as_deref(),
            field.r#type as u8 == 1,
        );
    }
    if !view.view_password || view.partial {
        fields.retain(|field| !field.sensitive);
    }
    VaultDetail {
        name: view.name,
        item_type: view.r#type as u8,
        fields,
        revision_date: view.revision_date.to_rfc3339(),
        restricted: view.partial,
        password_hidden: !view.view_password,
        attachments: view.attachments.as_ref().map_or(0, Vec::len),
    }
}

fn add_field(fields: &mut Vec<DisplayField>, label: &str, value: Option<&str>, sensitive: bool) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        fields.push(DisplayField {
            label: label.to_owned(),
            value: value.to_owned(),
            sensitive,
        });
    }
}

fn append_object<T: Serialize>(
    fields: &mut Vec<DisplayField>,
    object: &Option<T>,
    names: &[(&str, &str, bool)],
) {
    if let Some(object) = object
        .as_ref()
        .and_then(|value| serde_json::to_value(value).ok())
    {
        for (key, label, sensitive) in names {
            if let Some(Value::String(value)) = object.get(key) {
                add_field(fields, label, Some(value), *sensitive);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitwarden_api_api::{
        apis::ApiClient,
        models::{CipherDetailsResponseModel, ProfileResponseModel},
    };
    use bw_sdk_core::client::test_accounts::{
        test_bitwarden_com_account, test_bitwarden_com_account_v2,
    };
    use serde_json::json;

    fn test_view() -> CipherView {
        serde_json::from_value(json!({
            "id": uuid::Uuid::new_v4(), "collectionIds": [], "name": "Synthetic login", "notes": "Synthetic note",
            "type": 1, "login": { "username": "fixture-user", "password": "synthetic-password-DO-NOT-USE",
                "uris": [{ "uri": "https://example.invalid" }] },
            "favorite": true, "reprompt": 0, "organizationUseTotp": false, "edit": true, "viewPassword": true,
            "creationDate": "2026-11-20T00:00:00Z", "revisionDate": "2026-11-20T00:00:00Z"
        })).unwrap()
    }

    async fn response_for(client: &Client, view: CipherView) -> SyncResponseModel {
        let encrypted = client.vault().ciphers().encrypt(view).await.unwrap().cipher;
        let raw: CipherDetailsResponseModel =
            serde_json::from_value(serde_json::to_value(encrypted).unwrap()).unwrap();
        // Public synthetic SDK fixture; this is ciphertext, not a real account or secret.
        let encrypted_user_key = Some("2.Q/2PhzcC7GdeiMHhWguYAQ==|GpqzVdr0go0ug5cZh1n+uixeBC3oC90CIe0hd/HWA/pTRDZ8ane4fmsEIcuc8eMKUt55Y2q/fbNzsYu41YTZzzsJUSeqVjT8/iTQtgnNdpo=|dwI+uyvZ1h/iZ03VQ+/wrGEFYVewBUUl/syYgjsNMbE=".to_owned());
        SyncResponseModel {
            profile: Some(Box::new(ProfileResponseModel {
                id: Some(
                    client
                        .internal
                        .get_user_id()
                        .unwrap()
                        .to_string()
                        .parse()
                        .unwrap(),
                ),
                key: encrypted_user_key,
                organizations: Some(vec![]),
                ..Default::default()
            })),
            ciphers: Some(vec![raw]),
            folders: Some(vec![]),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn official_sync_api_decrypts_metadata_without_exposing_secrets() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let response = response_for(&client, test_view()).await;
        let mock = ApiClient::new_mocked(move |mock| {
            let response = response.clone();
            mock.sync_api
                .expect_get()
                .withf(|exclude| *exclude == Some(true))
                .times(1)
                .returning(move |_| Ok(response.clone()));
        });
        let client =
            Client::init_test_account_with_api_client(test_bitwarden_com_account(), mock).await;
        let cache = VaultCache::fetch(&client, &mut None).await.unwrap();
        let overview = cache.overview().unwrap();
        assert_eq!(overview.items.len(), 1);
        assert_eq!(overview.items[0].name, "Synthetic login");
        assert!(overview.items[0].favorite);
        assert_eq!(overview.failures, 0);
        let json = serde_json::to_string(overview).unwrap();
        assert!(!json.contains("synthetic-password-DO-NOT-USE"));
        assert!(!json.contains("Synthetic note"));
        let detail = cache
            .detail(&client, &overview.items[0].id, "")
            .await
            .unwrap();
        assert!(detail
            .fields
            .iter()
            .any(|field| field.label == "密码" && field.sensitive));
        assert!(!serde_json::to_value(detail)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("key"));
    }

    #[tokio::test]
    async fn malformed_records_are_counted_and_do_not_discard_healthy_records() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let mut response = response_for(&client, test_view()).await;
        let mut broken = response.ciphers.as_ref().unwrap()[0].clone();
        broken.id = Some(uuid::Uuid::new_v4());
        broken.name = Some("not-an-encrypted-string".to_owned());
        response.ciphers.as_mut().unwrap().push(broken);
        let cache = VaultCache::from_response(&client, response, &mut None)
            .await
            .unwrap();
        assert_eq!(cache.overview().unwrap().items.len(), 1);
        assert_eq!(cache.overview().unwrap().failures, 1);
    }

    #[tokio::test]
    async fn incomplete_duplicate_and_cross_account_responses_are_rejected() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let response = response_for(&client, test_view()).await;
        let mut incomplete = response.clone();
        incomplete.ciphers = None;
        assert!(matches!(
            VaultCache::from_response(&client, incomplete, &mut None).await,
            Err(VaultError::InvalidResponse)
        ));
        let mut wrong_account = response.clone();
        wrong_account.profile.as_mut().unwrap().id = Some(uuid::Uuid::new_v4());
        assert!(matches!(
            VaultCache::from_response(&client, wrong_account, &mut None).await,
            Err(VaultError::InvalidResponse)
        ));
        let mut duplicate = response;
        let cipher = duplicate.ciphers.as_ref().unwrap()[0].clone();
        duplicate.ciphers.as_mut().unwrap().push(cipher);
        assert!(matches!(
            VaultCache::from_response(&client, duplicate, &mut None).await,
            Err(VaultError::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn reprompt_is_enforced_in_rust_and_checks_the_actual_user_key() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let mut view = test_view();
        view.reprompt = bitwarden_vault::CipherRepromptType::Password;
        let cache =
            VaultCache::from_response(&client, response_for(&client, view).await, &mut None)
                .await
                .unwrap();
        let id = &cache.overview().unwrap().items[0].id;
        assert!(matches!(
            cache.detail(&client, id, "").await,
            Err(VaultError::PasswordRequired)
        ));
        assert!(matches!(
            cache.detail(&client, id, "wrong-password").await,
            Err(VaultError::WrongPassword)
        ));
        assert!(cache.detail(&client, id, "asdfasdfasdf").await.is_ok());
    }

    #[test]
    fn organization_password_permissions_strip_hidden_fields_before_ffi() {
        let mut view = test_view();
        view.view_password = false;
        let detail = display_detail(view);
        assert!(detail.password_hidden);
        assert!(!detail.fields.iter().any(|field| field.sensitive));
        assert!(!serde_json::to_string(&detail)
            .unwrap()
            .contains("synthetic-password-DO-NOT-USE"));
    }

    #[tokio::test]
    async fn all_eight_sdk_item_types_project_their_actual_fields() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let examples = [
            (
                1,
                "login",
                json!({"username":"fixture-user","password":"fixture-secret"}),
                "密码",
            ),
            (2, "secureNote", json!({"type":0}), "备注"),
            (
                3,
                "card",
                json!({"number":"4111111111111111","code":"123"}),
                "卡号",
            ),
            (4, "identity", json!({"firstName":"Synthetic"}), "名"),
            (
                5,
                "sshKey",
                json!({"privateKey":"synthetic-private-key","publicKey":"synthetic-public-key","fingerprint":"fixture"}),
                "私钥",
            ),
            (
                6,
                "bankAccount",
                json!({"nameOnAccount":"Synthetic","accountNumber":"1234","swiftCode":"TEST"}),
                "账户持有人",
            ),
            (
                7,
                "driversLicense",
                json!({"licenseNumber":"SYNTHETIC"}),
                "驾照号码",
            ),
            (
                8,
                "passport",
                json!({"passportNumber":"SYNTHETIC","nationalIdentificationNumber":"TEST"}),
                "国民身份号码",
            ),
        ];
        for (kind, property, data, label) in examples {
            let mut value = serde_json::to_value(test_view()).unwrap();
            value["type"] = json!(kind);
            value["login"] = Value::Null;
            value[property] = data;
            let view: CipherView = serde_json::from_value(value).unwrap();
            let cache =
                VaultCache::from_response(&client, response_for(&client, view).await, &mut None)
                    .await
                    .unwrap();
            let item = &cache.overview().unwrap().items[0];
            assert_eq!(item.item_type, kind);
            let detail = cache.detail(&client, &item.id, "").await.unwrap();
            assert!(
                detail.fields.iter().any(|field| field.label == label),
                "missing field for type {kind}"
            );
        }
    }

    #[tokio::test]
    async fn organization_cipher_is_decrypted_with_the_synced_wrapped_organization_key() {
        use bw_sdk_core::key_management::SymmetricKeySlotId;

        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let mut view = test_view();
        view.organization_id = Some("1bc9ac1e-f5aa-45f2-94bf-b181009709b8".parse().unwrap());
        let mut response = response_for(&client, view).await;
        response.profile.as_mut().unwrap().organizations = Some(vec![serde_json::from_value(json!({
            "id":"1bc9ac1e-f5aa-45f2-94bf-b181009709b8",
            "key":"4.rY01mZFXHOsBAg5Fq4gyXuklWfm6mQASm42DJpx05a+e2mmp+P5W6r54WU2hlREX0uoTxyP91bKKwickSPdCQQ58J45LXHdr9t2uzOYyjVzpzebFcdMw1eElR9W2DW8wEk9+mvtWvKwu7yTebzND+46y1nRMoFydi5zPVLSlJEf81qZZ4Uh1UUMLwXz+NRWfixnGXgq2wRq1bH0n3mqDhayiG4LJKgGdDjWXC8W8MMXDYx24SIJrJu9KiNEMprJE+XVF9nQVNijNAjlWBqkDpsfaWTUfeVLRLctfAqW1blsmIv4RQ91PupYJZDNc8nO9ZTF3TEVM+2KHoxzDJrLs2Q=="
        })).unwrap()]);
        let mut cache = VaultCache::from_response(&client, response, &mut None)
            .await
            .unwrap();
        assert_eq!(cache.overview().unwrap().failures, 0);
        assert!(cache.overview().unwrap().items[0].organization_id.is_some());
        assert!(cache
            .detail(&client, &cache.overview().unwrap().items[0].id, "")
            .await
            .is_ok());
        cache.discard_overview();
        client
            .internal
            .get_key_store()
            .context_mut()
            .retain_symmetric_keys(|slot| !matches!(slot, SymmetricKeySlotId::Organization(_)));
        assert!(cache.overview().is_err());
        cache.restore_overview(&client).await.unwrap();
        let overview = cache.overview().unwrap();
        assert_eq!(overview.failures, 0);
        assert_eq!(overview.items[0].name, "Synthetic login");
    }

    #[tokio::test]
    #[ignore = "Writes only synthetic public SDK ciphertext for manual native UI smoke tests"]
    async fn write_synthetic_ui_fixture() {
        let output =
            std::env::var("BITWARDEN_FIXTURE_OUTPUT").expect("set an explicit fixture output path");
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let mut response = response_for(&client, test_view()).await;
        let mut guarded = test_view();
        guarded.name = "Synthetic protected item".to_owned();
        guarded.reprompt = bitwarden_vault::CipherRepromptType::Password;
        response
            .ciphers
            .as_mut()
            .unwrap()
            .extend(response_for(&client, guarded).await.ciphers.unwrap());
        std::fs::write(output, serde_json::to_vec_pretty(&response).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn official_v2_blob_ciphertext_can_be_read_without_using_legacy_name_fields() {
        let client = Client::init_test_account(test_bitwarden_com_account_v2()).await;
        let mut response = response_for(&client, test_view()).await;
        response.profile.as_mut().unwrap().key = None;
        assert!(response.ciphers.as_ref().unwrap()[0].data.is_some());
        assert!(response.ciphers.as_ref().unwrap()[0].name.is_none());
        let cache = VaultCache::from_response(&client, response, &mut None)
            .await
            .unwrap();
        assert_eq!(cache.overview().unwrap().items[0].name, "Synthetic login");
    }
}
