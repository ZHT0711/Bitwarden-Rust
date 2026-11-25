//! Complete item-scale UI fixtures, not folder-scale fixtures with only two ciphers.
use super::{
    tests::{response_for, test_view},
    *,
};

fn item_id(index: usize) -> uuid::Uuid {
    uuid::Uuid::from_u128(0xbfff0000000040008000000000000000 + index as u128)
}

async fn response(client: &Client, count: usize) -> SyncResponseModel {
    assert!(count >= 2);
    let mut result = response_for(client, test_view()).await;
    let mut ciphers = Vec::with_capacity(count);
    for index in 0..count {
        let mut view = test_view();
        view.id = Some(item_id(index).to_string().parse().unwrap());
        view.name = format!("Synthetic entry {index:05}");
        view.login.as_mut().unwrap().username = Some(format!("user{index:05}"));
        view.favorite = index % 2 == 0;
        if index + 1 == count {
            view.reprompt = bitwarden_vault::CipherRepromptType::Password;
        }
        ciphers.extend(response_for(client, view).await.ciphers.unwrap());
    }
    result.ciphers = Some(ciphers);
    result
}

#[tokio::test]
async fn complete_item_fixture_round_trips_and_keeps_secrets_out_of_metadata() {
    let client =
        Client::init_test_account(bw_sdk_core::client::test_accounts::test_bitwarden_com_account())
            .await;
    let encrypted = response(&client, 12).await;
    let json = serde_json::to_string(&encrypted).unwrap();
    assert!(!json.contains("Synthetic entry"));
    assert!(!json.contains("synthetic-password-DO-NOT-USE"));
    let cache = VaultCache::from_response(&client, encrypted, &mut None)
        .await
        .unwrap();
    let overview = cache.overview().unwrap();
    assert_eq!(overview.items.len(), 12);
    assert_eq!(overview.failures, 0);
    assert_eq!(
        overview.items.iter().filter(|item| item.favorite).count(),
        6
    );
    let last = overview
        .items
        .iter()
        .find(|item| item.id == item_id(11).to_string())
        .unwrap();
    assert_eq!(last.name, "Synthetic entry 00011");
    assert!(last.reprompt);
    let projection = serde_json::to_string(overview).unwrap();
    assert!(!projection.contains("synthetic-password-DO-NOT-USE"));
    assert!(!projection.contains("Synthetic note"));
}

#[tokio::test]
#[ignore = "Writes 10000 public SDK encrypted items for isolated complete-vault UI measurement"]
async fn write_synthetic_large_vault_ui_fixture() {
    let output =
        std::env::var("BITWARDEN_FIXTURE_OUTPUT").expect("set an explicit fixture output path");
    let client =
        Client::init_test_account(bw_sdk_core::client::test_accounts::test_bitwarden_com_account())
            .await;
    let mut value = serde_json::to_value(response(&client, 10_000).await).unwrap();
    value["vaultStress"] = serde_json::json!({
        "synthetic": true, "items": 10000, "folders": 0,
        "firstId": item_id(0), "lastId": item_id(9999),
        "firstName": "Synthetic entry 00000", "lastName": "Synthetic entry 09999",
        "query": "entry 00042 user00042", "queryCount": 1
    });
    std::fs::write(output, serde_json::to_vec(&value).unwrap()).unwrap();
}
