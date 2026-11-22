//! Writes use real SDK encryption and API mocks. No production account or endpoint is used.

#[path = "projection_tests.rs"]
mod projection_tests;

/// Measures the local post-acknowledgement projection, including the returned overview clone.
/// Run explicitly with `cargo test --release measure_confirmed_write_projection -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "local performance measurement; no timing assertion on shared CI hosts"]
async fn measure_confirmed_write_projection() {
    use std::{hint::black_box, time::Instant};

    for size in [100, 1_000, 10_000] {
        let client = Client::init_test_account_with_api_client(
            test_bitwarden_com_account(),
            ApiClient::new_mocked(|_| {}),
        )
        .await;
        let views = (0..size)
            .map(|index| {
                let mut view = seed_view();
                view.name = format!("Synthetic benchmark {index:05}");
                view
            })
            .collect();
        let mut cache = seeded_cache(&client, views, false).await;
        let id = cache.overview().unwrap().items[size / 2].id.clone();
        for _ in 0..3 {
            black_box(cache.finish_write(&client, Some(id.clone())).await.unwrap());
        }
        let mut samples = Vec::new();
        for _ in 0..25 {
            let start = Instant::now();
            black_box(cache.finish_write(&client, Some(id.clone())).await.unwrap());
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "write_projection items={size} iterations={} median_ms={:.4} p95_ms={:.4}",
            samples.len(),
            samples[samples.len() / 2],
            samples[23]
        );
    }
}

use std::sync::{Arc, Mutex};

use bitwarden_api_api::{
    apis::ApiClient,
    models::{
        CipherDetailsResponseModel, CipherRequestModel, CipherResponseModel, ProfileResponseModel,
        SyncResponseModel,
    },
    ApiError, ResponseContent,
};
use bitwarden_state::repository::Repository;
use bitwarden_vault::{AttachmentView, CipherView, VaultClientExt};
use bw_sdk_core::{client::test_accounts::test_bitwarden_com_account, Client};
use serde_json::{json, Value};

use super::*;

const ORIGINAL_REVISION: &str = "2026-11-20T01:02:03.456Z";

fn registration_request() -> crate::vault::passkeys::PasskeyOperation {
    crate::vault::passkeys::PasskeyOperation {
        creating: true,
        rp_id: "passkeys.example.invalid".to_owned(),
        user_name: Some("synthetic-passkey-user".to_owned()),
        user_display_name: Some("Synthetic user".to_owned()),
        user_handle: vec![42; 16],
        client_data_hash: vec![17; 32],
        credential_ids: vec![],
        algorithms: vec![-7],
    }
}

#[tokio::test]
async fn passkey_creation_requires_a_confirmed_persisted_private_key() {
    for lose_ack in [true, false] {
        let client = Client::init_test_account_with_api_client(
            test_bitwarden_com_account(),
            ApiClient::new_mocked(move |mock| {
                mock.ciphers_api
                    .expect_post()
                    .times(1)
                    .returning(move |request| {
                        if lose_ack {
                            return Err(api_failure(503, "Synthetic unconfirmed write"));
                        }
                        let mut response =
                            response_for_request(request.unwrap(), uuid::Uuid::new_v4());
                        response.login.as_mut().unwrap().fido2_credentials = None;
                        Ok(response)
                    });
            }),
        )
        .await;
        let mut cache = seeded_cache(&client, vec![], false).await;
        let op = registration_request();
        assert!(matches!(
            cache.perform_passkey(&client, &op, "", "", 0, "").await,
            Err(VaultError::WriteUncertain)
        ));
        assert!(cache.needs_reconciliation());
        assert!(matches!(
            cache.perform_passkey(&client, &op, "", "", 0, "").await,
            Err(VaultError::NotSynced)
        ));
    }
}

#[tokio::test]
async fn passkey_registration_persists_encrypted_key_then_signs_bound_assertions() {
    use crate::vault::passkeys::PasskeyResult;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use p256::{
        ecdsa::{signature::Verifier, Signature, VerifyingKey},
        pkcs8::DecodePrivateKey,
        SecretKey,
    };
    use sha2::{Digest, Sha256};
    let sent = Arc::new(Mutex::new(Vec::new()));
    let capture = sent.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            let capture = capture.clone();
            mock.ciphers_api
                .expect_post()
                .times(1)
                .returning(move |request| {
                    let request = request.unwrap();
                    capture
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(&request).unwrap());
                    Ok(response_for_request(request, uuid::Uuid::new_v4()))
                });
        }),
    )
    .await;
    let mut cache = seeded_cache(&client, vec![], false).await;
    let op = registration_request();
    let result = cache
        .perform_passkey(&client, &op, "", "", 0, "")
        .await
        .unwrap();
    let PasskeyResult::Created {
        authenticator_data,
        credential_id,
    } = result
    else {
        panic!("expected registration")
    };
    assert_eq!(
        &authenticator_data[..32],
        &Sha256::digest(op.rp_id.as_bytes())[..]
    );
    assert_eq!(authenticator_data[32] & 5, 5);
    let choices = cache
        .passkey_choices(
            &client,
            &crate::vault::passkeys::PasskeyOperation {
                creating: false,
                ..op.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(choices.len(), 1);
    let choice = &choices[0];
    let cipher = cache.find_cipher(&choice.id).unwrap();
    let view = client.vault().ciphers().decrypt(cipher).await.unwrap();
    let stored = view.get_fido2_credentials().remove(0);
    assert_eq!(
        bitwarden_fido::string_to_guid_bytes(&stored.credential_id).unwrap(),
        credential_id
    );
    let key_bytes = URL_SAFE_NO_PAD.decode(&stored.key_value).unwrap();
    let secret = SecretKey::from_pkcs8_der(&key_bytes).unwrap();
    let key = VerifyingKey::from(secret.public_key());
    assert!(!sent.lock().unwrap()[0]
        .to_string()
        .contains(&stored.key_value));
    assert!(!sent.lock().unwrap()[0]
        .to_string()
        .contains("synthetic-passkey-user"));
    let assertion = crate::vault::passkeys::PasskeyOperation {
        creating: false,
        credential_ids: vec![credential_id.clone()],
        client_data_hash: vec![23; 32],
        ..op.clone()
    };
    let result = cache
        .perform_passkey(&client, &assertion, &choice.id, &choice.revision, 0, "")
        .await
        .unwrap();
    let PasskeyResult::Assertion {
        credential_id: returned_id,
        authenticator_data,
        signature,
        user_handle,
    } = result
    else {
        panic!("expected assertion")
    };
    assert_eq!(returned_id, credential_id);
    assert_eq!(user_handle, op.user_handle);
    assert_eq!(authenticator_data[32] & 5, 5);
    let mut signed = authenticator_data;
    signed.extend(&assertion.client_data_hash);
    key.verify(&signed, &Signature::from_der(&signature).unwrap())
        .unwrap();
    signed.push(0);
    assert!(key
        .verify(&signed, &Signature::from_der(&signature).unwrap())
        .is_err());
    let wrong_rp = crate::vault::passkeys::PasskeyOperation {
        rp_id: "other.example.invalid".to_owned(),
        ..assertion.clone()
    };
    assert!(cache
        .passkey_choices(&client, &wrong_rp)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        cache
            .perform_passkey(&client, &wrong_rp, &choice.id, &choice.revision, 0, "")
            .await,
        Err(VaultError::NotFound)
    ));
    let excluded = crate::vault::passkeys::PasskeyOperation {
        credential_ids: vec![credential_id],
        ..op
    };
    assert!(cache
        .perform_passkey(&client, &excluded, "", "", 0, "")
        .await
        .is_err());
    assert_eq!(cache.overview().unwrap().items.len(), 1);
}

