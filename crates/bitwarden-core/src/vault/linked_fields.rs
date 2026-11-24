//! Resolve only the selected cipher's linked custom fields, never a search/list projection.
use std::borrow::Cow;

use bitwarden_vault::{CipherType, CipherView, FieldView, IdentityView};

pub(super) struct LinkedValue<'a> {
    pub value: Cow<'a, str>,
    pub sensitive: bool,
}

pub(super) fn resolve<'a>(view: &'a CipherView, field: &FieldView) -> Option<LinkedValue<'a>> {
    // The pinned SDK does not re-export LinkedIdType. Its supported numeric conversion
    // serializes just the enum number, not the cipher or any of its plaintext properties.
    let id = u32::from(field.linked_id?);
    let sensitive = matches!(id, 101 | 303 | 305 | 412 | 414 | 415);
    if sensitive && (!view.view_password || view.partial) {
        return None;
    }
    let value = match (view.r#type, id) {
        (CipherType::Login, 100) => view.login.as_ref()?.username.as_deref(),
        (CipherType::Login, 101) => view.login.as_ref()?.password.as_deref(),
        (CipherType::Card, 300) => view.card.as_ref()?.cardholder_name.as_deref(),
        (CipherType::Card, 301) => view.card.as_ref()?.exp_month.as_deref(),
        (CipherType::Card, 302) => view.card.as_ref()?.exp_year.as_deref(),
        (CipherType::Card, 303) => view.card.as_ref()?.code.as_deref(),
        (CipherType::Card, 304) => view.card.as_ref()?.brand.as_deref(),
        (CipherType::Card, 305) => view.card.as_ref()?.number.as_deref(),
        (CipherType::Identity, 400) => view.identity.as_ref()?.title.as_deref(),
        (CipherType::Identity, 401) => view.identity.as_ref()?.middle_name.as_deref(),
        (CipherType::Identity, 402) => view.identity.as_ref()?.address1.as_deref(),
        (CipherType::Identity, 403) => view.identity.as_ref()?.address2.as_deref(),
        (CipherType::Identity, 404) => view.identity.as_ref()?.address3.as_deref(),
        (CipherType::Identity, 405) => view.identity.as_ref()?.city.as_deref(),
        (CipherType::Identity, 406) => view.identity.as_ref()?.state.as_deref(),
        (CipherType::Identity, 407) => view.identity.as_ref()?.postal_code.as_deref(),
        (CipherType::Identity, 408) => view.identity.as_ref()?.country.as_deref(),
        (CipherType::Identity, 409) => view.identity.as_ref()?.company.as_deref(),
        (CipherType::Identity, 410) => view.identity.as_ref()?.email.as_deref(),
        (CipherType::Identity, 411) => view.identity.as_ref()?.phone.as_deref(),
        (CipherType::Identity, 412) => view.identity.as_ref()?.ssn.as_deref(),
        (CipherType::Identity, 413) => view.identity.as_ref()?.username.as_deref(),
        (CipherType::Identity, 414) => view.identity.as_ref()?.passport_number.as_deref(),
        (CipherType::Identity, 415) => view.identity.as_ref()?.license_number.as_deref(),
        (CipherType::Identity, 416) => view.identity.as_ref()?.first_name.as_deref(),
        (CipherType::Identity, 417) => view.identity.as_ref()?.last_name.as_deref(),
        (CipherType::Identity, 418) => {
            return full_name(view.identity.as_ref()?).map(|value| LinkedValue {
                value,
                sensitive: false,
            });
        }
        // Never use a linked field's stale literal value, or a different cipher type's payload.
        _ => return None,
    }?;
    (!value.is_empty()).then_some(LinkedValue {
        value: Cow::Borrowed(value),
        sensitive,
    })
}

// Official desktop-v2026.11.0 IdentityView.fullName: title/first/middle/last,
// ignore whitespace-only components, join raw components, trim only the final boundaries.
// ECMAScript trim includes BOM and excludes NEL, unlike Rust's char::is_whitespace.
fn name_whitespace(ch: char) -> bool {
    (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}'
}

