use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(feature = "internal")]
use tracing::info;

use crate::auth::{
    api::response::IdentityTokenResponse, login::response::two_factor::TwoFactorProviders,
};
#[cfg(feature = "internal")]
use crate::{
    Client,
    auth::{api::request::PasswordTokenRequest, login::LoginError, login::TwoFactorRequest},
    client::LoginMethod,
    key_management::{MasterPasswordAuthenticationData, UserDecryptionData},
};

#[cfg(feature = "internal")]
pub(crate) async fn login_password(
    client: &Client,
    input: &PasswordLoginRequest,
) -> Result<(PasswordLoginResponse, Option<PasswordLoginUnlockData>), LoginError> {
    use bitwarden_crypto::EncString;

    use crate::{client::UserLoginMethod, require};

    info!("password logging in");

    let kdf = client.auth().prelogin(input.email.clone()).await?;

    let master_password_authentication =
        MasterPasswordAuthenticationData::derive(&input.password, &kdf, &input.email)?;

    let password_hash = master_password_authentication
        .master_password_authentication_hash
        .to_string();

    let response = request_identity_tokens(client, input, &password_hash).await?;
    let mut unlock_data = None;

    if let IdentityTokenResponse::Authenticated(r) = &response {
        use crate::key_management::account_cryptographic_state::WrappedAccountCryptographicState;

        client
            .internal
            .set_tokens(
                r.access_token.clone(),
                r.refresh_token.clone(),
                r.expires_in,
            )
            .await;

        let private_key: EncString = require!(&r.private_key).parse()?;

        let user_key_state = WrappedAccountCryptographicState::V1 { private_key };

        let master_password_unlock = r
            .user_decryption_options
            .as_ref()
            .map(UserDecryptionData::try_from)
            .transpose()?
            .and_then(|user_decryption| user_decryption.master_password_unlock);
        if let Some(master_password_unlock) = master_password_unlock {
            client
                .internal
                .initialize_user_crypto_master_password_unlock(
                    &input.password,
                    master_password_unlock.clone(),
                    user_key_state.clone(),
                    &None,
                )?;

            client
                .internal
                .set_login_method(LoginMethod::User(UserLoginMethod::Username {
                    client_id: "web".to_owned(),
                    email: master_password_unlock.salt.clone(),
                    kdf: master_password_unlock.kdf.clone(),
                }))
                .await;
            unlock_data = Some(PasswordLoginUnlockData {
                master_password: master_password_unlock,
                account: user_key_state,
            });
        }
    }

    Ok((PasswordLoginResponse::process_response(response), unlock_data))
}

/// Server-wrapped account keys from a successful password login, for in-process unlock.
#[cfg(feature = "internal")]
pub struct PasswordLoginUnlockData {
    /// KDF parameters and master-password-wrapped user key used during login.
    pub master_password: crate::key_management::MasterPasswordUnlockData,
    /// The encrypted account keys used during login.
    pub account: crate::key_management::account_cryptographic_state::WrappedAccountCryptographicState,
}

#[cfg(feature = "internal")]
impl PasswordLoginUnlockData {
    /// Reinitialize crypto on the same client after its key store has been cleared.
    pub fn unlock(
        &self,
        client: &Client,
        password: &str,
    ) -> Result<(), crate::client::encryption_settings::EncryptionSettingsError> {
        client.internal.initialize_user_crypto_master_password_unlock(
            password,
            self.master_password.clone(),
            self.account.clone(),
            &None,
        )
    }
}

#[cfg(feature = "internal")]
async fn request_identity_tokens(
    client: &Client,
    input: &PasswordLoginRequest,
    password_hash: &str,
) -> Result<IdentityTokenResponse, LoginError> {
    use crate::DeviceType;

    let config = client.internal.get_api_configurations();
    PasswordTokenRequest::new(
        &input.email,
        password_hash,
        DeviceType::ChromeBrowser,
        "b86dd6ab-4265-4ddf-a7f1-eb28d5677f33",
        &input.two_factor,
    )
    .send(&config.identity_config)
    .await
}

/// Login to Bitwarden with Username and Password
#[cfg(feature = "internal")]
#[derive(Serialize, Deserialize, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordLoginRequest {
    /// Bitwarden account email address
    pub email: String,
    /// Bitwarden account master password
    pub password: String,
    /// Two-factor authentication
    pub two_factor: Option<TwoFactorRequest>,
}

#[allow(missing_docs)]
#[derive(Serialize, Deserialize, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordLoginResponse {
    pub authenticated: bool,
    /// TODO: What does this do?
    pub reset_master_password: bool,
    /// Whether or not the user is required to update their master password
    pub force_password_reset: bool,
    /// The available two factor authentication options. Present only when authentication fails due
    /// to requiring a second authentication factor.
    pub two_factor: Option<TwoFactorProviders>,
}

impl PasswordLoginResponse {
    pub(crate) fn process_response(response: IdentityTokenResponse) -> PasswordLoginResponse {
        match response {
            IdentityTokenResponse::Authenticated(success) => PasswordLoginResponse {
                authenticated: true,
                reset_master_password: success.reset_master_password,
                force_password_reset: success.force_password_reset,
                two_factor: None,
            },
            IdentityTokenResponse::Payload(_) => PasswordLoginResponse {
                authenticated: true,
                reset_master_password: false,
                force_password_reset: false,
                two_factor: None,
            },
            IdentityTokenResponse::TwoFactorRequired(two_factor) => PasswordLoginResponse {
                authenticated: false,
                reset_master_password: false,
                force_password_reset: false,
                two_factor: Some(two_factor.two_factor_providers.into()),
            },
            IdentityTokenResponse::Refreshed(_) => {
                unreachable!("Got a `refresh_token` answer to a login request")
            }
        }
    }
}
