mod auth_request;
mod crypto;
mod ffi;
#[cfg(windows)]
mod local_cache;
mod model;
#[cfg(windows)]
mod passkey_provider;
mod protocol_compat;
mod storage;
mod vault;

pub use crypto::{EncryptedSnapshot, LocalKey, LocalKeyDeriver};
pub use ffi::desktop;
pub use model::{Cipher, CipherType, Folder, Login, SecureNote, VaultSnapshot};
pub use storage::{decode_snapshot, encode_snapshot, CoreError, VaultRepository};