fn full_name(identity: &IdentityView) -> Option<Cow<'_, str>> {
    let parts = [
        identity.title.as_deref(),
        identity.first_name.as_deref(),
        identity.middle_name.as_deref(),
        identity.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim_matches(name_whitespace).is_empty());
    let count = parts.clone().count();
    if count == 0 {
        return None;
    }
    if count == 1 {
        return parts
            .into_iter()
            .next()
            .map(|part| Cow::Borrowed(part.trim_matches(name_whitespace)));
    }
    let capacity = parts.clone().map(str::len).sum::<usize>() + count - 1;
    let mut name = String::with_capacity(capacity);
    for (index, part) in parts.enumerate() {
        if index != 0 {
            name.push(' ');
        }
        let part = if index == 0 {
            part.trim_start_matches(name_whitespace)
        } else if index + 1 == count {
            part.trim_end_matches(name_whitespace)
        } else {
            part
        };
        name.push_str(part);
    }
    Some(Cow::Owned(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{display_detail, tests::test_view, VaultCache, VaultError};
    use bitwarden_vault::FieldType;
    use bw_sdk_core::{client::test_accounts::test_bitwarden_com_account, Client};
    use serde_json::{json, Value};

    // Independent numeric/property/value oracle, not the production resolver's mapping.
    const LOGIN: &[(u32, &str, &str, bool)] = &[
        (100, "username", "fixture-user", false),
        (101, "password", "synthetic-password-DO-NOT-USE", true),
    ];
    const CARD: &[(u32, &str, &str, bool)] = &[
        (300, "cardholderName", "Public Cardholder", false),
        (301, "expMonth", "09", false),
        (302, "expYear", "2031", false),
        (303, "code", "synthetic-security-code", true),
        (304, "brand", "Visa", false),
        (305, "number", "synthetic-card-number", true),
    ];
    const IDENTITY: &[(u32, &str, &str, bool)] = &[
        (400, "title", "Dr", false),
        (401, "middleName", "Public", false),
        (402, "address1", "Synthetic Street One", false),
        (403, "address2", "Synthetic Street Two", false),
        (404, "address3", "Synthetic Street Three", false),
        (405, "city", "Synthetic City", false),
        (406, "state", "Synthetic State", false),
        (407, "postalCode", "00123", false),
        (408, "country", "Synthetic Country", false),
        (409, "company", "Synthetic Company", false),
        (410, "email", "public@example.invalid", false),
        (411, "phone", "synthetic-phone", false),
        (412, "ssn", "synthetic-ssn", true),
        (413, "username", "synthetic-identity-user", false),
        (414, "passportNumber", "synthetic-passport-number", true),
        (415, "licenseNumber", "synthetic-license-number", true),
        (416, "firstName", "Sample", false),
        (417, "lastName", "Person", false),
        (418, "fullName", "Dr Sample Public Person", false),
    ];

    fn cases(kind: u8) -> &'static [(u32, &'static str, &'static str, bool)] {
        match kind {
            1 => LOGIN,
            3 => CARD,
            4 => IDENTITY,
            _ => panic!("not a linked-field fixture type"),
        }
    }

    fn fixture(kind: u8) -> CipherView {
        let mut value = serde_json::to_value(test_view()).unwrap();
        value["type"] = json!(kind);
        value["login"] = Value::Null;
        let property = match kind {
            1 => "login",
            3 => "card",
            4 => "identity",
            _ => unreachable!(),
        };
        value[property] = json!({});
        for &(_, key, target, _) in cases(kind) {
            if key != "fullName" {
                value[property][key] = json!(target);
            }
        }
        value["fields"] = json!(cases(kind)
            .iter()
            .map(|&(id, _, _, _)| json!({
                "name": format!("linked-{id}"), "value": "stale-linked-storage-value",
                "type": 3, "linkedId": id
            }))
            .collect::<Vec<_>>());
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn all_27_links_resolve_the_correct_typed_target_and_sensitivity() {
        for kind in [1, 3, 4] {
            let detail = display_detail(fixture(kind));
            for &(id, _, value, sensitive) in cases(kind) {
                let field = detail
                    .fields
                    .iter()
                    .find(|field| field.label == format!("linked-{id}"))
                    .unwrap();
                assert_eq!(field.value, value, "linked id {id}");
                assert_eq!(field.sensitive, sensitive, "linked id {id}");
            }
            assert!(!serde_json::to_string(&detail)
                .unwrap()
                .contains("stale-linked-storage-value"));
        }
    }

    #[test]
    fn linked_secret_targets_are_stripped_before_ffi_for_both_permission_limits() {
        for kind in [1, 3, 4] {
            for (partial, view_password) in [(false, false), (true, true), (true, false)] {
                let mut view = fixture(kind);
                view.partial = partial;
                view.view_password = view_password;
                let detail = display_detail(view);
                let json = serde_json::to_string(&detail).unwrap();
                for &(id, _, value, sensitive) in cases(kind) {
                    assert_eq!(
                        detail
                            .fields
                            .iter()
                            .any(|field| field.label == format!("linked-{id}")),
                        !sensitive
                    );
                    if sensitive {
                        assert!(!json.contains(value), "secret linked id {id} reached FFI");
                    }
                }
                assert!(detail.fields.iter().all(|field| !field.sensitive));
            }
        }
    }

    #[test]
    fn linked_targets_do_not_fall_back_to_stored_values_or_other_type_payloads() {
        let mut view = fixture(1);
        view.card = fixture(3).card;
        view.fields = Some(
            serde_json::from_value(json!([
                {"name":"missing-link", "type":3, "value":"do-not-use-this-fallback"},
                {"name":"wrong-type", "type":3, "linkedId":305, "value":"do-not-use-this-fallback"},
                {"name":"live-user", "type":3, "linkedId":100, "value":"do-not-use-this-fallback"}
            ]))
            .unwrap(),
        );
        let detail = display_detail(view);
        assert!(!detail
            .fields
            .iter()
            .any(|field| matches!(field.label.as_str(), "missing-link" | "wrong-type")));
        assert_eq!(
            detail
                .fields
                .iter()
                .find(|field| field.label == "live-user")
                .unwrap()
                .value,
            "fixture-user"
        );
        assert!(!serde_json::to_string(&detail)
            .unwrap()
            .contains("do-not-use-this-fallback"));
        // Even populated foreign payloads cannot satisfy another cipher type's links.
        for (kind, id) in [(1, 305), (1, 418), (3, 100), (3, 418), (4, 100), (4, 305)] {
            let mut view = fixture(kind);
            view.login = fixture(1).login;
            view.card = fixture(3).card;
            view.identity = fixture(4).identity;
            view.fields = Some(
                serde_json::from_value(json!([{
                    "name":"wrong-type", "type":3, "linkedId":id, "value":"do-not-use-this-fallback"
                }]))
                .unwrap(),
            );
            assert!(!display_detail(view)
                .fields
                .iter()
                .any(|field| field.label == "wrong-type"));
        }
        // Unknown numeric IDs are rejected by the SDK, not interpreted as safe literal fields.
        assert!(serde_json::from_value::<FieldView>(json!({"type":3,"linkedId":999})).is_err());
    }

    #[test]
    fn missing_and_empty_linked_targets_remain_absent() {
        for kind in [1, 3, 4] {
            let view = fixture(kind);
            for target in [Value::Null, json!("")] {
                let mut raw = serde_json::to_value(&view).unwrap();
                let property = match kind {
                    1 => "login",
                    3 => "card",
                    _ => "identity",
                };
                for &(_, key, _, _) in cases(kind) {
                    if key != "fullName" {
                        raw[property][key] = target.clone();
                    }
                }
                let empty = display_detail(serde_json::from_value(raw).unwrap());
                assert!(!empty
                    .fields
                    .iter()
                    .any(|field| field.label.starts_with("linked-")));
            }
            let mut missing = view;
            missing.login = None;
            missing.card = None;
            missing.identity = None;
            assert!(!display_detail(missing)
                .fields
                .iter()
                .any(|field| field.label.starts_with("linked-")));
        }
    }

    #[test]
    fn ordinary_hidden_and_boolean_fields_keep_their_existing_semantics() {
        let mut view = fixture(1);
        view.fields = Some(
            serde_json::from_value(json!([
                {"name":"text", "type":0, "linkedId":101, "value":"ordinary literal"},
                {"name":"hidden", "type":1, "value":"ordinary hidden"},
                {"name":"boolean", "type":2, "value":"false"},
                {"name":"duplicate", "type":3, "linkedId":100},
                {"name":"duplicate", "type":3, "linkedId":101}
            ]))
            .unwrap(),
        );
        let detail = display_detail(view);
        let fields = &detail.fields;
        assert!(fields.iter().any(|field| field.label == "text"
            && field.value == "ordinary literal"
            && !field.sensitive));
        assert!(fields.iter().any(|field| field.label == "hidden"
            && field.value == "ordinary hidden"
            && field.sensitive));
        assert!(fields
            .iter()
            .any(|field| field.label == "boolean" && field.value == "false" && !field.sensitive));
        assert_eq!(
            fields
                .iter()
                .filter(|field| field.label == "duplicate")
                .count(),
            2
        );
    }

    #[test]
    fn full_name_matches_pinned_desktop_title_spacing_and_unicode_semantics() {
        let examples = [
            (json!({}), None),
            (
                json!({"title":" \t", "firstName":"", "middleName":"\u{feff}", "lastName":null}),
                None,
            ),
            (
                json!({"title":" Dr ", "firstName":" Sample ", "middleName":"Public", "lastName":" Person "}),
                Some("Dr   Sample  Public  Person"),
            ),
            (
                json!({"title":"Dr", "firstName":"Sample", "lastName":"Person"}),
                Some("Dr Sample Person"),
            ),
            (json!({"firstName":"\u{feff}单独😀\u{a0}"}), Some("单独😀")),
            (json!({"firstName":"\u{85}"}), Some("\u{85}")),
            (
                json!({"title":"\u{feff}", "firstName":" \u{2003}", "lastName":"尾名"}),
                Some("尾名"),
            ),
        ];
        for (raw, expected) in examples {
            let identity = serde_json::from_value(raw).unwrap();
            assert_eq!(full_name(&identity).as_deref(), expected);
        }
    }

    #[test]
    fn scalar_links_borrow_native_strings_and_only_composite_names_need_a_buffer() {
        for kind in [1, 3, 4] {
            let view = fixture(kind);
            for field in view.fields.as_ref().unwrap() {
                let resolved = resolve(&view, field).unwrap();
                let composite = u32::from(field.linked_id.unwrap()) == 418;
                assert_eq!(matches!(resolved.value, Cow::Owned(_)), composite);
            }
        }
        let identity = serde_json::from_value(json!({"firstName":"  Only Name  "})).unwrap();
        assert!(matches!(
            full_name(&identity),
            Some(Cow::Borrowed("Only Name"))
        ));
    }

    #[test]
    fn refreshing_a_source_changes_the_link_without_flattening_the_association() {
        let mut view = fixture(1);
        let before = serde_json::to_value(&view.fields).unwrap();
        view.login.as_mut().unwrap().username = Some("new-live-username".to_owned());
        let detail = display_detail(view.clone());
        assert_eq!(
            detail
                .fields
                .iter()
                .find(|field| field.label == "linked-100")
                .unwrap()
                .value,
            "new-live-username"
        );
        assert_eq!(serde_json::to_value(&view.fields).unwrap(), before);
        assert_eq!(view.fields.as_ref().unwrap()[0].r#type, FieldType::Linked);
    }

    #[tokio::test]
    async fn actual_sdk_encrypt_decrypt_projects_links_without_enlarging_overview() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        for kind in [1, 3, 4] {
            let cache = VaultCache::from_response(
                &client,
                crate::vault::tests::response_for(&client, fixture(kind)).await,
                &mut None,
            )
            .await
            .unwrap();
            let overview = cache.overview().unwrap();
            let serialized = serde_json::to_value(overview).unwrap();
            assert!(serialized["items"][0].get("fields").is_none());
            let detail = cache
                .detail(&client, &overview.items[0].id, "")
                .await
                .unwrap();
            for &(id, _, value, sensitive) in cases(kind) {
                let field = detail
                    .fields
                    .iter()
                    .find(|field| field.label == format!("linked-{id}"))
                    .unwrap();
                assert_eq!((field.value.as_str(), field.sensitive), (value, sensitive));
            }
        }
    }

    #[tokio::test]
    async fn linked_password_detail_still_requires_actual_master_password_reprompt() {
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let mut view = fixture(1);
        view.reprompt = bitwarden_vault::CipherRepromptType::Password;
        let cache = VaultCache::from_response(
            &client,
            crate::vault::tests::response_for(&client, view).await,
            &mut None,
        )
        .await
        .unwrap();
        let id = &cache.overview().unwrap().items[0].id;
        assert!(matches!(
            cache.detail(&client, id, "").await,
            Err(VaultError::PasswordRequired)
        ));
        assert!(matches!(
            cache.detail(&client, id, "wrong").await,
            Err(VaultError::WrongPassword)
        ));
        let detail = cache.detail(&client, id, "asdfasdfasdf").await.unwrap();
        assert!(detail
            .fields
            .iter()
            .any(|field| field.label == "linked-101" && field.sensitive));
    }

    #[tokio::test]
    #[ignore = "Writes public synthetic SDK ciphertext for isolated linked-field native/UI checks"]
    async fn write_synthetic_linked_field_fixture() {
        let output =
            std::env::var("BITWARDEN_FIXTURE_OUTPUT").expect("set explicit fixture output");
        let client = Client::init_test_account(test_bitwarden_com_account()).await;
        let mut response = crate::vault::tests::response_for(&client, fixture(1)).await;
        for (kind, name, hidden, partial, reprompt) in [
            (3, "Synthetic linked card", false, false, false),
            (4, "Synthetic linked identity", false, false, false),
            (1, "Synthetic protected item", false, false, true),
            (1, "Synthetic hidden links", true, false, false),
            (1, "Synthetic partial links", false, true, false),
        ] {
            let mut view = fixture(kind);
            view.name = name.to_owned();
            if partial {
                view.organization_id =
                    Some("1bc9ac1e-f5aa-45f2-94bf-b181009709b8".parse().unwrap());
            }
            if reprompt {
                view.reprompt = bitwarden_vault::CipherRepromptType::Password;
            }
            // The SDK correctly refuses to encrypt restricted views. Encrypt a normal
            // synthetic cipher first, then apply the server's unencrypted access flags.
            let cipher = crate::vault::tests::response_for(&client, view)
                .await
                .ciphers
                .unwrap()
                .remove(0);
            let mut raw = serde_json::to_value(cipher).unwrap();
            raw["viewPassword"] = json!(!hidden);
            if partial {
                // A real PAM view is driven by partialData, not a client-side `partial`
                // flag. The reduced envelope carries only the encrypted allowlisted name.
                assert!(raw["name"].is_string());
                raw["partialData"] =
                    json!(serde_json::to_string(&json!({"name":raw["name"]})).unwrap());
                for property in ["login", "fields", "notes", "passwordHistory", "data"] {
                    raw[property] = Value::Null;
                }
            }
            response
                .ciphers
                .as_mut()
                .unwrap()
                .push(serde_json::from_value(raw).unwrap());
        }
        // Public wrapped organization key from the same pinned SDK test account.
        response.profile.as_mut().unwrap().organizations = Some(vec![serde_json::from_value(json!({
            "id":"1bc9ac1e-f5aa-45f2-94bf-b181009709b8",
            "key":"4.rY01mZFXHOsBAg5Fq4gyXuklWfm6mQASm42DJpx05a+e2mmp+P5W6r54WU2hlREX0uoTxyP91bKKwickSPdCQQ58J45LXHdr9t2uzOYyjVzpzebFcdMw1eElR9W2DW8wEk9+mvtWvKwu7yTebzND+46y1nRMoFydi5zPVLSlJEf81qZZ4Uh1UUMLwXz+NRWfixnGXgq2wRq1bH0n3mqDhayiG4LJKgGdDjWXC8W8MMXDYx24SIJrJu9KiNEMprJE+XVF9nQVNijNAjlWBqkDpsfaWTUfeVLRLctfAqW1blsmIv4RQ91PupYJZDNc8nO9ZTF3TEVM+2KHoxzDJrLs2Q=="
        })).unwrap()]);
        let mut json = serde_json::to_value(response).unwrap();
        json["linkedFieldFixture"] = json!({"folders":0,"items":6,"links":27,"synthetic":true});
        std::fs::write(output, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    }
}
