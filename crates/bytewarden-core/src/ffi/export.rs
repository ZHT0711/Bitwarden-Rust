use serde::Deserialize;
use uuid::Uuid;

use super::*;
use crate::vault::{export::Format, import};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    account_id: Uuid,
    destination: String,
    format: ExportFormat,
    #[serde(default)]
    allow_plaintext: bool,
    #[serde(default)]
    export_password: String,
}

#[derive(Deserialize)]
enum ExportFormat {
    #[serde(rename = "plainJson")]
    Plain,
    #[serde(rename = "csv")]
    Csv,
    #[serde(rename = "accountJson")]
    Account,
    #[serde(rename = "passwordJson")]
    Password,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.export_password.zeroize();
    }
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_export(
    request_json: *const c_char,
    master_password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(json) = (unsafe { read_utf8(request_json) })
            .map(Zeroizing::new)
            .filter(|s| s.len() <= 32_768)
        else {
            return vault_error("validation");
        };
        let Ok(mut request) = serde_json::from_str::<Request>(&json) else {
            return vault_error("validation");
        };
        let Some(master) = (unsafe { read_utf8(master_password) })
            .map(Zeroizing::new)
            .filter(|s| !s.is_empty())
        else {
            return vault_error("passwordRequired");
        };
        let path = Path::new(&request.destination);
        if !path.is_absolute() || path.file_name().is_none() {
            return vault_error("fileAccess");
        }
        let format = match request.format {
            ExportFormat::Plain
                if request.allow_plaintext && request.export_password.is_empty() =>
            {
                Format::Plain
            }
            ExportFormat::Csv if request.allow_plaintext && request.export_password.is_empty() => {
                Format::Csv
            }
            ExportFormat::Account
                if !request.allow_plaintext && request.export_password.is_empty() =>
            {
                Format::Account
            }
            ExportFormat::Password
                if !request.allow_plaintext
                    && (1..=1024).contains(&request.export_password.encode_utf16().count()) =>
            {
                Format::Password(Zeroizing::new(std::mem::take(&mut request.export_password)))
            }
            _ => return vault_error("validation"),
        };
        let Some(runtime) = runtime() else {
            return vault_error("internal");
        };
        let mut guard = auth_client()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard
            .active
            .as_mut()
            .filter(|_| bw_auth_session_state() == SESSION_UNLOCKED)
        else {
            return vault_access_error();
        };
        if session.account_id != request.account_id {
            return vault_error("cancelled");
        }
        if session.offline_only {
            return vault_error("signInRequired");
        }
        let result = runtime.block_on(auth_request::run(request_id, async {
            let keys = session
                .unlock_data
                .as_ref()
                .ok_or(VaultError::PasswordRequired)?;
            if !session
                .client
                .internal
                .verify_desktop_user_key(&master, &keys.master_password)
            {
                return Err(VaultError::WrongPassword);
            }
            // A previously synchronized snapshot may predate an organization policy change.
            let cache = VaultCache::fetch(&session.client, &mut session.unlock_data).await?;
            if session.unlock_data.is_none() {
                return Err(VaultError::NotAuthenticated);
            }
            cache
                .export_personal(&session.client, format, path, request_id)
                .await
        }));
        #[cfg(windows)]
        if session.unlock_data.is_none() && !remove_cached_vault(session) {
            return vault_error("cacheRevocationFailed");
        }
        match result {
            Ok(Ok(report)) => vault_success(&report),
            Ok(Err(error)) => vault_error(&error.to_string()),
            Err(RequestError::Cancelled) => vault_error("cancelled"),
            Err(RequestError::TimedOut) => vault_error("timedOut"),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

#[no_mangle]
pub unsafe extern "C" fn bw_vault_import(
    account: *const c_char,
    source: *const c_char,
    password: *const c_char,
    request_id: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(account_id) =
            (unsafe { read_utf8(account) }).and_then(|v| Uuid::parse_str(&v).ok())
        else {
            return vault_error("validation");
        };
        let Some(source) = (unsafe { read_utf8(source) }).filter(|v| Path::new(v).is_absolute())
        else {
            return vault_error("fileAccess");
        };
        let Some(password) = (unsafe { read_utf8(password) }).map(Zeroizing::new) else {
            return vault_error("validation");
        };
        match import_command(account_id, source, password) {
            Ok(command) => perform_vault_write(command, request_id),
            Err(code) => vault_error(code),
        }
    }))
    .unwrap_or_else(|_| vault_error("internal"))
}

pub(super) fn import_command(
    account_id: Uuid,
    source: String,
    password: Zeroizing<String>,
) -> Result<VaultWriteCommand, &'static str> {
    if !Path::new(&source).is_absolute() {
        return Err("fileAccess");
    }
    if password.encode_utf16().count() > 1024 {
        return Err("validation");
    }
    Ok(VaultWriteCommand::Import {
        account_id,
        source,
        password,
    })
}

pub(super) async fn import(
    session: &mut AuthSession,
    source: &str,
    password: &str,
    request_id: u64,
) -> Result<VaultMutationResult, VaultError> {
    // Do not merge a new import against cached policy data or into a vault with a changed key.
    let fresh = tokio::time::timeout(
        auth_request::TIMEOUT,
        VaultCache::fetch(&session.client, &mut session.unlock_data),
    )
    .await
    .map_err(|_| VaultError::Network)??;
    if session.unlock_data.is_none() {
        return Err(VaultError::NotAuthenticated);
    }
    let body = import::prepare(
        &fresh,
        &session.client,
        Path::new(source),
        password,
        request_id,
    )
    .await?;
    let expected = import::ExpectedImport::from_request(&body)?;
    session
        .vault
        .submit_import(&session.client, body, request_id)
        .await?;
    let refreshed = tokio::time::timeout(
        auth_request::TIMEOUT,
        VaultCache::fetch(&session.client, &mut session.unlock_data),
    )
    .await;
    let cache = match refreshed {
        Ok(Ok(cache)) if session.unlock_data.is_some() && cache.overview().is_ok() => cache,
        _ => {
            session.vault.mark_write_uncertain();
            return Err(VaultError::WriteUncertain);
        }
    };
    let result = expected.confirm(&cache).map_err(|_| {
        session.vault.mark_write_uncertain();
        VaultError::WriteUncertain
    })?;
    cache.activate(&session.client);
    session.vault = cache;
    session.open_detail = None;
    Ok(result)
}
