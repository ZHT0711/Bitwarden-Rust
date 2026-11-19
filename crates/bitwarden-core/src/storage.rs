use serde_json;
use thiserror::Error;

use crate::{
    crypto::{decrypt_snapshot, encrypt_snapshot, EncryptedSnapshot, LocalKey},
    VaultSnapshot,
};

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid local encryption salt")]
    InvalidSalt,
    #[error("key derivation failed")]
    KeyDerivationFailed,
    #[error("cipher initialization failed")]
    CipherInitFailed,
    #[error("snapshot encryption failed")]
    EncryptionFailed,
    #[error("snapshot decryption failed")]
    DecryptionFailed,
    #[error("snapshot is invalid")]
    InvalidSnapshot,
    #[error("snapshot version {0} is unsupported")]
    UnsupportedSnapshotVersion(u8),
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub fn encode_snapshot(key: &LocalKey, vault: &VaultSnapshot) -> Result<String, CoreError> {
    let plaintext = serde_json::to_vec(vault)?;
    let encrypted = encrypt_snapshot(key, &plaintext)?;
    Ok(serde_json::to_string_pretty(&encrypted)?)
}

pub fn decode_snapshot(key: &LocalKey, encoded: &str) -> Result<VaultSnapshot, CoreError> {
    let encrypted: EncryptedSnapshot = serde_json::from_str(encoded)?;
    let plaintext = decrypt_snapshot(key, &encrypted)?;
    Ok(serde_json::from_slice(&plaintext)?)
}

pub trait VaultRepository {
    fn load(&self, key: &LocalKey) -> Result<VaultSnapshot, CoreError>;
    fn save(&self, key: &LocalKey, vault: &VaultSnapshot) -> Result<(), CoreError>;
}

#[cfg(test)]
mod tests {
    use super::{decode_snapshot, encode_snapshot};
    use crate::{LocalKeyDeriver, VaultSnapshot};

    #[test]
    fn encrypted_snapshot_round_trips() {
        let key = LocalKeyDeriver::derive("correct horse battery staple", "demo@example.com")
            .expect("key derivation should succeed");
        let original = VaultSnapshot::demo();
        let encoded = encode_snapshot(&key, &original).expect("snapshot should encrypt");
        let decoded = decode_snapshot(&key, &encoded).expect("snapshot should decrypt");
        assert_eq!(decoded.ciphers.len(), original.ciphers.len());
        assert_eq!(decoded.folders.len(), original.folders.len());
    }
}
