//! Windows per-user protection for locally persisted, server-encrypted vault snapshots.
use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    },
};
use zeroize::{Zeroize, Zeroizing};

const MAX_STATE_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_DIRECTORY: &str = "vault-cache";
const ENTROPY_PREFIX: &[u8] = b"BitwardenRust/WindowsVaultCache/v1/";
static STORAGE_ROOT: OnceLock<RwLock<Option<CacheRoot>>> = OnceLock::new();

struct CacheRoot {
    path: PathBuf,
    // A single process owns the cache and journal for the lifetime of the native core.
    _writer: File,
}

fn root() -> &'static RwLock<Option<CacheRoot>> {
    STORAGE_ROOT.get_or_init(|| RwLock::new(None))
}

fn acquire_writer(root: &Path) -> std::io::Result<File> {
    let directory = root.join(CACHE_DIRECTORY);
    fs::create_dir_all(&directory)?;
    // FILE_SHARE_READ | FILE_SHARE_WRITE, but no deletion of the lock file while open.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0x3)
        .open(directory.join("writer.lock"))?;
    file.try_lock().map_err(std::io::Error::from)?;
    Ok(file)
}

pub(crate) fn set_root(path: &str) -> bool {
    if path.is_empty() || path.len() > 32_768 {
        return false;
    }
    let candidate = PathBuf::from(path);
    if !candidate.is_absolute() || !candidate.is_dir() {
        return false;
    }
    let Ok(candidate) = fs::canonicalize(candidate) else {
        return false;
    };
    let mut current = root()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match current.as_ref() {
        Some(existing) => existing.path == candidate,
        None => {
            let Ok(writer) = acquire_writer(&candidate) else {
                return false;
            };
            *current = Some(CacheRoot {
                path: candidate,
                _writer: writer,
            });
            true
        }
    }
}

fn configured_root() -> Option<PathBuf> {
    root()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|root| root.path.clone())
}

pub(crate) fn is_configured() -> bool {
    configured_root().is_some()
}

pub(crate) fn account_digest(identity: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(ENTROPY_PREFIX);
    hash.update(identity.as_bytes());
    hash.finalize().into()
}

fn account_entropy(digest: &[u8; 32]) -> Vec<u8> {
    let mut value = Vec::with_capacity(ENTROPY_PREFIX.len() + digest.len());
    value.extend_from_slice(ENTROPY_PREFIX);
    value.extend_from_slice(digest);
    value
}

fn cache_path(root: &Path, digest: &[u8; 32]) -> PathBuf {
    let filename = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    root.join(CACHE_DIRECTORY).join(format!("{filename}.bin"))
}

struct DpapiBuffer {
    pointer: *mut u8,
    length: u32,
}

impl DpapiBuffer {
    fn bytes(&self) -> Result<&[u8], ()> {
        if self.pointer.is_null() || self.length == 0 || u64::from(self.length) > MAX_STATE_BYTES {
            return Err(());
        }
        Ok(unsafe { std::slice::from_raw_parts(self.pointer, self.length as usize) })
    }
}

impl Drop for DpapiBuffer {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(self.pointer, self.length as usize).zeroize();
                LocalFree(Some(HLOCAL(self.pointer.cast::<c_void>())));
            }
        }
    }
}

fn dpapi(
    input_bytes: &[u8],
    entropy_bytes: &[u8],
    encrypt: bool,
) -> Result<Zeroizing<Vec<u8>>, ()> {
    if input_bytes.is_empty() || input_bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input_bytes.len()).map_err(|_| ())?,
        pbData: input_bytes.as_ptr().cast_mut(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(entropy_bytes.len()).map_err(|_| ())?,
        pbData: entropy_bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    result.map_err(|_| ())?;
    let output = DpapiBuffer {
        pointer: output.pbData,
        length: output.cbData,
    };
    Ok(Zeroizing::new(output.bytes()?.to_vec()))
}

fn pending_path(root: &Path, digest: &[u8; 32]) -> PathBuf {
    cache_path(root, digest).with_extension("pending")
}