#[tokio::test]
async fn passkey_registration_into_existing_login_preserves_secrets_and_checks_revision() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|mock| {
            mock.ciphers_api
                .expect_put()
                .times(1)
                .returning(|id, request| Ok(response_for_request(request.unwrap(), id)));
        }),
    )
    .await;
    let original = seed_view();
    let id = original.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![original], false).await;
    let op = registration_request();
    assert!(matches!(
        cache
            .perform_passkey(&client, &op, &id, "2026-11-19T00:00:00Z", 0, "")
            .await,
        Err(VaultError::Conflict)
    ));
    cache
        .perform_passkey(&client, &op, &id, ORIGINAL_REVISION, 0, "")
        .await
        .unwrap();
    let view = client
        .vault()
        .ciphers()
        .decrypt(cache.find_cipher(&id).unwrap())
        .await
        .unwrap();
    assert_eq!(view.notes.as_deref(), Some("original synthetic notes"));
    let login = view.login.as_ref().unwrap();
    assert_eq!(login.password.as_deref(), Some("original-secret"));
    assert_eq!(login.totp.as_deref(), Some("SYNTHETIC-TOTP"));
    assert_eq!(view.get_fido2_credentials().len(), 1);
    assert!(cache
        .passkey_choices(&client, &op)
        .await
        .unwrap()
        .is_empty());
}
const NEXT_REVISION: &str = "2026-11-20T01:03:04.789Z";
const FIXTURE_ORG: &str = "1bc9ac1e-f5aa-45f2-94bf-b181009709b8";
// Public, synthetic SDK fixture ciphertext, not a real user's key.
const WRAPPED_USER_KEY: &str = "2.Q/2PhzcC7GdeiMHhWguYAQ==|GpqzVdr0go0ug5cZh1n+uixeBC3oC90CIe0hd/HWA/pTRDZ8ane4fmsEIcuc8eMKUt55Y2q/fbNzsYu41YTZzzsJUSeqVjT8/iTQtgnNdpo=|dwI+uyvZ1h/iZ03VQ+/wrGEFYVewBUUl/syYgjsNMbE=";
const WRAPPED_ORG_KEY: &str = "4.rY01mZFXHOsBAg5Fq4gyXuklWfm6mQASm42DJpx05a+e2mmp+P5W6r54WU2hlREX0uoTxyP91bKKwickSPdCQQ58J45LXHdr9t2uzOYyjVzpzebFcdMw1eElR9W2DW8wEk9+mvtWvKwu7yTebzND+46y1nRMoFydi5zPVLSlJEf81qZZ4Uh1UUMLwXz+NRWfixnGXgq2wRq1bH0n3mqDhayiG4LJKgGdDjWXC8W8MMXDYx24SIJrJu9KiNEMprJE+XVF9nQVNijNAjlWBqkDpsfaWTUfeVLRLctfAqW1blsmIv4RQ91PupYJZDNc8nO9ZTF3TEVM+2KHoxzDJrLs2Q==";

fn seed_view() -> CipherView {
    serde_json::from_value(json!({
        "id": uuid::Uuid::new_v4(), "collectionIds": [], "name": "Synthetic original",
        "notes": "original synthetic notes", "type": 1,
        "login": {"username":"original-user","password":"original-secret","totp":"SYNTHETIC-TOTP",
            "autofillOnPageLoad":true, "httpAuthenticationEnabled":true,
            "uris":[{"uri":"https://first.example.invalid","match":0},{"uri":"https://second.example.invalid","match":3}]},
        "favorite":false,"reprompt":0,"organizationUseTotp":false,"edit":true,
        "permissions":{"delete":true,"restore":true},"viewPassword":true,
        "creationDate":"2026-11-20T00:00:00Z","revisionDate":ORIGINAL_REVISION
    })).unwrap()
}

fn response_model(mut value: Value, id: uuid::Uuid, revision: &str) -> CipherResponseModel {
    value["id"] = json!(id);
    value["creationDate"] = json!("2026-11-20T00:00:00Z");
    value["revisionDate"] = json!(revision);
    value["edit"] = json!(true);
    value["viewPassword"] = json!(true);
    value["permissions"] = json!({"delete":true,"restore":true});
    if let Some(attachments) = value.get("attachments2").and_then(Value::as_object) {
        let attachments: Vec<Value> = attachments
            .iter()
            .map(|(id, attachment)| {
                json!({
                    "id":id,"fileName":attachment["fileName"],"key":attachment["key"],
                    "url":"https://attachment.example.invalid/file","size":"12","sizeName":"12 B"
                })
            })
            .collect();
        value["attachments"] = json!(attachments);
    }
    serde_json::from_value(value).unwrap()
}

fn response_for_request(request: CipherRequestModel, id: uuid::Uuid) -> CipherResponseModel {
    response_model(serde_json::to_value(request).unwrap(), id, NEXT_REVISION)
}

fn api_failure(status: u16, message: &str) -> ApiError {
    ApiError::Response(ResponseContent {
        status: status.try_into().unwrap(),
        message: json!({"message":message}).to_string(),
    })
}

