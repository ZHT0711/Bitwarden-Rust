//! In-process typed adapter support for account-scoped sharing and backup.
use super::desktop::{self, Error, Mutation, Request};
use super::{export, sends};
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
pub struct Sends {
    pub items: Vec<SendSummary>,
    pub failures: usize,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendSummary {
    pub id: String,
    pub name: String,
    pub kind: u8,
    pub disabled: bool,
    pub deletion_date: String,
    pub expiration_date: Option<String>,
    pub revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendDetail {
    #[serde(flatten)]
    pub summary: SendSummary,
    pub notes: Option<String>,
    pub text: Option<String>,
    pub file_name: Option<String>,
    pub hidden: bool,
    pub hide_email: bool,
    pub max_access_count: Option<u32>,
    pub access_count: u32,
    pub auth_type: u8,
    pub has_password: bool,
    pub emails: Vec<String>,
}

impl Drop for SendDetail {
    fn drop(&mut self) {
        self.notes.zeroize();
        self.text.zeroize();
        self.emails.zeroize();
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendDraft {
    pub name: String,
    pub text: Option<String>,
    pub notes: Option<String>,
    pub hidden: bool,
    pub disabled: bool,
    pub hide_email: bool,
    pub max_access_count: Option<u32>,
    pub deletion_date: String,
    pub expiration_date: Option<String>,
    pub password: Option<String>,
    pub emails: Option<Vec<String>>,
}

impl Drop for SendDraft {
    fn drop(&mut self) {
        self.text.zeroize();
        self.notes.zeroize();
        self.password.zeroize();
        self.emails.zeroize();
    }
}

#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "camelCase")]
enum SendCommand<'a> {
    List,
    Detail {
        id: &'a str,
    },
    Link {
        id: &'a str,
    },
    Create {
        data: &'a SendDraft,
    },
    CreateFile {
        data: &'a SendDraft,
        source: &'a str,
    },
    Edit {
        id: &'a str,
        revision: &'a str,
        data: &'a SendDraft,
    },
    Delete {
        id: &'a str,
        revision: &'a str,
    },
    RemoveAuth {
        id: &'a str,
        revision: &'a str,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub items: usize,
    pub folders: usize,
    pub skipped_restricted: usize,
    pub omitted_attachments: usize,
    #[serde(default)]
    pub omitted_unsupported: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    PlainJson,
    Csv,
    AccountJson,
    PasswordJson,
}

impl Request {
    fn send<T: serde::de::DeserializeOwned>(
        self,
        account_id: &str,
        command: SendCommand<'_>,
    ) -> Result<T, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            account_id: &'a str,
            command: SendCommand<'a>,
        }
        let payload = desktop::serial(&Payload {
            account_id,
            command,
        })?;
        desktop::read(unsafe { sends::bw_send_command(desktop::ptr(&payload), self.raw()) })
    }

    pub fn sends(self, account: &str) -> Result<Sends, Error> {
        self.send(account, SendCommand::List)
    }
    pub fn send_detail(self, account: &str, id: &str) -> Result<SendDetail, Error> {
        self.send(account, SendCommand::Detail { id })
    }
    pub fn send_link(self, account: &str, id: &str) -> Result<Zeroizing<String>, Error> {
        self.send::<String>(account, SendCommand::Link { id })
            .map(Zeroizing::new)
    }
    pub fn save_send(
        self,
        account: &str,
        current: Option<&SendSummary>,
        data: &SendDraft,
        source: Option<&Path>,
    ) -> Result<(), Error> {
        let command = match (current, source) {
            (Some(item), None) => SendCommand::Edit {
                id: &item.id,
                revision: &item.revision,
                data,
            },
            (None, Some(source)) => SendCommand::CreateFile {
                source: source.to_str().ok_or_else(Error::validation)?,
                data,
            },
            (None, None) => SendCommand::Create { data },
            (Some(_), Some(_)) => return Err(Error::validation()),
        };
        self.send::<bool>(account, command).map(|_| ())
    }
    pub fn delete_send(self, account: &str, item: &SendSummary) -> Result<(), Error> {
        self.send::<bool>(
            account,
            SendCommand::Delete {
                id: &item.id,
                revision: &item.revision,
            },
        )
        .map(|_| ())
    }
    pub fn remove_send_auth(self, account: &str, item: &SendSummary) -> Result<(), Error> {
        self.send::<bool>(
            account,
            SendCommand::RemoveAuth {
                id: &item.id,
                revision: &item.revision,
            },
        )
        .map(|_| ())
    }

    pub fn import(self, account: &str, source: &Path, password: &str) -> Result<Mutation, Error> {
        let account = desktop::checked_argument(account)?;
        let source = desktop::checked_argument(source.to_str().ok_or_else(Error::validation)?)?;
        let password = desktop::checked_argument(password)?;
        let account_id = uuid::Uuid::parse_str(account).map_err(|_| Error::validation())?;
        let command =
            export::import_command(account_id, source.into(), Zeroizing::new(password.into()))
                .map_err(Error::from_code)?;
        self.write(command)
    }

    pub fn export(
        self,
        account_id: &str,
        destination: &Path,
        format: ExportFormat,
        explicitly_allow_plaintext: bool,
        master: &str,
        export_password: &str,
    ) -> Result<ExportReport, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            account_id: &'a str,
            destination: &'a str,
            format: ExportFormat,
            allow_plaintext: bool,
            export_password: &'a str,
        }
        let payload = desktop::serial(&Payload {
            account_id,
            destination: destination.to_str().ok_or_else(Error::validation)?,
            format,
            allow_plaintext: explicitly_allow_plaintext,
            export_password,
        })?;
        let master = desktop::argument(master)?;
        desktop::read(unsafe {
            export::bw_vault_export(desktop::ptr(&payload), desktop::ptr(&master), self.raw())
        })
    }
}
