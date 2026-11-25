//! Public SDK account only. These fixtures exercise real encrypted folder metadata in WinUI.
use super::{
    tests::{response_for, test_view},
    *,
};

struct Scenario {
    names: Vec<String>,
    root: usize,
    target: usize,
    roots: usize,
    search: String,
}

fn scenario(kind: &str, size: usize) -> Scenario {
    assert!(size >= 2);
    match kind {
        "flat" => Scenario {
            names: (0..size).map(|index| format!("Flat {index:05}")).collect(),
            root: 0,
            target: size - 1,
            roots: size,
            search: format!("Flat {:05}", size - 1),
        },
        "branching" => {
            let roots = (size / 100).clamp(1, 100);
            let names = (0..roots)
                .map(|index| format!("Group {index:03}"))
                .chain(
                    (0..size - roots)
                        .map(|index| format!("Group {:03}/Child {index:05}", index % roots)),
                )
                .collect();
            Scenario {
                names,
                root: 0,
                target: roots,
                roots,
                search: "Child 00000".to_owned(),
            }
        }
        "deep" => {
            let mut path = String::new();
            let mut names = Vec::with_capacity(size);
            for index in 0..size {
                if index > 0 {
                    path.push('/');
                }
                path.push_str(&format!("Depth{index:04}"));
                names.push(path.clone());
            }
            Scenario {
                names,
                root: 0,
                target: size - 1,
                roots: 1,
                search: format!("Depth{:04}", size - 1),
            }
        }
        "long-orphan" => Scenario {
            names: vec![
                "Anchor".to_owned(),
                format!("Anchor/{}Leaf", "Missing/".repeat(size)),
            ],
            root: 0,
            target: 1,
            roots: 1,
            search: "Leaf".to_owned(),
        },
        _ => panic!("folder scenario must be flat, branching, deep or long-orphan"),
    }
}

fn folder_id(index: usize) -> uuid::Uuid {
    uuid::Uuid::from_u128(0xafff0000000040008000000000000000 + index as u128)
}

#[allow(deprecated)] // Local encryption fixture; higher-level create would require a server write.
fn encrypted_folders(
    client: &Client,
    names: &[String],
) -> Vec<bitwarden_api_api::models::FolderResponseModel> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let folder = client
                .vault()
                .folders()
                .encrypt(bitwarden_vault::FolderView {
                    id: Some(folder_id(index).to_string().parse().unwrap()),
                    name: name.clone(),
                    revision_date: "2026-11-20T00:00:00Z".parse().unwrap(),
                })
                .unwrap();
            assert!(folder.name.to_string().starts_with("2."));
            serde_json::from_value(serde_json::to_value(folder).unwrap()).unwrap()
        })
        .collect()
}

async fn response(client: &Client, shape: &Scenario) -> SyncResponseModel {
    let mut login = test_view();
    login.folder_id = Some(folder_id(shape.target).to_string().parse().unwrap());
    let mut result = response_for(client, login).await;
    let mut protected = test_view();
    protected.name = "Synthetic protected item".to_owned();
    protected.reprompt = bitwarden_vault::CipherRepromptType::Password;
    protected.folder_id = Some(folder_id(shape.root).to_string().parse().unwrap());
    result
        .ciphers
        .as_mut()
        .unwrap()
        .extend(response_for(client, protected).await.ciphers.unwrap());
    result.folders = Some(encrypted_folders(client, &shape.names));
    result
}

#[test]
fn shapes_keep_real_ids_and_expected_counts() {
    let flat = scenario("flat", 10_000);
    assert_eq!((flat.names.len(), flat.roots), (10_000, 10_000));
    let branches = scenario("branching", 10_000);
    assert_eq!(
        (branches.names.len(), branches.roots, branches.target),
        (10_000, 100, 100)
    );
    assert_eq!(branches.names[100], "Group 000/Child 00000");
    let deep = scenario("deep", 1_000);
    assert_eq!(deep.names.len(), 1_000);
    assert_eq!(deep.names[999].split('/').count(), 1_000);
    let orphan = scenario("long-orphan", 8_192);
    assert_eq!(orphan.names.len(), 2);
    assert_eq!(orphan.names[1].split('/').count(), 8_194);
    assert_ne!(folder_id(0), folder_id(1));
}

#[tokio::test]
async fn encrypted_scenarios_round_trip_through_actual_vault_projection() {
    for kind in ["flat", "branching", "deep", "long-orphan"] {
        let client = Client::init_test_account(
            bw_sdk_core::client::test_accounts::test_bitwarden_com_account(),
        )
        .await;
        let shape = scenario(kind, 12);
        let cache = VaultCache::from_response(&client, response(&client, &shape).await, &mut None)
            .await
            .unwrap();
        let overview = cache.overview().unwrap();
        assert_eq!(overview.folders.len(), shape.names.len());
        for (index, name) in shape.names.iter().enumerate() {
            let folder = overview
                .folders
                .iter()
                .find(|folder| folder.id == folder_id(index).to_string())
                .unwrap();
            assert_eq!(&folder.name, name);
        }
        assert_eq!(overview.items.len(), 2);
        assert_eq!(
            overview
                .items
                .iter()
                .find(|item| item.name == "Synthetic login")
                .unwrap()
                .folder_id,
            Some(folder_id(shape.target).to_string())
        );
        assert!(
            overview
                .items
                .iter()
                .find(|item| item.name == "Synthetic protected item")
                .unwrap()
                .reprompt
        );
    }
}

#[tokio::test]
#[ignore = "Writes public synthetic encrypted folder metadata for local WinUI pressure tests"]
async fn write_synthetic_folder_ui_fixture() {
    let output =
        std::env::var("BYTEWARDEN_FIXTURE_OUTPUT").expect("set an explicit fixture output path");
    let kind =
        std::env::var("BYTEWARDEN_FOLDER_SCENARIO").expect("set an explicit folder scenario");
    let size = match kind.as_str() {
        "flat" | "branching" => 10_000,
        "deep" => 1_000,
        "long-orphan" => 8_192,
        _ => panic!("unsupported synthetic folder scenario"),
    };
    let shape = scenario(&kind, size);
    let client =
        Client::init_test_account(bw_sdk_core::client::test_accounts::test_bitwarden_com_account())
            .await;
    let mut value = serde_json::to_value(response(&client, &shape).await).unwrap();
    value["folderStress"] = serde_json::json!({
        "scenario": kind, "folders": shape.names.len(), "roots": shape.roots, "depth": if kind == "deep" { size } else { 0 },
        "rootId": folder_id(shape.root), "targetId": folder_id(shape.target),
        "rootName": shape.names[shape.root], "targetName": shape.names[shape.target], "search": shape.search,
        "nameChars": shape.names.iter().map(|name| name.len()).sum::<usize>()
    });
    std::fs::write(output, serde_json::to_vec(&value).unwrap()).unwrap();
}
