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
    let response_status = response.status();
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
        ResponseKind::Token => normalize_token(&mut fields, response_status),
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

fn normalize_token(fields: &mut Map<String, Value>, status: reqwest::StatusCode) -> bool {
    let mut changed = coalesce(fields, "userDecryptionOptions", "UserDecryptionOptions")
        | coalesce(fields, "twoFactorProviders2", "TwoFactorProviders2")
        | coalesce(fields, "twoFactorToken", "TwoFactorToken");
    let decryption_key = if fields.contains_key("userDecryptionOptions") {
        "userDecryptionOptions"
    } else {
        "UserDecryptionOptions"
    };
    if let Some(Value::Object(options)) = fields.get_mut(decryption_key) {
        changed |= coalesce(options, "masterPasswordUnlock", "MasterPasswordUnlock");
    }
    if status == reqwest::StatusCode::BAD_REQUEST {
        changed |= normalize_two_factor_challenge(fields);
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

fn normalize_two_factor_challenge(fields: &mut Map<String, Value>) -> bool {
    if fields
        .get("error")
        .or_else(|| fields.get("Error"))
        .and_then(Value::as_str)
        != Some("invalid_grant")
    {
        return false;
    }
    let mut changed = false;
    if !fields.contains_key("twoFactorProviders2") && !fields.contains_key("TwoFactorProviders2") {
        let custom = fields
            .get("customResponse")
            .or_else(|| fields.get("CustomResponse"))
            .and_then(Value::as_object);
        let provider_map = custom
            .and_then(|custom| {
                custom
                    .get("twoFactorProviders2")
                    .or_else(|| custom.get("TwoFactorProviders2"))
            })
            .filter(|value| value.is_object())
            .cloned();
        let legacy = fields
            .get("twoFactorProviders")
            .or_else(|| fields.get("TwoFactorProviders"))
            .or_else(|| {
                custom.and_then(|custom| {
                    custom
                        .get("twoFactorProviders")
                        .or_else(|| custom.get("TwoFactorProviders"))
                })
            });
        // Only the authenticator has no provider-specific challenge parameters. Never invent
        // email, WebAuthn or other provider data from an old list of provider IDs.
        let authenticator = legacy.and_then(Value::as_array).is_some_and(|list| {
            list.iter()
                .any(|id| id.as_u64() == Some(0) || id.as_str() == Some("0"))
        });
        if let Some(providers) =
            provider_map.or_else(|| authenticator.then(|| serde_json::json!({ "0": {} })))
        {
            fields.insert("twoFactorProviders2".to_owned(), providers);
            changed = true;
        }
    }
    if fields
        .get("twoFactorProviders2")
        .or_else(|| fields.get("TwoFactorProviders2"))
        .is_some_and(Value::is_object)
    {
        for (canonical, alternate) in [
            ("error", "Error"),
            ("error_description", "ErrorDescription"),
        ] {
            if !fields.contains_key(canonical) {
                if let Some(value) = fields
                    .get(alternate)
                    .filter(|value| value.is_string())
                    .cloned()
                {
                    fields.insert(canonical.to_owned(), value);
                    changed = true;
                }
            }
        }
        if !fields.contains_key("error_description") {
            fields.insert(
                "error_description".to_owned(),
                Value::String("Two factor required.".to_owned()),
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token(mut value: Value, status: reqwest::StatusCode) -> Value {
        normalize_token(value.as_object_mut().unwrap(), status);
        value
    }

    #[test]
    fn modern_nodewarden_challenge_keeps_its_authenticator_and_other_methods() {
        let normalized = token(
            json!({
                "error":"invalid_grant", "error_description":"Two factor required.",
                "TwoFactorProviders":["0", "3"], "TwoFactorProviders2":{"0": null, "3":{"Nfc":true}}
            }),
            reqwest::StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            normalized["TwoFactorProviders2"],
            json!({"0":{}, "3":{"Nfc":true}})
        );
    }

    #[test]
    fn legacy_discriminator_and_nested_details_are_normalized_without_inventing_providers() {
        let legacy = token(
            json!({
                "Error":"invalid_grant", "ErrorDescription":"Two factor required.",
                "CustomResponse":{"TwoFactorProviders":[0, "1", "7"]}
            }),
            reqwest::StatusCode::BAD_REQUEST,
        );
        assert_eq!(legacy["error"], "invalid_grant");
        assert_eq!(legacy["error_description"], "Two factor required.");
        assert_eq!(legacy["twoFactorProviders2"], json!({"0":{}}));

        let current = token(
            json!({
                "error":"invalid_grant", "TwoFactorProviders":[0], "TwoFactorProviders2":{"7":{}}
            }),
            reqwest::StatusCode::BAD_REQUEST,
        );
        assert_eq!(current["TwoFactorProviders2"], json!({"7":{}}));
        assert!(!current
            .as_object()
            .unwrap()
            .contains_key("twoFactorProviders2"));
    }

    #[test]
    fn token_failures_or_non_challenge_http_statuses_never_turn_into_legacy_challenges() {
        let values = [
            (
                json!({"error":"invalid_grant", "error_description":"Two-step token is invalid. Try again."}),
                reqwest::StatusCode::BAD_REQUEST,
            ),
            (
                json!({"error":"invalid_request", "TwoFactorProviders":[0]}),
                reqwest::StatusCode::BAD_REQUEST,
            ),
            (
                json!({"Error":"invalid_grant", "TwoFactorProviders":[0]}),
                reqwest::StatusCode::TOO_MANY_REQUESTS,
            ),
        ];
        for (value, status) in values {
            assert_eq!(token(value.clone(), status), value);
        }
    }
}
