//! Shared encrypted folder storage for the vault cache and SDK folder client.
use std::{collections::HashMap, sync::RwLock};

use bitwarden_state::repository::{Repository, RepositoryError};
use bitwarden_vault::{Folder, FolderId};

#[derive(Default)]
pub(super) struct FolderRepository {
    folders: RwLock<HashMap<String, Folder>>,
}

impl FolderRepository {
    pub(super) fn from_folders(folders: Vec<Folder>) -> Result<Self, RepositoryError> {
        let mut values = HashMap::with_capacity(folders.len());
        for folder in folders {
            let id = folder
                .id
                .as_ref()
                .ok_or_else(|| {
                    RepositoryError::Internal("Folder repository entry has no id.".to_owned())
                })?
                .to_string();
            if values.insert(id, folder).is_some() {
                return Err(RepositoryError::Internal(
                    "Duplicate folder repository entry.".to_owned(),
                ));
            }
        }
        Ok(Self {
            folders: RwLock::new(values),
        })
    }

    pub(super) fn snapshot(&self) -> Result<Vec<Folder>, RepositoryError> {
        Ok(self
            .folders
            .read()
            .map_err(|_| unavailable())?
            .values()
            .cloned()
            .collect())
    }
}

fn unavailable() -> RepositoryError {
    RepositoryError::Internal("Folder repository is unavailable.".to_owned())
}

fn validate_key(key: &FolderId, folder: &Folder) -> Result<(), RepositoryError> {
    if folder.id.as_ref() != Some(key) {
        return Err(RepositoryError::Internal(
            "Invalid folder repository entry.".to_owned(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl Repository<Folder> for FolderRepository {
    async fn get(&self, key: FolderId) -> Result<Option<Folder>, RepositoryError> {
        Ok(self
            .folders
            .read()
            .map_err(|_| unavailable())?
            .get(&key.to_string())
            .cloned())
    }

    async fn list(&self) -> Result<Vec<Folder>, RepositoryError> {
        self.snapshot()
    }

    async fn set(&self, key: FolderId, value: Folder) -> Result<(), RepositoryError> {
        validate_key(&key, &value)?;
        self.folders
            .write()
            .map_err(|_| unavailable())?
            .insert(key.to_string(), value);
        Ok(())
    }

    async fn set_bulk(&self, values: Vec<(FolderId, Folder)>) -> Result<(), RepositoryError> {
        let mut replacement = HashMap::with_capacity(values.len());
        for (key, folder) in values {
            validate_key(&key, &folder)?;
            if replacement.insert(key.to_string(), folder).is_some() {
                return Err(RepositoryError::Internal(
                    "Duplicate folder repository entry.".to_owned(),
                ));
            }
        }
        self.folders
            .write()
            .map_err(|_| unavailable())?
            .extend(replacement);
        Ok(())
    }

    async fn remove(&self, key: FolderId) -> Result<(), RepositoryError> {
        self.folders
            .write()
            .map_err(|_| unavailable())?
            .remove(&key.to_string());
        Ok(())
    }

    async fn remove_bulk(&self, keys: Vec<FolderId>) -> Result<(), RepositoryError> {
        let mut folders = self.folders.write().map_err(|_| unavailable())?;
        for key in keys {
            folders.remove(&key.to_string());
        }
        Ok(())
    }

    async fn remove_all(&self) -> Result<(), RepositoryError> {
        self.folders.write().map_err(|_| unavailable())?.clear();
        Ok(())
    }

    async fn replace_all(&self, values: Vec<(FolderId, Folder)>) -> Result<(), RepositoryError> {
        let mut replacement = HashMap::with_capacity(values.len());
        for (key, folder) in values {
            validate_key(&key, &folder)?;
            if replacement.insert(key.to_string(), folder).is_some() {
                return Err(RepositoryError::Internal(
                    "Duplicate folder repository entry.".to_owned(),
                ));
            }
        }
        *self.folders.write().map_err(|_| unavailable())? = replacement;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn folder(id: FolderId, name: &str) -> Folder {
        serde_json::from_value(json!({
            "id": id.to_string(),
            "name": name,
            "revisionDate": "2026-11-20T00:00:00Z"
        }))
        .unwrap()
    }

    fn id() -> FolderId {
        uuid::Uuid::new_v4().to_string().parse().unwrap()
    }

    #[tokio::test]
    async fn replace_all_is_atomic_and_validates_ids_and_duplicates() {
        let old = id();
        let next = id();
        let wrong = id();
        let repository = FolderRepository::from_folders(vec![folder(old, "old")]).unwrap();
        assert!(repository
            .replace_all(vec![(next, folder(wrong, "wrong"))])
            .await
            .is_err());
        assert_eq!(repository.snapshot().unwrap().len(), 1);
        assert!(repository.get(old).await.unwrap().is_some());

        assert!(repository
            .replace_all(vec![
                (next, folder(next, "first")),
                (next, folder(next, "second"))
            ])
            .await
            .is_err());
        assert_eq!(repository.snapshot().unwrap().len(), 1);
        assert!(repository.get(old).await.unwrap().is_some());

        repository
            .replace_all(vec![(next, folder(next, "new"))])
            .await
            .unwrap();
        assert!(repository.get(old).await.unwrap().is_none());
        assert!(repository.get(next).await.unwrap().is_some());
    }
}