fn save_protected_at(path: &Path, digest: &[u8; 32], plaintext: &[u8]) -> Result<(), ()> {
    let encrypted = dpapi(plaintext, &account_entropy(digest), true)?;
    let directory = path.parent().ok_or(())?;
    fs::create_dir_all(directory).map_err(|_| ())?;
    let mut temporary = NamedTempFile::new_in(directory).map_err(|_| ())?;
    temporary.write_all(&encrypted).map_err(|_| ())?;
    temporary.as_file().sync_all().map_err(|_| ())?;
    temporary.persist(path).map_err(|_| ())?;
    if let Ok(directory) = File::open(directory) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn save_at(root: &Path, identity: &str, plaintext: &[u8]) -> Result<(), ()> {
    let digest = account_digest(identity);
    save_protected_at(&cache_path(root, &digest), &digest, plaintext)?;
    remove_if_present(&revoked_path(root, &digest)).map_err(|_| ())
}

const PENDING_WRITE_MARKER: &[u8] = b"BitwardenRust/pending-vault-write/v1";

fn mark_pending_at(root: &Path, digest: &[u8; 32]) -> Result<(), ()> {
    save_protected_at(&pending_path(root, digest), digest, PENDING_WRITE_MARKER)
}

fn has_pending_at(root: &Path, digest: &[u8; 32]) -> bool {
    match fs::symlink_metadata(pending_path(root, digest)) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn clear_pending_at(root: &Path, digest: &[u8; 32]) {
    let _ = remove_if_present(&pending_path(root, digest));
}

fn revoked_path(root: &Path, digest: &[u8; 32]) -> PathBuf {
    cache_path(root, digest).with_extension("revoked")
}

fn revoke_at(root: &Path, digest: &[u8; 32]) -> bool {
    if remove_if_present(&cache_path(root, digest)).is_ok() {
        let _ = remove_if_present(&revoked_path(root, digest));
        clear_pending_at(root, digest);
        return true;
    }
    // On Windows an open handle may deny deletion. Leave a separate revocation marker
    // before reporting success, so startup cannot recover that snapshot.
    let marker = revoked_path(root, digest);
    fs::symlink_metadata(&marker).is_ok()
        || save_protected_at(&marker, digest, b"BitwardenRust/revoked-vault/v1").is_ok()
}

pub(crate) fn mark_pending(identity: &str) -> bool {
    let Some(root) = configured_root() else {
        return false;
    };
    mark_pending_at(&root, &account_digest(identity)).is_ok()
}

pub(crate) fn has_pending(identity: &str) -> bool {
    configured_root().is_some_and(|root| has_pending_at(&root, &account_digest(identity)))
}

pub(crate) fn has_pending_digest(digest: &[u8; 32]) -> bool {
    configured_root().is_some_and(|root| has_pending_at(&root, digest))
}

pub(crate) fn clear_pending(identity: &str) {
    if let Some(root) = configured_root() {
        clear_pending_at(&root, &account_digest(identity));
    }
}

fn load_at(root: &Path, identity: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ()> {
    let digest = account_digest(identity);
    load_protected_at(&cache_path(root, &digest), &digest)
}

fn load_protected_at(path: &Path, digest: &[u8; 32]) -> Result<Option<Zeroizing<Vec<u8>>>, ()> {
    // Fail closed if the marker exists or its status cannot be checked.
    match fs::symlink_metadata(path.with_extension("revoked")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Ok(None),
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_STATE_BYTES
    {
        let _ = fs::remove_file(path);
        return Ok(None);
    }
    let file = OpenOptions::new().read(true).open(path).map_err(|_| ())?;
    let mut encrypted = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut encrypted)
        .map_err(|_| ())?;
    if encrypted.len() as u64 > MAX_STATE_BYTES {
        let _ = fs::remove_file(path);
        return Ok(None);
    }
    match dpapi(&encrypted, &account_entropy(digest), false) {
        Ok(plaintext) => Ok(Some(plaintext)),
        Err(()) => {
            let _ = fs::remove_file(path);
            Ok(None)
        }
    }
}

pub(crate) fn save(identity: &str, plaintext: &[u8]) -> bool {
    configured_root().is_some_and(|root| save_at(&root, identity, plaintext).is_ok())
}

pub(crate) fn load(identity: &str) -> Option<Zeroizing<Vec<u8>>> {
    let root = configured_root()?;
    load_at(&root, identity).ok().flatten()
}

pub(crate) fn entries() -> impl Iterator<Item = ([u8; 32], Zeroizing<Vec<u8>>)> {
    configured_root()
        .and_then(|root| fs::read_dir(root.join(CACHE_DIRECTORY)).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("bin") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.as_bytes();
            if stem.len() != 64 || !stem.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let mut digest = [0_u8; 32];
            let (pairs, _) = stem.as_chunks::<2>();
            for (byte, pair) in digest.iter_mut().zip(pairs) {
                *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
            }
            Some((digest, load_protected_at(&path, &digest).ok()??))
        })
}

pub(crate) fn remove(identity: &str) -> bool {
    configured_root().is_none_or(|root| revoke_at(&root, &account_digest(identity)))
}

#[no_mangle]
pub unsafe extern "C" fn bw_local_cache_set_root(path: *const std::ffi::c_char) -> u8 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if path.is_null() {
            return 0;
        }
        let Ok(path) = unsafe { std::ffi::CStr::from_ptr(path) }.to_str() else {
            return 0;
        };
        u8::from(set_root(path))
    }))
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_state_round_trips_and_is_bound_to_account_identity() {
        let directory = tempfile::tempdir().unwrap();
        let identity = "https://api.bitwarden.com\nuser@example.invalid";
        let plaintext = br#"{"vault":"encrypted records"}"#;
        save_at(directory.path(), identity, plaintext).unwrap();
        let restored = load_at(directory.path(), identity).unwrap().unwrap();
        assert_eq!(restored.as_slice(), plaintext);
        assert!(load_at(
            directory.path(),
            "https://api.bitwarden.eu\nuser@example.invalid"
        )
        .unwrap()
        .is_none());
        let path = cache_path(directory.path(), &account_digest(identity));
        let ciphertext = fs::read(path).unwrap();
        assert!(!ciphertext.is_empty());
        assert!(!ciphertext.windows(7).any(|window| window == b"vault\""));
    }

    #[test]
    fn pending_write_marker_is_encrypted_and_requires_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let identity = "https://api.bitwarden.com\nuser@example.invalid";
        let digest = account_digest(identity);
        assert!(!has_pending_at(directory.path(), &digest));

        mark_pending_at(directory.path(), &digest).unwrap();
        assert!(has_pending_at(directory.path(), &digest));
        let marker = pending_path(directory.path(), &digest);
        let mut bytes = fs::read(&marker).unwrap();
        bytes[0] ^= 0x40;
        fs::write(&marker, bytes).unwrap();
        assert!(has_pending_at(directory.path(), &digest));

        clear_pending_at(directory.path(), &digest);
        assert!(!has_pending_at(directory.path(), &digest));
    }

    #[test]
    fn cache_journal_has_a_single_writer() {
        let directory = tempfile::tempdir().unwrap();
        let owner = acquire_writer(directory.path()).unwrap();
        assert_eq!(
            acquire_writer(directory.path()).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(owner);
        assert!(acquire_writer(directory.path()).is_ok());
    }

    #[test]
    fn revocation_blocks_a_snapshot_even_when_a_handle_prevents_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = "https://api.bitwarden.com\nuser@example.invalid";
        let digest = account_digest(identity);
        save_at(root, identity, b"old cache").unwrap();
        mark_pending_at(root, &digest).unwrap();
        let path = cache_path(root, &digest);
        let held = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();

        assert!(revoke_at(root, &digest));
        assert!(path.exists());
        assert!(has_pending_at(root, &digest));
        assert!(load_at(root, identity).unwrap().is_none());

        drop(held);
        save_at(root, identity, b"fresh cache").unwrap();
        assert!(!revoked_path(root, &digest).exists());
        assert_eq!(
            load_at(root, identity).unwrap().unwrap().as_slice(),
            b"fresh cache"
        );
    }

    #[test]
    fn tampered_state_is_discarded_without_returning_partial_data() {
        let directory = tempfile::tempdir().unwrap();
        let identity = "https://api.bitwarden.com\nuser@example.invalid";
        save_at(directory.path(), identity, b"sensitive test state").unwrap();
        let path = cache_path(directory.path(), &account_digest(identity));
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0x40;
        fs::write(&path, bytes).unwrap();
        assert!(load_at(directory.path(), identity).unwrap().is_none());
        assert!(!path.exists());
    }
}