async fn seeded_cache(client: &Client, views: Vec<CipherView>, premium: bool) -> VaultCache {
    let mut ciphers = Vec::new();
    for view in views {
        let cipher = client.vault().ciphers().encrypt(view).await.unwrap().cipher;
        let raw: CipherDetailsResponseModel =
            serde_json::from_value(serde_json::to_value(cipher).unwrap()).unwrap();
        ciphers.push(raw);
    }
    let profile: ProfileResponseModel = serde_json::from_value(json!({
        "id":client.internal.get_user_id().unwrap().to_string(),
        "key":WRAPPED_USER_KEY,"premium":premium,
        "organizations":[{"id":FIXTURE_ORG,"key":WRAPPED_ORG_KEY,"usersGetPremium":false}]
    }))
    .unwrap();
    let cache = VaultCache::from_response(
        client,
        SyncResponseModel {
            profile: Some(Box::new(profile)),
            ciphers: Some(ciphers),
            folders: Some(vec![]),
            ..Default::default()
        },
        &mut None,
    )
    .await
    .unwrap();
    cache.activate(client);
    cache
}

fn new_input(kind: u8) -> VaultEditorData {
    let values = match kind {
        1 => {
            json!({"username":"new-user","password":"new-secret-password","totp":"SYNTHETIC-TOTP"})
        }
        2 => json!({}),
        3 => json!({"cardholderName":"Synthetic Holder","number":"4111111111111111","code":"123"}),
        4 => json!({"firstName":"Synthetic","lastName":"Person","ssn":"NOT-REAL"}),
        5 => {
            json!({"privateKey":"SYNTHETIC-SSH-PRIVATE","publicKey":"SYNTHETIC-SSH-PUBLIC","fingerprint":"SYNTHETIC-FINGERPRINT"})
        }
        6 => {
            json!({"bankName":"Synthetic Bank","nameOnAccount":"Synthetic Holder","accountNumber":"123456","pin":"1234"})
        }
        7 => json!({"licenseNumber":"SYNTHETIC-LICENSE","dateOfBirth":"2000-01-02"}),
        8 => {
            json!({"passportNumber":"SYNTHETIC-PASSPORT","issuingCountry":"TEST","expirationDate":"2030-01-02"})
        }
        _ => unreachable!(),
    };
    serde_json::from_value(json!({
        "name":format!("Synthetic type {kind}"),"itemType":kind,
        "notes":"new-secret-notes","values":values,"favorite":true,
        "uris":if kind == 1 { json!([{"uri":"https://new.example.invalid","match":2}]) } else { json!([]) },
        "customFields":[{"name":"test hidden","value":"new-secret-custom","type":1,"linkedId":null}]
    })).unwrap()
}

#[test]
fn editor_input_cannot_inject_native_key_or_passkey_storage() {
    for property in ["key", "fido2Credentials", "passwordHistory", "attachments"] {
        let mut input = serde_json::to_value(new_input(1)).unwrap();
        input[property] = json!("forged native material");
        assert!(serde_json::from_value::<VaultEditorData>(input).is_err());
    }
}

#[tokio::test]
async fn eight_types_are_created_and_edited_through_sdk_encryption() {
    let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
    let create_capture = sent.clone();
    let edit_capture = sent.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            let capture = create_capture.clone();
            mock.ciphers_api
                .expect_post()
                .times(8)
                .returning(move |request| {
                    let request = request.unwrap();
                    capture
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(&request).unwrap());
                    Ok(response_for_request(request, uuid::Uuid::new_v4()))
                });
            let capture = edit_capture.clone();
            mock.ciphers_api
                .expect_put()
                .times(8)
                .returning(move |id, request| {
                    let request = request.unwrap();
                    capture
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(&request).unwrap());
                    Ok(response_for_request(request, id))
                });
        }),
    )
    .await;
    let mut cache = seeded_cache(&client, vec![], false).await;
    for kind in 1..=8 {
        let input = new_input(kind);
        let expected_values = input.values.clone();
        let result = cache.create(&client, input).await.unwrap();
        let id = result.id.unwrap();
        let mut editor = cache.editor(&client, &id, "").await.unwrap();
        assert_eq!(editor.item_type, kind);
        for (name, value) in expected_values {
            assert_eq!(editor.values.get(&name), Some(&value));
        }
        editor.name.push_str(" edited");
        cache.update(&client, editor, "").await.unwrap();
        let editor = cache.editor(&client, &id, "").await.unwrap();
        assert!(editor.name.ends_with(" edited"));
        assert_eq!(cache.overview().unwrap().items.len(), usize::from(kind));
    }
    for request in sent.lock().unwrap().iter() {
        let wire = request.to_string();
        assert!(!wire.contains("new-secret-password"));
        assert!(!wire.contains("new-secret-notes"));
        assert!(!wire.contains("new-secret-custom"));
        assert!(!wire.contains("SYNTHETIC-SSH-PRIVATE"));
        assert!(request
            .get("encryptedFor")
            .and_then(Value::as_str)
            .is_some());
        assert!(request.get("key").and_then(Value::as_str).is_some());
    }
}

