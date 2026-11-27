use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use bytewarden_core::desktop::{Account, Session};

use crate::client::profile_directory;

const MAX_PREFERENCE_BYTES: usize = 4096;
const FILE_PREFIX: &str = "generator-options-v1-";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    Anonymous,
    Account(String),
    Unavailable,
}

impl Scope {
    pub(super) fn for_account(account: Option<&Account>) -> Self {
        match account {
            None => Self::Anonymous,
            Some(account) if account.session() == Session::SignedOut => Self::Anonymous,
            Some(account) => account
                .settings_scope()
                .map(Self::Account)
                .unwrap_or(Self::Unavailable),
        }
    }

    fn key(&self) -> Option<&str> {
        match self {
            Self::Anonymous => Some("anonymous"),
            Self::Account(scope)
                if scope.len() == 64 && scope.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                Some(scope)
            }
            Self::Account(_) | Self::Unavailable => None,
        }
    }
}

pub(super) fn load(scope: &Scope) -> Option<String> {
    let root = profile_directory().ok()?;
    load_from(&root, scope)
}

fn load_from(root: &Path, scope: &Scope) -> Option<String> {
    let path = path(root, scope)?;
    let file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(MAX_PREFERENCE_BYTES.min(512));
    file.take((MAX_PREFERENCE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_PREFERENCE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

pub(super) fn save(scope: &Scope, content: &str) -> io::Result<()> {
    if content.len() > MAX_PREFERENCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "generator preferences exceed the size limit",
        ));
    }
    let root = profile_directory()?;
    save_to(&root, scope, content)
}

fn save_to(root: &Path, scope: &Scope, content: &str) -> io::Result<()> {
    if content.len() > MAX_PREFERENCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "generator preferences exceed the size limit",
        ));
    }
    fs::create_dir_all(root)?;
    let destination = path(root, scope)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid preference scope"))?;
    let temporary = root.join(format!(
        ".{FILE_PREFIX}{}-{}-{}.tmp",
        scope.key().unwrap_or_default(),
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, &destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn path(root: &Path, scope: &Scope) -> Option<PathBuf> {
    Some(root.join(format!("{FILE_PREFIX}{}.conf", scope.key()?)))
}

#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "bytewarden-generator-preferences-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn account_scopes_are_validated_and_anonymous_is_separate() {
        let root = temp_root();
        let anonymous = Scope::Anonymous;
        let account = Scope::Account("A".repeat(64));
        save_to(&root, &anonymous, "scope=anonymous").expect("anonymous preferences save");
        save_to(&root, &account, "scope=account").expect("account preferences save");

        assert_eq!(
            load_from(&root, &anonymous).as_deref(),
            Some("scope=anonymous")
        );
        assert_eq!(load_from(&root, &account).as_deref(), Some("scope=account"));
        assert!(path(&root, &Scope::Account("../unsafe".into())).is_none());
        assert!(path(&root, &Scope::Unavailable).is_none());
        fs::remove_dir_all(root).expect("temporary preference directory cleanup");
    }

    #[test]
    fn replace_is_complete_and_oversized_or_invalid_files_are_ignored() {
        let root = temp_root();
        let scope = Scope::Account("B".repeat(64));
        save_to(&root, &scope, "version=1\nvalue=old").expect("initial save");
        save_to(&root, &scope, "version=1\nvalue=new").expect("atomic replacement");
        assert_eq!(
            load_from(&root, &scope).as_deref(),
            Some("version=1\nvalue=new")
        );

        let file = path(&root, &scope).expect("valid settings path");
        fs::write(&file, vec![b'x'; MAX_PREFERENCE_BYTES + 1]).expect("oversized fixture");
        assert!(load_from(&root, &scope).is_none());
        fs::write(&file, [0xff, 0xfe]).expect("invalid UTF-8 fixture");
        assert!(load_from(&root, &scope).is_none());
        assert!(save_to(&root, &scope, &"x".repeat(MAX_PREFERENCE_BYTES + 1)).is_err());
        fs::remove_dir_all(root).expect("temporary preference directory cleanup");
    }
}
