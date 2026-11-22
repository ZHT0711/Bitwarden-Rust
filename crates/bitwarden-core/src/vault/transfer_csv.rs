//! The published personal Bitwarden CSV interchange format only supports logins and notes.
use csv::{ReaderBuilder, StringRecord, WriterBuilder};
use serde_json::json;
use zeroize::Zeroizing;

use super::*;

const HEADERS: [&str; 11] = [
    "folder",
    "favorite",
    "type",
    "name",
    "notes",
    "fields",
    "reprompt",
    "login_uri",
    "login_username",
    "login_password",
    "login_totp",
];

pub(super) fn emit(folders: Vec<Value>, items: Vec<Value>) -> Result<Vec<u8>, VaultError> {
    let folders = folders
        .iter()
        .filter_map(|row| Some((row.get("id")?.as_str()?, row.get("name")?.as_str()?)))
        .collect::<HashMap<_, _>>();
    let mut writer = WriterBuilder::new().from_writer(Vec::new());
    writer
        .write_record(HEADERS.into_iter().chain(["archivedDate"]))
        .map_err(|_| VaultError::Internal)?;
    for item in &items {
        let data = |field| item.get(field).and_then(Value::as_str).unwrap_or_default();
        let login = |field| {
            item.get("login")
                .and_then(|l| l.get(field))
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let kind = match item.get("type").and_then(Value::as_u64) {
            Some(1) => "login",
            Some(2) => "note",
            _ => return Err(VaultError::Internal),
        };
        let fields = Zeroizing::new(
            item.get("fields")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|field| {
                    let text = |name| field.get(name).and_then(Value::as_str).unwrap_or_default();
                    format!("{}: {}", text("name"), text("value"))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        // Bitwarden embeds a second CSV row in login_uri to represent multiple addresses.
        let uri = encode_uris(
            item.get("login")
                .and_then(|l| l.get("uris"))
                .and_then(Value::as_array),
        )?;
        writer
            .write_record([
                folders.get(data("folderId")).copied().unwrap_or_default(),
                if item.get("favorite").and_then(Value::as_bool) == Some(true) {
                    "1"
                } else {
                    ""
                },
                kind,
                data("name"),
                data("notes"),
                &fields,
                if item.get("reprompt").and_then(Value::as_u64) == Some(1) {
                    "1"
                } else {
                    "0"
                },
                &uri,
                login("username"),
                login("password"),
                login("totp"),
                data("archivedDate"),
            ])
            .map_err(|_| VaultError::Internal)?;
    }
    writer.into_inner().map_err(|_| VaultError::Internal)
}

fn encode_uris(uris: Option<&Vec<Value>>) -> Result<Zeroizing<String>, VaultError> {
    let uris = uris
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("uri").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if uris.is_empty() {
        return Ok(Zeroizing::new(String::new()));
    }
    let mut writer = WriterBuilder::new().from_writer(Vec::new());
    writer
        .write_record(uris)
        .map_err(|_| VaultError::Internal)?;
    let mut result = writer.into_inner().map_err(|_| VaultError::Internal)?;
    result.pop(); // csv::Writer's default LF record terminator (not part of the field).
    String::from_utf8(result)
        .map(Zeroizing::new)
        .map_err(|_| VaultError::Internal)
}

pub(super) fn parse(data: &[u8], max_rows: usize) -> Result<Value, VaultError> {
    let mut reader = ReaderBuilder::new().from_reader(data);
    let header = reader
        .headers()
        .map_err(|_| VaultError::ImportFormat)?
        .clone();
    let mut fields = HashMap::with_capacity(header.len());
    for (i, name) in header.iter().enumerate() {
        if fields.insert(name, i).is_some()
            || (name != "archivedDate"
                && !HEADERS.contains(&name)
                && !["username", "password", "totp", "uri"].contains(&name))
        {
            return Err(VaultError::ImportFormat);
        }
    }
    if !fields.contains_key("type") || !fields.contains_key("name") {
        return Err(VaultError::ImportFormat);
    }
    let mut folders = HashMap::new();
    let mut items = Vec::new();
    for record in reader.records() {
        let row = record.map_err(|_| VaultError::ImportFormat)?;
        let value = |key| field(&row, &fields, key).unwrap_or_default();
        let optional = |key| field(&row, &fields, key);
        let alias = |a, b| optional(a).or_else(|| optional(b));
        if row.iter().all(str::is_empty) {
            continue;
        }
        let folder = optional("folder")
            .map(|name| *folders.entry(name.to_owned()).or_insert_with(Uuid::new_v4));
        if items.len() + folders.len() >= max_rows {
            return Err(VaultError::ImportFormat);
        }
        let favorite = match value("favorite") {
            "" | "0" => false,
            "1" => true,
            _ => return Err(VaultError::ImportFormat),
        };
        let reprompt = match value("reprompt") {
            "" | "0" => 0,
            "1" => 1,
            _ => return Err(VaultError::ImportFormat),
        };
        let mut custom = Vec::new();
        for line in value("fields")
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            let (name, value) = line.rsplit_once(": ").ok_or(VaultError::ImportFormat)?;
            custom.push(json!({"name":name, "value":value,"type":0}));
        }
        let mut item = json!({ "folderId": folder, "name": optional("name").unwrap_or("--"), "notes": optional("notes"),
            "favorite":favorite, "reprompt":reprompt, "archivedDate": optional("archivedDate"), "fields":custom });
        match value("type").to_ascii_lowercase().as_str() {
            "note" => {
                item["type"] = json!(2);
                item["secureNote"] = json!({ "type": 0 });
            }
            "login" => {
                item["type"] = json!(1);
                let mut uris = Vec::new();
                if let Some(encoded) = alias("login_uri", "uri") {
                    let mut reader = ReaderBuilder::new()
                        .has_headers(false)
                        .from_reader(encoded.as_bytes());
                    let mut records = reader.records();
                    if let Some(record) = records.next() {
                        for uri in &record.map_err(|_| VaultError::ImportFormat)? {
                            if !uri.is_empty() {
                                uris.push(json!({ "uri":uri }));
                            }
                        }
                    }
                    if records.next().is_some() {
                        return Err(VaultError::ImportFormat);
                    }
                }
                item["login"] = json!({ "username":alias("login_username", "username"), "password":alias("login_password", "password"),
                    "totp":alias("login_totp","totp"), "uris":uris });
            }
            _ => return Err(VaultError::ImportFormat),
        }
        items.push(item);
    }
    let folders = folders
        .into_iter()
        .map(|(name, id)| json!({ "name": name, "id": id }))
        .collect::<Vec<_>>();
    Ok(json!({ "encrypted": false, "folders": folders, "items": items }))
}

fn field<'a>(row: &'a StringRecord, headers: &HashMap<&str, usize>, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|index| row.get(*index))
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_multiline_and_nested_uri_fields_without_loss() {
        let id = Uuid::new_v4();
        let item = json!({ "folderId": id, "name":"中文, \"逗号\"", "type":1, "notes":"first\r\nsecond",
            "favorite":true,"reprompt":1,"login":{"username":"a", "password":" spaces ","uris":[{"uri":"https://example.invalid/a,b"},{"uri":"https://example.invalid/c"}]},
            "fields":[{"name":"user", "value":"value", "type":0}] });
        let bytes = emit(vec![json!({"id":id,"name":"a/b"})], vec![item.clone()]).unwrap();
        let response = parse(&bytes, 100).unwrap();
        let restored = &response["items"][0];
        for key in ["name", "notes", "favorite", "type", "reprompt", "fields"] {
            assert_eq!(item[key], restored[key]);
        }
        assert_eq!(item["login"]["password"], restored["login"]["password"]);
        assert_eq!(item["login"]["uris"], restored["login"]["uris"]);
        assert_eq!(response["folders"][0]["name"], "a/b");
        assert_eq!(response["folders"][0]["id"], restored["folderId"]);
    }

    #[test]
    fn unknown_or_ambiguous_csv_does_not_become_a_personal_import() {
        assert!(parse(b"type,name,collections\nlogin,x,shared", 100).is_err());
        assert!(parse(b"type,name,name\nlogin,x,y", 100).is_err());
        assert!(parse(b"type,name\ncard,x", 100).is_err());
    }
}