#[tokio::test]
async fn full_edit_preserves_native_secrets_ownership_attachment_keys_and_history() {
    let sent = Arc::new(Mutex::new(None::<Value>));
    let capture = sent.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put()
                .times(1)
                .returning(move |id, request| {
                    let request = request.unwrap();
                    *capture.lock().unwrap() = Some(serde_json::to_value(&request).unwrap());
                    Ok(response_for_request(request, id))
                });
        }),
    )
    .await;
    let mut view = seed_view();
    view.organization_id = Some(FIXTURE_ORG.parse().unwrap());
    view.collection_ids = vec![uuid::Uuid::new_v4().to_string().parse().unwrap()];
    view.password_history = Some(
        serde_json::from_value(json!([{
            "password":"OLDER-SYNTHETIC-PASSWORD","lastUsedDate":"2026-11-19T00:00:00Z"
        }]))
        .unwrap(),
    );
    view.fields = Some(
        serde_json::from_value(json!([
            {"name":"secret question","value":"old hidden answer","type":1},
            {"name":"linked username","type":3,"linkedId":100}
        ]))
        .unwrap(),
    );
    view.login.as_mut().unwrap().fido2_credentials = Some(serde_json::from_value(json!([{
        "credentialId":"SYNTHETIC-CREDENTIAL","keyType":"public-key","keyAlgorithm":"ECDSA","keyCurve":"P-256",
        "keyValue":"SYNTHETIC-PASSKEY-PRIVATE","rpId":"example.invalid","counter":"0","discoverable":"true","creationDate":"2026-11-19T00:00:00Z"
    }])).unwrap());
    let cipher = client.vault().ciphers().encrypt(view).await.unwrap().cipher;
    let mut view = client.vault().ciphers().decrypt(cipher).await.unwrap();
    view.attachments = Some(vec![AttachmentView {
        id: Some("synthetic-attachment".to_owned()),
        url: Some("https://attachment.example.invalid/file".to_owned()),
        size: Some("12".to_owned()),
        size_name: Some("12 B".to_owned()),
        file_name: Some("synthetic.txt".to_owned()),
        key: view.key.clone(),
    }]);
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let original = client
        .vault()
        .ciphers()
        .decrypt(cache.find_cipher(&id).unwrap())
        .await
        .unwrap();
    let mut editor = cache.editor(&client, &id, "").await.unwrap();
    let ffi = serde_json::to_value(&editor).unwrap();
    let wire = ffi.to_string();
    assert!(ffi.get("key").is_none());
    assert!(!wire.contains("SYNTHETIC-PASSKEY-PRIVATE"));
    assert!(!wire.contains("OLDER-SYNTHETIC-PASSWORD"));
    assert!(ffi.get("attachments").is_none());
    editor.name = "Renamed synthetic item".to_owned();
    editor
        .values
        .insert("password".to_owned(), "replacement-secret".to_owned());
    editor.custom_fields[0].value = Some("replacement hidden answer".to_owned());
    editor.uris[0].uri = Some("https://changed.example.invalid".to_owned());
    editor.uris[0].r#match = Some(5);
    cache.update(&client, editor, "").await.unwrap();
    let updated = client
        .vault()
        .ciphers()
        .decrypt(cache.find_cipher(&id).unwrap())
        .await
        .unwrap();
    assert_eq!(updated.organization_id, original.organization_id);
    assert_eq!(updated.collection_ids, original.collection_ids);
    assert_eq!(updated.key, original.key);
    assert_eq!(updated.attachments, original.attachments);
    let login = updated.login.as_ref().unwrap();
    let old_login = original.login.as_ref().unwrap();
    assert_eq!(login.fido2_credentials, old_login.fido2_credentials);
    assert_eq!(login.autofill_on_page_load, old_login.autofill_on_page_load);
    assert_eq!(
        login.http_authentication_enabled,
        old_login.http_authentication_enabled
    );
    assert_eq!(login.uris.as_ref().unwrap().len(), 2);
    assert_eq!(
        login.uris.as_ref().unwrap()[0]
            .r#match
            .map(|kind| kind as u8),
        Some(5)
    );
    assert_eq!(
        login.uris.as_ref().unwrap()[1].r#match,
        old_login.uris.as_ref().unwrap()[1].r#match
    );
    let history = updated.password_history.as_ref().unwrap();
    assert!(history
        .iter()
        .any(|entry| entry.password == "original-secret"));
    assert!(history
        .iter()
        .any(|entry| entry.password == "OLDER-SYNTHETIC-PASSWORD"));
    assert!(history
        .iter()
        .any(|entry| entry.password == "secret question: old hidden answer"));
    assert_eq!(
        updated.fields.as_ref().unwrap()[1].linked_id,
        original.fields.as_ref().unwrap()[1].linked_id
    );
    let posted = sent.lock().unwrap();
    let posted = posted.as_ref().unwrap();
    assert!(!posted.to_string().contains("replacement-secret"));
    assert!(posted["lastKnownRevisionDate"]
        .as_str()
        .unwrap()
        .starts_with("2026-11-20T01:02:03"));
}

#[tokio::test]
async fn stale_editor_is_rejected_locally_without_sending_any_write() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let view = seed_view();
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let mut editor = cache.editor(&client, &id, "").await.unwrap();
    editor.revision_date = Some("2026-11-20T01:02:03.455Z".to_owned());
    assert!(matches!(
        cache.update(&client, editor, "").await,
        Err(VaultError::Conflict)
    ));
    assert_eq!(
        cache.editor(&client, &id, "").await.unwrap().name,
        "Synthetic original"
    );
    let mut change_type = cache.editor(&client, &id, "").await.unwrap();
    change_type.item_type = 2;
    change_type.values.clear();
    change_type.uris.clear();
    assert!(matches!(
        cache.update(&client, change_type, "").await,
        Err(VaultError::Validation)
    ));
}

#[tokio::test]
async fn server_conflict_patterns_are_distinguished_from_other_bad_requests() {
    for (status, message, conflict) in [
        (400, "The item cannot be saved because it is out of date. To edit this item, first sync your vault.", true),
        (409, "Revision conflict", true),
        (400, "The name field is invalid.", false),
    ] {
        let client = Client::init_test_account_with_api_client(test_bitwarden_com_account(), ApiClient::new_mocked(move |mock| {
            mock.ciphers_api.expect_put().times(1).returning(move |_, _| Err(api_failure(status, message)));
        })).await;
        let view = seed_view();
        let id = view.id.unwrap().to_string();
        let mut cache = seeded_cache(&client, vec![view], false).await;
        let editor = cache.editor(&client, &id, "").await.unwrap();
        let result = cache.update(&client, editor, "").await;
        if conflict { assert!(matches!(result, Err(VaultError::Conflict))); }
        else {
            assert!(matches!(result, Err(VaultError::Validation)));
            assert!(cache.editor(&client, &id, "").await.is_ok());
        }
        assert_eq!(cache.find_cipher(&id).unwrap().revision_date, ORIGINAL_REVISION.parse::<DateTime<Utc>>().unwrap());
    }
}

