//! Shared ciphertext storage for the cache and the official SDK's mutation clients.
//! Validation completes before any mutation, including full replacement during sync.
use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
};

use bitwarden_state::repository::{Repository, RepositoryError};
use bitwarden_vault::{Cipher, CipherId, FolderId};

#[derive(Default)]
pub(super) struct CipherRepository {
    ciphers: RwLock<HashMap<String, Cipher>>,
}

impl From<HashMap<String, Cipher>> for CipherRepository {
    /// The sync cache constructs this map from validated, unique cipher ids.
    fn from(ciphers: HashMap<String, Cipher>) -> Self {
        Self {
            ciphers: RwLock::new(ciphers),
        }
    }
}

impl CipherRepository {
    pub(super) fn find(&self, id: &str) -> Result<Option<Cipher>, RepositoryError> {
        Ok(self
            .ciphers
            .read()
            .map_err(|_| unavailable())?
            .get(id)
            .cloned())
    }

    pub(super) fn snapshot(&self) -> Result<Vec<Cipher>, RepositoryError> {
        // Callers need owned ciphertexts, not a second hash table and duplicate string keys.
        Ok(self
            .ciphers
            .read()
            .map_err(|_| unavailable())?
            .values()
            .cloned()
            .collect())
    }

    pub(super) fn clear_folders(&self, deleted: &HashSet<FolderId>) -> Result<(), RepositoryError> {
        for cipher in self
            .ciphers
            .write()
            .map_err(|_| unavailable())?
            .values_mut()
        {
            if cipher.folder_id.is_some_and(|id| deleted.contains(&id)) {
                cipher.folder_id = None;
            }
        }
        Ok(())
    }
}

fn unavailable() -> RepositoryError {
    // A poisoned lock can carry the original panic payload. Never forward it or stored values.
    RepositoryError::Internal("Cipher repository is unavailable.".to_owned())
}

fn invalid_entry() -> RepositoryError {
    RepositoryError::Internal("Invalid cipher repository entry.".to_owned())
}

fn validate_key(key: &CipherId, cipher: &Cipher) -> Result<(), RepositoryError> {
    if cipher.id.as_ref() != Some(key) {
        return Err(invalid_entry());
    }
    Ok(())
}

fn validated_batch(
    values: Vec<(CipherId, Cipher)>,
) -> Result<HashMap<String, Cipher>, RepositoryError> {
    let mut replacement = HashMap::with_capacity(values.len());
    for (key, cipher) in values {
        validate_key(&key, &cipher)?;
        if replacement.insert(key.to_string(), cipher).is_some() {
            return Err(RepositoryError::Internal(
                "Duplicate cipher repository entry.".to_owned(),
            ));
        }
    }
    Ok(replacement)
}

#[async_trait::async_trait]
impl Repository<Cipher> for CipherRepository {
    async fn get(&self, key: CipherId) -> Result<Option<Cipher>, RepositoryError> {
        self.find(&key.to_string())
    }

    async fn list(&self) -> Result<Vec<Cipher>, RepositoryError> {
        self.snapshot()
    }

    async fn set(&self, key: CipherId, value: Cipher) -> Result<(), RepositoryError> {
        validate_key(&key, &value)?;
        self.ciphers
            .write()
            .map_err(|_| unavailable())?
            .insert(key.to_string(), value);
        Ok(())
    }

    async fn set_bulk(&self, values: Vec<(CipherId, Cipher)>) -> Result<(), RepositoryError> {
        let batch = validated_batch(values)?;
        self.ciphers
            .write()
            .map_err(|_| unavailable())?
            .extend(batch);
        Ok(())
    }

    async fn remove(&self, key: CipherId) -> Result<(), RepositoryError> {
        self.ciphers
            .write()
            .map_err(|_| unavailable())?
            .remove(&key.to_string());
        Ok(())
    }

    async fn remove_bulk(&self, keys: Vec<CipherId>) -> Result<(), RepositoryError> {
        let mut ciphers = self.ciphers.write().map_err(|_| unavailable())?;
        for key in keys {
            ciphers.remove(&key.to_string());
        }
        Ok(())
    }

    async fn remove_all(&self) -> Result<(), RepositoryError> {
        self.ciphers.write().map_err(|_| unavailable())?.clear();
        Ok(())
    }

