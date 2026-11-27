//! In-process Rust desktop smoke test. Run against the repo's public loopback
//! fixture server only; never accepts a user account or remote server.
use bytewarden_core::desktop::{
    self, AuthStatus, DraftField, DraftUri, Login, Request, Server, Session,
};
use std::{
    env,
    io::{Read, Write},
    net::{Ipv4Addr, TcpStream},
    time::Duration,
};
use uuid::Uuid;

#[test]
#[ignore = "requires a running tests/fixture_server.py and BYTEWARDEN_TEST_FIXTURE_PORT"]
fn native_desktop_sign_in_sync_reprompt_write_lock_and_unlock() {
    let port = env::var("BYTEWARDEN_TEST_FIXTURE_PORT")
        .expect("port")
        .parse::<u16>()
        .expect("16-bit port");
    let mut health =
        TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), Duration::from_secs(2))
            .unwrap();
    health
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        health,
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    health.take(16 * 1024).read_to_string(&mut reply).unwrap();
    let (headers, body) = reply.split_once("\r\n\r\n").expect("HTTP health");
    assert!(headers.lines().next().unwrap().contains(" 200 "));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap()["fixture"],
        true
    );
    let root = tempfile::tempdir().unwrap();
    desktop::configure_cache(root.path()).unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let password = "asdfasdfasdf"; // publicly documented SDK synthetic password
    let status = Request::begin()
        .login(Login {
            email: "test@bitwarden.com",
            password,
            server: Server::SelfHosted(&url),
            device: Uuid::new_v4(),
            two_factor: None,
            device_code: "",
            remember_2fa: false,
        })
        .unwrap();
    assert_eq!(status, AuthStatus::Authenticated);
    assert_eq!(
        desktop::accounts()
            .unwrap()
            .iter()
            .filter(|account| account.active)
            .count(),
        1
    );
    let account = desktop::accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.active)
        .unwrap();
    let overview = Request::begin().sync().unwrap();
    assert!(!overview.read_only);
    let protected = overview.items.iter().find(|item| item.reprompt).unwrap();
    for (secret, code) in [("", "passwordRequired"), ("wrong", "wrongPassword")] {
        assert_eq!(
            Request::begin()
                .detail(&protected.id, secret)
                .err()
                .unwrap()
                .code(),
            code
        );
    }
    let details = Request::begin().detail(&protected.id, password).unwrap();
    assert_eq!(details.name, protected.name);
    assert!(details.fields.iter().any(|field| field.sensitive));
    let attachment = details
        .attachment_files
        .first()
        .expect("fixture attachment");
    let lease = details.attachment_lease().unwrap();
    let attachment_path = root.path().join("synthetic-attachment.txt");
    lease.download(&attachment.id, &attachment_path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&attachment_path).unwrap(),
        "Bitwarden synthetic attachment fixture\n"
    );
    details.revoke();
    assert_eq!(
        lease
            .download(&attachment.id, &attachment_path)
            .unwrap_err()
            .code(),
        "attachmentViewExpired"
    );
    let login = overview
        .items
        .iter()
        .find(|item| item.name == "Synthetic login")
        .unwrap();
    let detail = Request::begin().detail(&login.id, "").unwrap();
    assert_eq!(detail.passkeys.len(), 1);
    assert_eq!(
        detail.totp_lease().unwrap().refresh().unwrap().code.len(),
        6
    );
    detail.revoke();

    let request = Request::begin();
    let folder = request.create_folder("Native GPUI / Synthetic").unwrap();
    let folder_id = folder.id.unwrap();
    let mut draft = desktop::Draft::default();
    draft.name = "GPUI synthetic note".into();
    draft.notes = Some("Synthetic draft".into());
    draft.item_type = 2;
    draft.folder_id = Some(folder_id.clone());
    let created = Request::begin().save_item(&draft, "").unwrap();
    let mut uri_login = desktop::Draft::default();
    uri_login.name = "GPUI synthetic URI login".into();
    uri_login.item_type = 1;
    uri_login
        .values
        .insert("username".into(), "synthetic-user".into());
    uri_login
        .values
        .insert("password".into(), "synthetic-password".into());
    uri_login.uris = vec![
        DraftUri {
            uri: Some("https://first.example.invalid".into()),
            r#match: None,
        },
        DraftUri {
            uri: Some("login.example.invalid".into()),
            r#match: Some(1),
        },
    ];
    uri_login.custom_fields = vec![
        DraftField {
            name: Some("synthetic text".into()),
            value: Some("plain".into()),
            r#type: 0,
            linked_id: None,
        },
        DraftField {
            name: Some("synthetic hidden".into()),
            value: Some("hidden-value".into()),
            r#type: 1,
            linked_id: None,
        },
        DraftField {
            name: Some("synthetic boolean".into()),
            value: Some("true".into()),
            r#type: 2,
            linked_id: None,
        },
        DraftField {
            name: Some("synthetic linked username".into()),
            value: None,
            r#type: 3,
            linked_id: Some(100),
        },
    ];
    let uri_login_id = Request::begin()
        .save_item(&uri_login, "")
        .unwrap()
        .id
        .unwrap();
    let mut uri_login = Request::begin().editor(&uri_login_id, "").unwrap();
    assert_eq!(uri_login.uris.len(), 2);
    assert_eq!(uri_login.uris[0].r#match, None);
    assert_eq!(uri_login.uris[1].r#match, Some(1));
    assert_eq!(uri_login.custom_fields.len(), 4);
    assert_eq!(uri_login.custom_fields[3].linked_id, Some(100));
    assert_eq!(
        uri_login.values.get("password").map(String::as_str),
        Some("synthetic-password")
    );
    uri_login.uris.remove(0);
    Request::begin().save_item(&uri_login, "").unwrap();
    let edited_login = Request::begin().editor(&uri_login_id, "").unwrap();
    assert_eq!(edited_login.uris.len(), 1);
    assert_eq!(
        edited_login.uris[0].uri.as_deref(),
        Some("login.example.invalid")
    );
    assert_eq!(edited_login.uris[0].r#match, Some(1));
    assert_eq!(edited_login.custom_fields.len(), 4);
    assert_eq!(
        edited_login.custom_fields[1].value.as_deref(),
        Some("hidden-value")
    );
    assert_eq!(edited_login.custom_fields[3].linked_id, Some(100));
    assert_eq!(
        edited_login.values.get("password").map(String::as_str),
        Some("synthetic-password")
    );
    let id = created.id.unwrap();
    let mut draft = Request::begin().editor(&id, "").unwrap();
    assert_eq!(draft.folder_id.as_deref(), Some(folder_id.as_str()));
    draft.name = "GPUI synthetic renamed note".into();
    Request::begin().save_item(&draft, "").unwrap();
    Request::begin()
        .rename_folder(&folder_id, "Native GPUI / Renamed")
        .unwrap();
    Request::begin()
        .move_items(std::slice::from_ref(&id), None)
        .unwrap();
    Request::begin().delete_folders(&[folder_id]).unwrap();
    let refreshed = desktop::overview().unwrap();
    assert!(refreshed.items.iter().any(|item| item.id == id
        && item.name == "GPUI synthetic renamed note"
        && item.folder_id.is_none()));
    let mut send = desktop::SendDraft::default();
    send.name = "Synthetic GPUI Send".into();
    send.text = Some("回环合成文本".into());
    send.password = Some("synthetic-send-password".into());
    send.deletion_date = (chrono::Utc::now() + chrono::TimeDelta::days(7)).to_rfc3339();
    Request::begin()
        .save_send(&account.id, None, &send, None)
        .unwrap();
    let listing = Request::begin().sends(&account.id).unwrap();
    let item = listing
        .items
        .iter()
        .find(|item| item.name == send.name)
        .unwrap();
    let info = Request::begin().send_detail(&account.id, &item.id).unwrap();
    assert_eq!(info.text.as_deref(), send.text.as_deref());
    assert!(info.has_password);
    assert!(Request::begin()
        .send_link(&account.id, &item.id)
        .unwrap()
        .starts_with(&url));
    Request::begin()
        .remove_send_auth(&account.id, &info.summary)
        .unwrap();
    let info = Request::begin().send_detail(&account.id, &item.id).unwrap();
    assert!(!info.has_password);
    Request::begin()
        .delete_send(&account.id, &info.summary)
        .unwrap();
    send.name = "Synthetic GPUI File Send".into();
    send.text = None;
    Request::begin()
        .save_send(&account.id, None, &send, Some(&attachment_path))
        .unwrap();
    let listing = Request::begin().sends(&account.id).unwrap();
    let file = listing
        .items
        .iter()
        .find(|item| item.name == send.name)
        .unwrap();
    assert_eq!(
        Request::begin()
            .send_detail(&account.id, &file.id)
            .unwrap()
            .file_name
            .as_deref(),
        Some("synthetic-attachment.txt")
    );
    Request::begin().delete_send(&account.id, file).unwrap();

    let export_path = root.path().join("synthetic-backup.json");
    let format = desktop::ExportFormat::PlainJson;
    assert_eq!(
        Request::begin()
            .export(&account.id, &export_path, format, false, password, "")
            .err()
            .unwrap()
            .code(),
        "validation"
    );
    let report = Request::begin()
        .export(&account.id, &export_path, format, true, password, "")
        .unwrap();
    assert!(report.items >= 2);
    Request::begin()
        .import(&account.id, &export_path, "")
        .unwrap();
    desktop::lock().unwrap();
    assert_eq!(desktop::session(), Session::Locked);
    assert_eq!(desktop::overview().err().unwrap().code(), "locked");
    Request::begin().unlock(password).unwrap();
    assert_eq!(desktop::session(), Session::Unlocked);
    desktop::logout().unwrap();
    assert_eq!(desktop::session(), Session::SignedOut);
}
