use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::storage::CoreError;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[derive(Clone)]
pub struct LocalKey(Zeroizing<[u8; KEY_LEN]>);

impl LocalKey {
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for LocalKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub struct LocalKeyDeriver;

impl LocalKeyDeriver {
    pub fn derive(password: &str, salt: &str) -> Result<LocalKey, CoreError> {
        let salt = SaltString::encode_b64(salt.as_bytes()).map_err(|_| CoreError::InvalidSalt)?;
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| CoreError::KeyDerivationFailed)?;
        let digest = hash.hash.ok_or(CoreError::KeyDerivationFailed)?;
        let raw = digest.as_bytes();
        if raw.len() < KEY_LEN {
            return Err(CoreError::KeyDerivationFailed);
        }
        let mut key = [0_u8; KEY_LEN];
        key.copy_from_slice(&raw[..KEY_LEN]);
        Ok(LocalKey(Zeroizing::new(key)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedSnapshot {
    pub version: u8,
    pub nonce: String,
    pub ciphertext: String,
}

pub fn encrypt_snapshot(key: &LocalKey, plaintext: &[u8]) -> Result<EncryptedSnapshot, CoreError> {
    let cipher =
        Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| CoreError::CipherInitFailed)?;
    let mut nonce = [0_u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| CoreError::EncryptionFailed)?;
    Ok(EncryptedSnapshot {
        version: 1,
        nonce: BASE64.encode(nonce),
        ciphertext: BASE64.encode(ciphertext),
    })
}

pub fn decrypt_snapshot(
    key: &LocalKey,
    snapshot: &EncryptedSnapshot,
) -> Result<Vec<u8>, CoreError> {
    if snapshot.version != 1 {
        return Err(CoreError::UnsupportedSnapshotVersion(snapshot.version));
    }
    let nonce = BASE64
        .decode(&snapshot.nonce)
        .map_err(|_| CoreError::InvalidSnapshot)?;
    let ciphertext = BASE64
        .decode(&snapshot.ciphertext)
        .map_err(|_| CoreError::InvalidSnapshot)?;
    if nonce.len() != NONCE_LEN {
        return Err(CoreError::InvalidSnapshot);
    }
    let cipher =
        Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| CoreError::CipherInitFailed)?;
    cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| CoreError::DecryptionFailed)
}