#[tokio::test]
async fn backend_permissions_cannot_be_overridden_by_editor_flags() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let view = seed_view();
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let mut editor = cache.editor(&client, &id, "").await.unwrap();
    let mut cipher = cache.find_cipher(&id).unwrap();
    cipher.edit = false;
    cipher.permissions =
        Some(serde_json::from_value(json!({"delete":false,"restore":false})).unwrap());
    cache
        .ciphers
        .set(cipher.id.unwrap(), cipher.clone())
        .await
        .unwrap();
    editor.can_edit = true;
    editor.can_delete = true;
    editor.can_restore = true;
    assert!(matches!(
        cache.update(&client, editor, "").await,
        Err(VaultError::PermissionDenied)
    ));
    assert!(matches!(
        cache
            .action(&client, &id, "trash", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::PermissionDenied)
    ));
    cipher.deleted_date = Some(Utc::now());
    cache
        .ciphers
        .set(cipher.id.unwrap(), cipher.clone())
        .await
        .unwrap();
    assert!(matches!(
        cache
            .action(&client, &id, "restore", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::PermissionDenied)
    ));
    assert!(matches!(
        cache
            .action(&client, &id, "delete", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::PermissionDenied)
    ));
    cipher.deleted_date = None;
    cipher.edit = true;
    cipher.view_password = false;
    cache.ciphers.set(cipher.id.unwrap(), cipher).await.unwrap();
    let mut hidden = cache.editor(&client, &id, "").await.unwrap();
    assert!(!hidden.can_edit);
    assert!(hidden.values.is_empty());
    assert!(hidden.notes.is_none());
    hidden.can_edit = true;
    hidden
        .values
        .insert("password".to_owned(), "forged password".to_owned());
    assert!(matches!(
        cache.update(&client, hidden, "").await,
        Err(VaultError::PermissionDenied)
    ));
}

#[tokio::test]
async fn reprompt_is_checked_again_at_save_and_destructive_actions() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let mut view = seed_view();
    view.reprompt = CipherRepromptType::Password;
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    assert!(matches!(
        cache.editor(&client, &id, "").await,
        Err(VaultError::PasswordRequired)
    ));
    assert!(matches!(
        cache.editor(&client, &id, "wrong synthetic password").await,
        Err(VaultError::WrongPassword)
    ));
    let editor = cache.editor(&client, &id, "asdfasdfasdf").await.unwrap();
    assert!(matches!(
        cache.update(&client, editor, "").await,
        Err(VaultError::PasswordRequired)
    ));
    assert!(matches!(
        cache
            .action(&client, &id, "trash", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::PasswordRequired)
    ));
}

#[tokio::test]
async fn restricted_items_allow_favorite_without_exposing_or_reprompting_for_secrets() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_partial()
                .times(1)
                .returning(move |_, request| {
                    let request = request.unwrap();
                    assert_eq!(request.favorite, Some(true));
                    let mut response = capture.lock().unwrap().clone().unwrap();
                    response.favorite = Some(true);
                    Ok(response)
                });
        }),
    )
    .await;
    let mut view = seed_view();
    // Restricted ciphers are organization-owned; the SDK rejects personal partials.
    view.organization_id = Some(FIXTURE_ORG.parse().unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let mut cipher = cache.find_cipher(&id).unwrap();
    cipher.partial_data =
        Some(json!({"name":cipher.name.as_ref().map(ToString::to_string)}).to_string());
    cipher.edit = false;
    cipher.reprompt = CipherRepromptType::Password;
    *response.lock().unwrap() =
        Some(serde_json::from_value(serde_json::to_value(&cipher).unwrap()).unwrap());
    cache.ciphers.set(cipher.id.unwrap(), cipher).await.unwrap();
    cache
        .action(&client, &id, "favorite", ORIGINAL_REVISION, "")
        .await
        .unwrap();
    assert!(cache.find_cipher(&id).unwrap().favorite);
    let editor = cache.editor(&client, &id, "asdfasdfasdf").await.unwrap();
    assert!(!editor.can_edit);
    assert!(editor.values.is_empty());
    assert!(editor.custom_fields.is_empty());
    assert!(editor.notes.is_none());
    assert!(matches!(
        cache.update(&client, editor, "asdfasdfasdf").await,
        Err(VaultError::PermissionDenied)
    ));
}

#[tokio::test]
async fn archive_requires_premium_but_unarchive_remains_available_after_it_ends() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let archive_response = response.clone();
    let restore_response = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_archive()
                .times(1)
                .returning(move |_| {
                    let mut response = archive_response.lock().unwrap().clone().unwrap();
                    response.archived_date = Some(NEXT_REVISION.to_owned());
                    response.revision_date = Some(NEXT_REVISION.to_owned());
                    Ok(response)
                });
            mock.ciphers_api
                .expect_put_unarchive()
                .times(1)
                .returning(move |_| {
                    let mut response = restore_response.lock().unwrap().clone().unwrap();
                    response.archived_date = None;
                    response.revision_date = Some(NEXT_REVISION.to_owned());
                    Ok(response)
                });
        }),
    )
    .await;
    let mut view = seed_view();
    view.organization_id = Some(FIXTURE_ORG.parse().unwrap());
    view.collection_ids = vec![uuid::Uuid::new_v4().to_string().parse().unwrap()];
    view.local_data =
        Some(serde_json::from_value(json!({"lastUsedDate":ORIGINAL_REVISION})).unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let original = cache.find_cipher(&id).unwrap();
    *response.lock().unwrap() =
        Some(serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap());
    assert!(matches!(
        cache
            .action(&client, &id, "archive", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::PremiumRequired)
    ));
    cache.can_archive = true;
    cache
        .action(&client, &id, "archive", ORIGINAL_REVISION, "")
        .await
        .unwrap();
    let archived = cache.find_cipher(&id).unwrap();
    assert!(archived.archived_date.is_some());
    assert_eq!(archived.collection_ids, original.collection_ids);
    assert_eq!(
        serde_json::to_value(&archived.local_data).unwrap(),
        serde_json::to_value(&original.local_data).unwrap()
    );
    cache.can_archive = false;
    cache
        .action(&client, &id, "unarchive", NEXT_REVISION, "")
        .await
        .unwrap();
    assert!(cache.find_cipher(&id).unwrap().archived_date.is_none());
}