    async fn replace_all(&self, values: Vec<(CipherId, Cipher)>) -> Result<(), RepositoryError> {
        // The SDK default performs remove_all + set_bulk, which can expose empty or partial state.
        // Validate everything separately, then swap the complete map while holding one lock.
        let replacement = validated_batch(values)?;
        *self.ciphers.write().map_err(|_| unavailable())? = replacement;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{panic::AssertUnwindSafe, sync::Arc};

    use serde_json::json;

    use super::*;

    fn id() -> CipherId {
        uuid::Uuid::new_v4().to_string().parse().unwrap()
    }

    fn cipher(id: CipherId, favorite: bool) -> Cipher {
        // No real data or usable encrypted payload is needed to exercise repository semantics.
        serde_json::from_value(json!({
            "id": id.to_string(), "collectionIds": [], "type": 1,
            "favorite": favorite, "reprompt": 0, "organizationUseTotp": false,
            "edit": true, "viewPassword": true,
            "creationDate": "2026-11-20T00:00:00Z",
            "revisionDate": "2026-11-20T00:00:00Z"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn sdk_and_cache_share_one_map_through_the_same_arc() {
        let key = id();
        let repository = Arc::new(CipherRepository::default());
        let sdk: Arc<dyn Repository<Cipher>> = repository.clone();
        sdk.set(key, cipher(key, false)).await.unwrap();
        assert!(!repository.find(&key.to_string()).unwrap().unwrap().favorite);

        repository.set(key, cipher(key, true)).await.unwrap();
        assert!(sdk.get(key).await.unwrap().unwrap().favorite);
        assert_eq!(repository.snapshot().unwrap().len(), 1);
        assert_eq!(sdk.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn sdk_removal_is_visible_to_cache_reads_and_is_idempotent() {
        let first = id();
        let second = id();
        let repository = CipherRepository::from(HashMap::from([
            (first.to_string(), cipher(first, false)),
            (second.to_string(), cipher(second, true)),
        ]));

        repository.remove(first).await.unwrap();
        assert!(repository.find(&first.to_string()).unwrap().is_none());
        assert_eq!(repository.snapshot().unwrap().len(), 1);
        repository
            .remove_bulk(vec![first, second, second])
            .await
            .unwrap();
        assert!(repository.snapshot().unwrap().is_empty());

        repository.set(first, cipher(first, false)).await.unwrap();
        repository.remove_all().await.unwrap();
        assert!(repository.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mismatched_or_missing_ids_do_not_change_existing_entries() {
        let key = id();
        let other = id();
        let repository =
            CipherRepository::from(HashMap::from([(key.to_string(), cipher(key, false))]));

        assert!(repository.set(other, cipher(key, true)).await.is_err());
        let mut missing = cipher(other, true);
        missing.id = None;
        assert!(repository.set(other, missing).await.is_err());
        assert!(repository
            .set_bulk(vec![(key, cipher(key, true)), (other, cipher(key, true))])
            .await
            .is_err());
        assert_eq!(repository.snapshot().unwrap().len(), 1);
        assert!(!repository.find(&key.to_string()).unwrap().unwrap().favorite);
    }

    #[tokio::test]
    async fn duplicate_bulk_ids_are_rejected_before_mutation() {
        let key = id();
        let repository =
            CipherRepository::from(HashMap::from([(key.to_string(), cipher(key, false))]));
        assert!(repository
            .set_bulk(vec![(key, cipher(key, true)), (key, cipher(key, false))])
            .await
            .is_err());
        assert!(!repository.find(&key.to_string()).unwrap().unwrap().favorite);
    }

    #[tokio::test]
    async fn failed_replace_all_preserves_the_entire_previous_snapshot() {
        let old = id();
        let next = id();
        let wrong = id();
        let repository =
            CipherRepository::from(HashMap::from([(old.to_string(), cipher(old, false))]));
        assert!(repository
            .replace_all(vec![
                (next, cipher(next, true)),
                (wrong, cipher(next, false))
            ])
            .await
            .is_err());
        assert_eq!(repository.snapshot().unwrap().len(), 1);
        assert!(repository.find(&old.to_string()).unwrap().is_some());
        assert!(repository.find(&next.to_string()).unwrap().is_none());

        assert!(repository
            .replace_all(vec![
                (next, cipher(next, true)),
                (next, cipher(next, false))
            ])
            .await
            .is_err());
        assert!(repository.find(&old.to_string()).unwrap().is_some());

        repository
            .replace_all(vec![(next, cipher(next, true))])
            .await
            .unwrap();
        assert!(repository.find(&old.to_string()).unwrap().is_none());
        assert!(
            repository
                .find(&next.to_string())
                .unwrap()
                .unwrap()
                .favorite
        );
        repository.replace_all(vec![]).await.unwrap();
        assert!(repository.snapshot().unwrap().is_empty());
    }

    #[tokio::test]
    async fn poisoned_locks_return_a_fixed_error_without_panic_payloads() {
        let repository = CipherRepository::default();
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = repository.ciphers.write().unwrap();
            panic!("synthetic panic payload must not be forwarded");
        }));
        let error = repository.find(&id().to_string()).unwrap_err();
        assert!(
            matches!(&error, RepositoryError::Internal(message) if message == "Cipher repository is unavailable.")
        );
        assert!(!error.to_string().contains("synthetic panic payload"));
        assert!(repository.snapshot().is_err());
        assert!(repository.list().await.is_err());
        let key = id();
        assert!(repository.set(key, cipher(key, false)).await.is_err());
        assert!(repository.remove_all().await.is_err());
    }
}
