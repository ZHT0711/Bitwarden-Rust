use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CipherType {
    Login,
    SecureNote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Login {
    pub username: Option<String>,
    pub password: Option<String>,
    pub uris: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureNote {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cipher {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub name: String,
    pub notes: Option<String>,
    pub favorite: bool,
    pub kind: CipherType,
    pub login: Option<Login>,
    pub secure_note: Option<SecureNote>,
}

impl Cipher {
    pub fn demo_login(name: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id: None,
            folder_id: None,
            name: name.into(),
            notes: None,
            favorite: false,
            kind: CipherType::Login,
            login: Some(Login {
                username: Some(username.into()),
                password: Some("demo-password".to_string()),
                uris: vec!["https://example.com".to_string()],
            }),
            secure_note: None,
        }
    }

    pub fn demo_note(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id: None,
            folder_id: None,
            name: name.into(),
            notes: None,
            favorite: false,
            kind: CipherType::SecureNote,
            login: None,
            secure_note: Some(SecureNote { text: text.into() }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSnapshot {
    pub folders: Vec<Folder>,
    pub ciphers: Vec<Cipher>,
}

impl VaultSnapshot {
    pub fn demo() -> Self {
        Self {
            folders: vec![Folder {
                id: Uuid::new_v4(),
                name: "Personal".to_string(),
            }],
            ciphers: vec![
                Cipher::demo_login("Example account", "alice@example.com"),
                Cipher::demo_login("Code repository", "alice"),
                Cipher::demo_note("Recovery codes", "Replace this demo note after unlocking."),
            ],
        }
    }
}