#[tokio::test]
async fn trash_restore_and_permanent_delete_use_their_distinct_sdk_endpoints() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_delete()
                .times(2)
                .returning(|_| Ok(()));
            mock.ciphers_api
                .expect_put_restore()
                .times(1)
                .returning(move |_| {
                    let mut response = capture.lock().unwrap().clone().unwrap();
                    response.deleted_date = None;
                    response.revision_date = Some(NEXT_REVISION.to_owned());
                    Ok(response)
                });
            mock.ciphers_api
                .expect_delete()
                .times(1)
                .returning(|_| Ok(()));
        }),
    )
    .await;
    let view = seed_view();
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    *response.lock().unwrap() = Some(
        serde_json::from_value(serde_json::to_value(cache.find_cipher(&id).unwrap()).unwrap())
            .unwrap(),
    );
    assert!(matches!(
        cache
            .action(&client, &id, "delete", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::PermissionDenied)
    ));
    cache
        .action(&client, &id, "trash", ORIGINAL_REVISION, "")
        .await
        .unwrap();
    assert!(cache.find_cipher(&id).unwrap().deleted_date.is_some());
    assert!(cache.overview().unwrap().items[0].deleted);
    let revision = cache.find_cipher(&id).unwrap().revision_date.to_rfc3339();
    cache
        .action(&client, &id, "restore", &revision, "")
        .await
        .unwrap();
    assert!(cache.find_cipher(&id).unwrap().deleted_date.is_none());
    assert!(!cache.overview().unwrap().items[0].deleted);
    cache
        .action(&client, &id, "trash", NEXT_REVISION, "")
        .await
        .unwrap();
    let revision = cache.find_cipher(&id).unwrap().revision_date.to_rfc3339();
    let result = cache
        .action(&client, &id, "delete", &revision, "")
        .await
        .unwrap();
    assert!(result.id.is_none());
    assert!(result.overview.items.is_empty());
    assert!(cache.ciphers.find(&id).unwrap().is_none());
}

#[tokio::test]
async fn uncertain_create_is_not_automatically_retried_or_allowed_to_duplicate_items() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|mock| {
            mock.ciphers_api.expect_post().times(1).returning(|_| {
                Err(ApiError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Synthetic lost response after dispatch",
                )))
            });
        }),
    )
    .await;
    let mut cache = seeded_cache(&client, vec![], false).await;
    assert!(matches!(
        cache.create(&client, new_input(1)).await,
        Err(VaultError::WriteUncertain)
    ));
    assert!(matches!(
        cache.create(&client, new_input(1)).await,
        Err(VaultError::NotSynced)
    ));
    assert!(cache.write_uncertain);
}

#[tokio::test]
async fn success_response_that_cannot_be_merged_is_also_an_uncertain_write() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|mock| {
            mock.ciphers_api
                .expect_post()
                .times(1)
                .returning(|request| {
                    let mut response = response_for_request(request.unwrap(), uuid::Uuid::new_v4());
                    response.r#type = None;
                    Ok(response)
                });
        }),
    )
    .await;
    let mut cache = seeded_cache(&client, vec![], false).await;
    assert!(matches!(
        cache.create(&client, new_input(1)).await,
        Err(VaultError::WriteUncertain)
    ));
    assert!(matches!(
        cache.create(&client, new_input(1)).await,
        Err(VaultError::NotSynced)
    ));
    assert!(cache.write_uncertain);
}

#[tokio::test]
async fn invalid_input_and_unsynced_writes_never_reach_the_api() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let mut cache = VaultCache::default();
    assert!(matches!(
        cache.create(&client, new_input(1)).await,
        Err(VaultError::NotSynced)
    ));
    let mut cache = seeded_cache(&client, vec![], false).await;
    let mut invalid_key = new_input(1);
    invalid_key.values.insert(
        "fido2Credentials".to_owned(),
        "forged secret material".to_owned(),
    );
    let mut invalid_date = new_input(7);
    invalid_date
        .values
        .insert("dateOfBirth".to_owned(), "not-a-date".to_owned());
    let mut invalid_name = new_input(1);
    invalid_name.name = "  ".to_owned();
    let mut invalid_link = new_input(1);
    invalid_link.custom_fields[0].r#type = 3;
    invalid_link.custom_fields[0].linked_id = None;
    for input in [invalid_key, invalid_date, invalid_name, invalid_link] {
        assert!(matches!(
            cache.create(&client, input).await,
            Err(VaultError::Validation)
        ));
    }
    assert!(!cache.write_uncertain);
    assert!(cache.overview().unwrap().items.is_empty());
}

#[tokio::test]
async fn archive_omitted_security_metadata_does_not_relax_previous_restrictions() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_archive()
                .times(1)
                .returning(move |_| {
                    let mut response = capture.lock().unwrap().clone().unwrap();
                    response.archived_date = Some(NEXT_REVISION.to_owned());
                    response.revision_date = Some(NEXT_REVISION.to_owned());
                    response.view_password = None;
                    response.edit = None;
                    response.reprompt = None;
                    response.permissions = None;
                    Ok(response)
                });
        }),
    )
    .await;
    let mut view = seed_view();
    view.edit = false;
    view.view_password = false;
    view.reprompt = CipherRepromptType::Password;
    view.permissions =
        Some(serde_json::from_value(json!({"delete":false,"restore":false})).unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], true).await;
    *response.lock().unwrap() = Some(
        serde_json::from_value(serde_json::to_value(cache.find_cipher(&id).unwrap()).unwrap())
            .unwrap(),
    );
    cache
        .action(&client, &id, "archive", ORIGINAL_REVISION, "asdfasdfasdf")
        .await
        .unwrap();
    let cached = cache.find_cipher(&id).unwrap();
    assert!(!cached.edit);
    assert!(!cached.view_password);
    assert_eq!(cached.reprompt, CipherRepromptType::Password);
    let permissions = cached.permissions.unwrap();
    assert!(!permissions.delete);
    assert!(!permissions.restore);
    assert!(matches!(
        cache.editor(&client, &id, "").await,
        Err(VaultError::PasswordRequired)
    ));
    let editor = cache.editor(&client, &id, "asdfasdfasdf").await.unwrap();
    assert!(!editor.can_edit);
    assert!(!editor.can_delete);
    assert!(!editor.can_restore);
    assert!(editor.values.is_empty());
    assert!(editor.notes.is_none());
}

