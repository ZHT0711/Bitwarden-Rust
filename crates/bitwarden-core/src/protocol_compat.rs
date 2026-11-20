use async_trait::async_trait;
use bitwarden_api_api::models::UserDecryptionResponseModel;
use reqwest::ResponseBuilderExt;
use serde_json::{Map, Value};

pub(crate) enum ResponseKind {
    Prelogin,
    Token,
    Sync,
}

pub(crate) struct SyncCompatibility;

#[async_trait]
impl reqwest_middleware::Middleware for SyncCompatibility {
    async fn handle(
        &self,
        request: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let is_sync =
            request.method() == reqwest::Method::GET && request.url().path().ends_with("/sync");
        let response = next.run(request, extensions).await?;
        if is_sync && response.status().is_success() {
            normalize(response, ResponseKind::Sync).await
        } else {
            Ok(response)
        }
    }
}

/// NodeWarden returns both camelCase and PascalCase names for several protocol fields.
/// The SDK model treats those as aliases and would reject the response as a duplicate field.
pub(crate) async fn normalize(
    response: reqwest::Response,
    kind: ResponseKind,
) -> reqwest_middleware::Result<reqwest::Response> {
    if !response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
    {
        return Ok(response);
    }
    let mut builder = http::Response::builder()
        .status(response.status())
        .version(response.version())
        .url(response.url().clone());
    if let Some(headers) = builder.headers_mut() {
        *headers = response.headers().clone();
    }
    let original = response.bytes().await?;
    let Ok(Value::Object(mut fields)) = serde_json::from_slice(&original) else {
        return builder
            .body(original)
            .map(Into::into)
            .map_err(reqwest_middleware::Error::middleware);
    };
    let changed = match kind {
        ResponseKind::Prelogin => {
            coalesce(&mut fields, "kdfSettings", "KdfSettings")
                | coalesce(&mut fields, "salt", "Salt")
        }
        ResponseKind::Token => normalize_token(&mut fields),
        ResponseKind::Sync => normalize_sync(&mut fields),
    };
    if !changed {
        return builder
            .body(original)
            .map(Into::into)
            .map_err(reqwest_middleware::Error::middleware);
    }
    if let Some(headers) = builder.headers_mut() {
        headers.remove(http::header::CONTENT_LENGTH);
        headers.remove(http::header::CONTENT_ENCODING);
    }
    builder
        .body(Value::Object(fields).to_string())
        .map(Into::into)
        .map_err(reqwest_middleware::Error::middleware)
}

fn normalize_sync(fields: &mut Map<String, Value>) -> bool {
    let mut changed =
        remove_object_aliases(fields) | coalesce(fields, "userDecryption", "UserDecryption");
    if let (Some(current), Some(legacy)) =
        (fields.get("userDecryption"), fields.get("UserDecryption"))
    {
        // The two NodeWarden shapes share the wrapped key, but only one carries extra unlock
        // options. Compare typed data and retain the additional fields when they agree.
        let current =
            serde_json::from_value::<Option<UserDecryptionResponseModel>>(current.clone());
        let legacy = serde_json::from_value::<Option<UserDecryptionResponseModel>>(legacy.clone());
        if let (Ok(current), Ok(legacy)) = (current, legacy) {
            if let Some(merged) = merge_unlock_options(current, legacy) {
                fields.remove("UserDecryption");
                fields.insert(
                    "userDecryption".to_owned(),
                    serde_json::to_value(merged)
                        .expect("the generated response model serializes as JSON"),
                );
                changed = true;
            }
        }
    }
    changed
}

fn merge_unlock_options(
    current: Option<UserDecryptionResponseModel>,
    legacy: Option<UserDecryptionResponseModel>,
) -> Option<UserDecryptionResponseModel> {
    let (Some(current), Some(legacy)) = (&current, &legacy) else {
        return current.or(legacy);
    };
    Some(UserDecryptionResponseModel {
        master_password_unlock: merge_field(
            &current.master_password_unlock,
            &legacy.master_password_unlock,
        )?,
        web_authn_prf_options: merge_field(
            &current.web_authn_prf_options,
            &legacy.web_authn_prf_options,
        )?,
        v2_upgrade_token: merge_field(&current.v2_upgrade_token, &legacy.v2_upgrade_token)?,
        user_key_id: merge_field(&current.user_key_id, &legacy.user_key_id)?,
    })
}

fn merge_field<T: Clone + PartialEq>(current: &Option<T>, legacy: &Option<T>) -> Option<Option<T>> {
    match (current, legacy) {
        (Some(current), Some(legacy)) if current != legacy => None,
        _ => Some(current.as_ref().or(legacy.as_ref()).cloned()),
    }
}

fn remove_object_aliases(fields: &mut Map<String, Value>) -> bool {
    fn visit(value: &mut Value) -> bool {
        match value {
            Value::Object(object) => remove_object_aliases(object),
            Value::Array(elements) => elements
                .iter_mut()
                .fold(false, |changed, val| changed | visit(val)),
            _ => false,
        }
    }
    let changed = coalesce(fields, "object", "Object");
    fields
        .values_mut()
        .fold(changed, |changed, val| changed | visit(val))
}

fn normalize_token(fields: &mut Map<String, Value>) -> bool {
    let mut changed = coalesce(fields, "userDecryptionOptions", "UserDecryptionOptions")
        | coalesce(fields, "twoFactorProviders2", "TwoFactorProviders2");
    let decryption_key = if fields.contains_key("userDecryptionOptions") {
        "userDecryptionOptions"
    } else {
        "UserDecryptionOptions"
    };
    if let Some(Value::Object(options)) = fields.get_mut(decryption_key) {
        changed |= coalesce(options, "masterPasswordUnlock", "MasterPasswordUnlock");
    }
    let providers_key = if fields.contains_key("twoFactorProviders2") {
        "twoFactorProviders2"
    } else {
        "TwoFactorProviders2"
    };
    if let Some(Value::Object(providers)) = fields.get_mut(providers_key) {
        if let Some(provider) = providers.get_mut("0").filter(|value| value.is_null()) {
            // A null value for a listed authenticator means it has no provider-specific fields.
            *provider = Value::Object(Map::new());
            changed = true;
        }
    }
    changed
}

fn coalesce(fields: &mut Map<String, Value>, primary: &str, alias: &str) -> bool {
    let (Some(preferred), Some(alternate)) = (fields.get(primary), fields.get(alias)) else {
        return false;
    };
    if preferred.is_null() {
        let alternate = fields.remove(alias).expect("the alias exists");
        fields.insert(primary.to_owned(), alternate);
    } else if alternate.is_null() || equivalent(preferred, alternate) {
        fields.remove(alias);
    } else {
        // Keep conflicting values for the SDK to reject.
        return false;
    }
    true
}

fn equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.iter().any(|(candidate, other)| {
                        key.eq_ignore_ascii_case(candidate) && equivalent(value, other)
                    })
                })
        }
        _ => left == right,
    }
}
