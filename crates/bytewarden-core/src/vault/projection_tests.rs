//! Incremental metadata must agree with a complete SDK projection, including failure counts.
use super::*;

fn damage_name(cipher: &mut Cipher) {
    cipher.name = Some("2.AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAA==|AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".parse().unwrap());
}

async fn assert_matches_full_rebuild(cache: &mut VaultCache, client: &Client, failures: usize) {
    assert_eq!(cache.overview().unwrap().failures, failures);
    let incremental = serde_json::to_value(cache.overview().unwrap()).unwrap();
    cache.rebuild_overview(client).await.unwrap();
    assert_eq!(
        serde_json::to_value(cache.overview().unwrap()).unwrap(),
        incremental
    );
}

#[tokio::test]
async fn single_item_projection_matches_full_rebuild_for_insert_reorder_and_repair() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let views = (0..16)
        .map(|index| {
            let mut view = seed_view();
            view.name = format!("Synthetic item {index:02}");
            view
        })
        .collect();
    let mut cache = seeded_cache(&client, views, false).await;
    let id = cache.overview().unwrap().items[7].id.clone();
    let damaged_id = cache.overview().unwrap().items[15].id.clone();
    let intact = cache.find_cipher(&damaged_id).unwrap();
    let mut damaged = intact.clone();
    damage_name(&mut damaged);
    cache
        .ciphers
        .set(damaged.id.unwrap(), damaged)
        .await
        .unwrap();
    // Model unrelated parse/folder errors as well as an individual decryption failure.
    cache.parse_failures = 2;
    cache.rebuild_overview(&client).await.unwrap();
    assert_eq!(cache.overview().unwrap().failures, 3);

    for name in ["z last", "A first", "middle", "MIDDLE"] {
        let cipher = cache.find_cipher(&id).unwrap();
        let mut view = client.vault().ciphers().decrypt(cipher).await.unwrap();
        view.name = name.to_owned();
        view.favorite = !view.favorite;
        let cipher = client.vault().ciphers().encrypt(view).await.unwrap().cipher;
        cache.ciphers.set(cipher.id.unwrap(), cipher).await.unwrap();
        let result = cache.finish_write(&client, Some(id.clone())).await.unwrap();
        assert_eq!(result.id.as_deref(), Some(id.as_str()));
        assert_eq!(
            result
                .overview
                .items
                .iter()
                .filter(|item| item.id == id)
                .count(),
            1
        );
        assert_matches_full_rebuild(&mut cache, &client, 3).await;
    }

    for name in ["0 first insert", "zz last insert", "middle insert"] {
        let mut view = seed_view();
        view.name = name.to_owned();
        let cipher = client.vault().ciphers().encrypt(view).await.unwrap().cipher;
        let id = cipher.id.unwrap();
        cache.ciphers.set(id, cipher).await.unwrap();
        cache
            .finish_write(&client, Some(id.to_string()))
            .await
            .unwrap();
        assert_matches_full_rebuild(&mut cache, &client, 3).await;
    }

    cache.ciphers.set(intact.id.unwrap(), intact).await.unwrap();
    cache.finish_write(&client, Some(damaged_id)).await.unwrap();
    assert_matches_full_rebuild(&mut cache, &client, 2).await;
}

#[tokio::test]
async fn confirmed_removal_updates_only_its_metadata_and_failure_count() {
    let client = Client::init_test_account_with_api_client(
        test_bitwarden_com_account(),
        ApiClient::new_mocked(|_| {}),
    )
    .await;
    let mut cache = seeded_cache(&client, vec![seed_view(), seed_view()], false).await;
    let id = cache.overview().unwrap().items[0].id.clone();
    let damaged_id = cache.overview().unwrap().items[1].id.clone();
    let mut damaged = cache.find_cipher(&damaged_id).unwrap();
    damage_name(&mut damaged);
    cache
        .ciphers
        .set(damaged.id.unwrap(), damaged)
        .await
        .unwrap();
    cache.parse_failures = 2;
    cache.rebuild_overview(&client).await.unwrap();
    assert_eq!(cache.overview().unwrap().failures, 3);

    cache
        .ciphers
        .remove(damaged_id.parse().unwrap())
        .await
        .unwrap();
    let removed = cache.finish_removal(&damaged_id).unwrap();
    assert!(removed.id.is_none());
    assert_matches_full_rebuild(&mut cache, &client, 2).await;
    cache.ciphers.remove(id.parse().unwrap()).await.unwrap();
    let removed = cache.finish_removal(&id).unwrap();
    assert!(removed.overview.items.is_empty());
    assert_matches_full_rebuild(&mut cache, &client, 2).await;
}

#[tokio::test]
async fn unprojectable_or_unconfirmed_changes_preserve_the_previous_overview() {
    for mode in ["damaged", "missing", "missing-id", "not-removed"] {
        let client = Client::init_test_account_with_api_client(
            test_bitwarden_com_account(),
            ApiClient::new_mocked(|_| {}),
        )
        .await;
        let mut cache = seeded_cache(&client, vec![seed_view()], false).await;
        let id = cache.overview().unwrap().items[0].id.clone();
        let before = serde_json::to_value(cache.overview().unwrap()).unwrap();
        let result = match mode {
            "damaged" => {
                let mut damaged = cache.find_cipher(&id).unwrap();
                damage_name(&mut damaged);
                cache
                    .ciphers
                    .set(damaged.id.unwrap(), damaged)
                    .await
                    .unwrap();
                cache.finish_write(&client, Some(id)).await
            }
            "missing" => {
                cache.ciphers.remove(id.parse().unwrap()).await.unwrap();
                cache.finish_write(&client, Some(id)).await
            }
            "missing-id" => cache.finish_write(&client, None).await,
            "not-removed" => cache.finish_removal(&id),
            _ => unreachable!(),
        };
        assert!(matches!(result, Err(VaultError::WriteUncertain)), "{mode}");
        assert!(cache.needs_reconciliation());
        assert_eq!(
            serde_json::to_value(cache.overview().unwrap()).unwrap(),
            before
        );
    }
}