#[tokio::test]
async fn restore_omitted_security_metadata_keeps_hidden_password_and_reprompt_requirements() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_restore()
                .times(1)
                .returning(move |_| {
                    let mut response = capture.lock().unwrap().clone().unwrap();
                    response.deleted_date = None;
                    response.revision_date = Some(NEXT_REVISION.to_owned());
                    response.view_password = None;
                    response.edit = None;
                    response.reprompt = None;
                    response.permissions = None;
                    Ok(response)
                });
        }),
    )
    .await;
    let mut view = seed_view();
    view.deleted_date = Some("2026-11-20T00:30:00Z".parse().unwrap());
    view.edit = false;
    view.view_password = false;
    view.reprompt = CipherRepromptType::Password;
    view.permissions =
        Some(serde_json::from_value(json!({"delete":false,"restore":true})).unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    *response.lock().unwrap() = Some(
        serde_json::from_value(serde_json::to_value(cache.find_cipher(&id).unwrap()).unwrap())
            .unwrap(),
    );
    cache
        .action(&client, &id, "restore", ORIGINAL_REVISION, "asdfasdfasdf")
        .await
        .unwrap();
    let restored = cache.find_cipher(&id).unwrap();
    assert!(restored.deleted_date.is_none());
    assert!(!restored.view_password);
    assert!(!restored.edit);
    assert_eq!(restored.reprompt, CipherRepromptType::Password);
    assert!(matches!(
        cache.editor(&client, &id, "").await,
        Err(VaultError::PasswordRequired)
    ));
    let editor = cache.editor(&client, &id, "asdfasdfasdf").await.unwrap();
    assert!(!editor.can_edit);
    assert!(editor.values.is_empty());
    assert!(editor.notes.is_none());
}

#[tokio::test]
async fn archive_explicit_new_server_restrictions_and_partial_data_are_authoritative() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_archive()
                .times(1)
                .returning(move |_| Ok(capture.lock().unwrap().clone().unwrap()));
        }),
    )
    .await;
    let mut view = seed_view();
    view.organization_id = Some(FIXTURE_ORG.parse().unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], true).await;
    let cipher = cache.find_cipher(&id).unwrap();
    let mut restricted: CipherResponseModel =
        serde_json::from_value(serde_json::to_value(&cipher).unwrap()).unwrap();
    restricted.archived_date = Some(NEXT_REVISION.to_owned());
    restricted.revision_date = Some(NEXT_REVISION.to_owned());
    restricted.edit = Some(false);
    restricted.view_password = Some(false);
    restricted.reprompt = Some(serde_json::from_value(json!(1)).unwrap());
    restricted.permissions = Some(Box::new(
        serde_json::from_value(json!({"delete":false,"restore":false})).unwrap(),
    ));
    restricted.partial_data =
        Some(json!({"name":cipher.name.as_ref().map(ToString::to_string)}).to_string());
    *response.lock().unwrap() = Some(restricted);
    cache
        .action(&client, &id, "archive", ORIGINAL_REVISION, "")
        .await
        .unwrap();
    let cached = cache.find_cipher(&id).unwrap();
    assert!(cached.partial_data.is_some());
    assert!(!cached.edit);
    assert!(!cached.view_password);
    assert_eq!(cached.reprompt, CipherRepromptType::Password);
    assert!(cached
        .permissions
        .is_some_and(|permissions| !permissions.delete && !permissions.restore));
    let editor = cache.editor(&client, &id, "asdfasdfasdf").await.unwrap();
    assert!(!editor.can_edit);
    assert!(!editor.can_delete);
    assert!(!editor.can_restore);
    assert!(editor.values.is_empty());
}

#[tokio::test]
async fn archive_never_installs_a_full_write_return_over_a_previously_gated_cipher() {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_archive()
                .times(1)
                .returning(move |_| Ok(capture.lock().unwrap().clone().unwrap()));
        }),
    )
    .await;
    let mut view = seed_view();
    view.organization_id = Some(FIXTURE_ORG.parse().unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], true).await;
    let mut gated = cache.find_cipher(&id).unwrap();
    let mut full: CipherResponseModel =
        serde_json::from_value(serde_json::to_value(&gated).unwrap()).unwrap();
    full.archived_date = Some(NEXT_REVISION.to_owned());
    full.revision_date = Some(NEXT_REVISION.to_owned());
    assert!(full.partial_data.is_none());
    *response.lock().unwrap() = Some(full);
    gated.partial_data =
        Some(json!({"name":gated.name.as_ref().map(ToString::to_string)}).to_string());
    gated.edit = false;
    gated.view_password = false;
    gated.permissions =
        Some(serde_json::from_value(json!({"delete":false,"restore":false})).unwrap());
    gated.login = None;
    gated.notes = None;
    gated.fields = None;
    gated.password_history = None;
    gated.data = None;
    let before = serde_json::to_value(&gated).unwrap();
    cache.ciphers.set(gated.id.unwrap(), gated).await.unwrap();
    assert!(matches!(
        cache
            .action(&client, &id, "archive", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::WriteUncertain)
    ));
    assert!(cache.write_uncertain);
    let cached = cache.find_cipher(&id).unwrap();
    assert_eq!(serde_json::to_value(&cached).unwrap(), before);
    assert!(cached.partial_data.is_some());
    assert!(cached.login.is_none());
    assert!(matches!(
        cache
            .action(&client, &id, "favorite", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::NotSynced)
    ));
}

#[tokio::test]
async fn damaged_attachment_ciphertext_blocks_edit_before_the_api_can_drop_it() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let cipher = client
        .vault()
        .ciphers()
        .encrypt(seed_view())
        .await
        .unwrap()
        .cipher;
    let mut view = client.vault().ciphers().decrypt(cipher).await.unwrap();
    view.attachments = Some(
        ["healthy", "damaged"]
            .into_iter()
            .map(|suffix| AttachmentView {
                id: Some(format!("synthetic-{suffix}")),
                url: Some("https://attachment.example.invalid/file".to_owned()),
                size: Some("12".to_owned()),
                size_name: Some("12 B".to_owned()),
                file_name: Some(format!("{suffix}.txt")),
                key: view.key.clone(),
            })
            .collect(),
    );
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let mut cipher = cache.find_cipher(&id).unwrap();
    cipher.attachments.as_mut().unwrap()[1].key = Some("2.AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".parse().unwrap());
    let before = serde_json::to_value(&cipher).unwrap();
    cache
        .ciphers
        .set(cipher.id.unwrap(), cipher.clone())
        .await
        .unwrap();
    // The official strict decrypt intentionally returns a usable view plus an attachment failure list.
    let decrypted = client.vault().ciphers().decrypt(cipher).await.unwrap();
    assert_eq!(decrypted.attachments.as_ref().unwrap().len(), 1);
    assert_eq!(
        decrypted
            .attachment_decryption_failures
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    let mut editor = cache.editor(&client, &id, "").await.unwrap();
    assert!(!editor.can_edit);
    assert_eq!(
        editor.edit_blocked_reason.as_deref(),
        Some("attachmentUnavailable")
    );
    editor.can_edit = true;
    editor.name = "Attempted rename must not remove attachment".to_owned();
    assert!(matches!(
        cache.update(&client, editor, "").await,
        Err(VaultError::AttachmentUnavailable)
    ));
    assert_eq!(
        serde_json::to_value(cache.find_cipher(&id).unwrap()).unwrap(),
        before
    );
    assert!(!cache.write_uncertain);
}

#[tokio::test]
async fn mutation_cannot_report_success_when_the_changed_item_disappears_from_decrypted_metadata() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|mock| {
            mock.ciphers_api
                .expect_put_delete()
                .times(1)
                .returning(|_| Ok(()));
        }),
    )
    .await;
    let view = seed_view();
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let mut cipher = cache.find_cipher(&id).unwrap();
    cipher.name = Some("2.AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".parse().unwrap());
    cache.ciphers.set(cipher.id.unwrap(), cipher).await.unwrap();
    assert!(matches!(
        cache
            .action(&client, &id, "trash", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::WriteUncertain)
    ));
    assert!(cache.write_uncertain);
    assert!(matches!(
        cache.create(&client, new_input(1)).await,
        Err(VaultError::NotSynced)
    ));
}

#[tokio::test]
async fn restricted_favorite_full_write_return_is_rolled_back_before_any_metadata_is_exposed() {
    assert_restricted_favorite_rejects_full_return(false).await;
}

#[tokio::test]
async fn restricted_favorite_full_return_is_rolled_back_even_when_sdk_decryption_fails() {
    assert_restricted_favorite_rejects_full_return(true).await;
}

async fn assert_restricted_favorite_rejects_full_return(corrupt_return: bool) {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_partial()
                .times(1)
                .returning(move |_, request| {
                    assert_eq!(request.unwrap().favorite, Some(true));
                    Ok(capture.lock().unwrap().clone().unwrap())
                });
        }),
    )
    .await;
    let mut view = seed_view();
    view.organization_id = Some(FIXTURE_ORG.parse().unwrap());
    view.reprompt = CipherRepromptType::Password;
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    let mut gated = cache.find_cipher(&id).unwrap();
    let mut full: CipherResponseModel =
        serde_json::from_value(serde_json::to_value(&gated).unwrap()).unwrap();
    full.favorite = Some(true);
    full.revision_date = Some(NEXT_REVISION.to_owned());
    full.reprompt = Some(serde_json::from_value(json!(0)).unwrap());
    if corrupt_return {
        // SDK edit_partial writes to its repository before decrypting the returned view.
        full.name = Some("2.AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned());
    }
    assert!(full.partial_data.is_none());
    *response.lock().unwrap() = Some(full);
    gated.partial_data =
        Some(json!({"name":gated.name.as_ref().map(ToString::to_string)}).to_string());
    gated.edit = false;
    gated.view_password = false;
    gated.permissions =
        Some(serde_json::from_value(json!({"delete":false,"restore":false})).unwrap());
    gated.login = None;
    gated.notes = None;
    gated.fields = None;
    gated.password_history = None;
    gated.data = None;
    let before_cipher = serde_json::to_value(&gated).unwrap();
    cache.ciphers.set(gated.id.unwrap(), gated).await.unwrap();
    cache.finish_write(&client, Some(id.clone())).await.unwrap();
    let before_overview = serde_json::to_value(cache.overview().unwrap()).unwrap();
    assert!(!before_overview.to_string().contains("original-user"));
    assert!(matches!(
        cache
            .action(&client, &id, "favorite", ORIGINAL_REVISION, "")
            .await,
        Err(VaultError::WriteUncertain)
    ));
    assert!(cache.write_uncertain);
    assert_eq!(
        serde_json::to_value(cache.find_cipher(&id).unwrap()).unwrap(),
        before_cipher
    );
    assert_eq!(
        serde_json::to_value(cache.overview().unwrap()).unwrap(),
        before_overview
    );
    assert!(matches!(
        cache.detail(&client, &id, "asdfasdfasdf").await,
        Err(VaultError::NotSynced)
    ));
}

#[tokio::test]
async fn favorite_omitted_security_metadata_preserves_hidden_fields_and_reprompt_without_prompting_for_favorite(
) {
    let response = Arc::new(Mutex::new(None::<CipherResponseModel>));
    let capture = response.clone();
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(move |mock| {
            mock.ciphers_api
                .expect_put_partial()
                .times(1)
                .returning(move |_, request| {
                    assert_eq!(request.unwrap().favorite, Some(true));
                    let mut response = capture.lock().unwrap().clone().unwrap();
                    response.favorite = Some(true);
                    response.revision_date = Some(NEXT_REVISION.to_owned());
                    response.view_password = None;
                    response.edit = None;
                    response.reprompt = None;
                    response.permissions = None;
                    Ok(response)
                });
        }),
    )
    .await;
    let mut view = seed_view();
    view.edit = false;
    view.view_password = false;
    view.reprompt = CipherRepromptType::Password;
    view.permissions =
        Some(serde_json::from_value(json!({"delete":false,"restore":false})).unwrap());
    let id = view.id.unwrap().to_string();
    let mut cache = seeded_cache(&client, vec![view], false).await;
    *response.lock().unwrap() = Some(
        serde_json::from_value(serde_json::to_value(cache.find_cipher(&id).unwrap()).unwrap())
            .unwrap(),
    );
    cache
        .action(&client, &id, "favorite", ORIGINAL_REVISION, "")
        .await
        .unwrap();
    let cached = cache.find_cipher(&id).unwrap();
    assert!(cached.favorite);
    assert!(!cached.view_password);
    assert!(!cached.edit);
    assert_eq!(cached.reprompt, CipherRepromptType::Password);
    assert!(cached
        .permissions
        .is_some_and(|permissions| !permissions.delete && !permissions.restore));
    assert!(!cache.write_uncertain);
    assert!(matches!(
        cache.editor(&client, &id, "").await,
        Err(VaultError::PasswordRequired)
    ));
    let editor = cache.editor(&client, &id, "asdfasdfasdf").await.unwrap();
    assert!(!editor.can_edit);
    assert!(editor.values.is_empty());
    assert!(editor.notes.is_none());
}
