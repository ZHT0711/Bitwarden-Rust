use super::*;

fn overview(names: &[&str]) -> Overview {
    Overview {
        folders: names
            .iter()
            .enumerate()
            .map(|(index, name)| Folder {
                id: index.to_string(),
                name: (*name).into(),
            })
            .collect(),
        items: Vec::new(),
        last_sync: String::new(),
        failures: 0,
        read_only: false,
    }
}

fn visible_names<'a>(folders: &Folders, overview: &'a Overview) -> Vec<&'a str> {
    folders
        .visible
        .iter()
        .map(|index| overview.folders[folders.rows[*index].source].name.as_str())
        .collect()
}

#[test]
fn folder_search_matches_stored_full_names_across_all_levels_and_keeps_duplicates() {
    let overview = overview(&[
        "Work",
        "Work/Project",
        "Work/Project",
        "Work/Other",
        "Personal/Project",
    ]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    assert_eq!(
        visible_names(&folders, &overview),
        ["Personal/Project", "Work"]
    );
    assert!(folders.set_query("  wOrK/pRoJ  "));
    assert_eq!(
        visible_names(&folders, &overview),
        ["Work/Project", "Work/Project"]
    );
    let ids = folders
        .visible
        .iter()
        .map(|index| &overview.folders[folders.rows[*index].source].id)
        .collect::<Vec<_>>();
    assert_ne!(ids[0], ids[1]);
    folders.set_query("project");
    assert_eq!(folders.visible.len(), 3);
    folders.set_query("Work Other");
    assert!(
        folders.visible.is_empty(),
        "folder search is one literal substring, not item-search terms"
    );
    folders.set_query("  ");
    assert_eq!(
        visible_names(&folders, &overview),
        ["Personal/Project", "Work"]
    );
}

#[test]
fn folder_search_handles_unicode_case_and_never_expands_distinct_names() {
    let overview = overview(&["团队/😀计划", "Équipe/Σχέδιο", "Straße", "STRASSE"]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    folders.set_query("😀计");
    assert_eq!(visible_names(&folders, &overview), ["团队/😀计划"]);
    folders.set_query("éQUIPE/σχέ");
    assert_eq!(visible_names(&folders, &overview), ["Équipe/Σχέδιο"]);
    folders.set_query("strasse");
    assert_eq!(visible_names(&folders, &overview), ["STRASSE"]);
    assert_ne!(
        query_key("é"),
        query_key("e\u{301}"),
        "ordinal search does not normalize Unicode"
    );
}

#[test]
fn folder_hierarchy_has_only_real_parent_ids_and_keeps_missing_segments_visible() {
    let overview = overview(&["A", "A/B/C", "A/BC", "/missing/leaf/", "A/B/C/D", "A!"]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    folders
        .manager
        .select(Some("1"), &folders.by_id, &folders.rows);
    folders.update_visible();
    assert_eq!(visible_names(&folders, &overview), ["A/B/C/D"]);
    let trail = folders
        .manager
        .trail
        .iter()
        .map(|index| folders.rows[*index].source)
        .collect::<Vec<_>>();
    assert_eq!(trail, [0, 1]);
    let row = &folders.rows[folders.by_id["1"]];
    assert_eq!(row.display.as_ref(), "B/C");
    let orphan = &folders.rows[folders.by_id["3"]];
    assert!(orphan.parent.is_none());
    assert_eq!(orphan.display.as_ref(), "missing/leaf");
    assert_eq!(folders.name("3", Some(&overview)), "/missing/leaf/");
    folders.set_query("a/bc");
    assert_eq!(visible_names(&folders, &overview), ["A/BC"]);
}

#[test]
fn folder_duplicate_paths_have_one_parent_representative_and_independent_ids() {
    let overview = overview(&["A/B", "A", "A/B", "A/B/C"]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    let first = folders.by_id["0"];
    let duplicate = folders.by_id["2"];
    let child = folders.by_id["3"];
    assert_eq!(folders.rows[child].parent, Some(first));
    assert_eq!(folders.children[first].as_ref(), &[child]);
    assert!(folders.children[duplicate].is_empty());
    assert_eq!(folders.rows[first].parent, folders.rows[duplicate].parent);
    assert_eq!(folders.name("2", Some(&overview)), "A/B");
}

#[test]
fn folder_cache_reuses_snapshot_query_and_trail_and_refreshes_removed_ids() {
    let overview = overview(&["A", "A/B", "A/B/C"]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    folders
        .manager
        .select(Some("1"), &folders.by_id, &folders.rows);
    folders.update_visible();
    let rows = folders.rows.clone();
    let visible = folders.visible.clone();
    folders.refresh(1, Some(&overview));
    assert!(Rc::ptr_eq(&rows, &folders.rows));
    assert!(Rc::ptr_eq(&visible, &folders.visible));
    let trail = folders.manager.trail.clone();
    folders
        .manager
        .select(Some("1"), &folders.by_id, &folders.rows);
    assert!(Rc::ptr_eq(&trail, &folders.manager.trail));
    folders.set_query("a/b");
    let visible = folders.visible.clone();
    assert!(!folders.set_query(" A/B "));
    assert!(Rc::ptr_eq(&visible, &folders.visible));
    folders.selected = Some("2".into());
    let replacement = Overview {
        folders: vec![Folder {
            id: "replacement".into(),
            name: "A/B/new".into(),
        }],
        ..overview
    };
    folders.refresh(2, Some(&replacement));
    assert_eq!(visible_names(&folders, &replacement), ["A/B/new"]);
    assert!(folders.manager.id.is_none());
    assert!(folders.manager.trail.is_empty());
    assert!(folders.selected.is_none());
    folders.refresh(3, None);
    assert!(folders.rows.is_empty());
    assert!(folders.visible.is_empty());
}

#[test]
fn folder_search_clears_hidden_mutation_selection_without_changing_navigation() {
    let overview = overview(&["A", "A/B", "X"]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    folders
        .manager
        .select(Some("0"), &folders.by_id, &folders.rows);
    folders.selected = Some("1".into());
    folders.set_query("X");
    assert!(folders.selected.is_none());
    assert_eq!(folders.manager.id.as_deref(), Some("0"));
    assert_eq!(visible_names(&folders, &overview), ["X"]);
    folders.set_query("");
    assert_eq!(visible_names(&folders, &overview), ["A/B"]);
}

#[test]
fn folder_deep_trail_retains_every_real_ancestor_and_bounds_display_text() {
    let mut path = String::new();
    let overview = Overview {
        folders: (0..1500)
            .map(|index| {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str("甲😀");
                Folder {
                    id: index.to_string(),
                    name: path.clone(),
                }
            })
            .collect(),
        ..overview(&[])
    };
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    folders
        .vault
        .select(Some("1499"), &folders.by_id, &folders.rows);
    assert_eq!(folders.vault.trail.len(), 1500);
    for (expected, index) in folders.vault.trail.iter().enumerate() {
        assert_eq!(folders.rows[*index].source, expected);
    }
    assert_eq!(folders.vault.trail.len().saturating_sub(8), 1492);
    assert!(
        folders
            .rows
            .iter()
            .all(|row| row.full_display.chars().count() <= 160)
    );
    assert_eq!(folders.name("1499", Some(&overview)), path);
    let trail = folders.vault.trail.clone();
    folders
        .vault
        .select(Some("1499"), &folders.by_id, &folders.rows);
    assert!(Rc::ptr_eq(&trail, &folders.vault.trail));
}

#[test]
fn folder_projection_does_not_treat_empty_or_duplicate_ids_as_navigable_folders() {
    let overview = Overview {
        folders: vec![
            Folder {
                id: String::new(),
                name: "A".into(),
            },
            Folder {
                id: "real".into(),
                name: "A/B".into(),
            },
            Folder {
                id: "real".into(),
                name: "other".into(),
            },
        ],
        ..overview(&[])
    };
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    assert_eq!(folders.rows.len(), 1);
    assert!(!folders.by_id.contains_key(""));
    assert_eq!(visible_names(&folders, &overview), ["A/B"]);
    assert!(folders.rows[0].parent.is_none());
    folders
        .vault
        .select(Some("missing"), &folders.by_id, &folders.rows);
    assert!(folders.vault.trail.is_empty());
    assert!(folders.vault.id.is_none());
}

#[test]
fn folder_open_filter_remains_exact_with_descendants_and_duplicate_names() {
    use crate::vault_list::{Filter, Index, Sort};
    use bytewarden_core::desktop::Item;
    let overview = overview(&["A", "A/B", "A"]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    let items = [Some("0"), Some("1"), Some("2"), None]
        .into_iter()
        .enumerate()
        .map(|(index, folder_id)| Item {
            id: index.to_string(),
            name: index.to_string(),
            subtitle: String::new(),
            item_type: 1,
            favorite: false,
            folder_id: folder_id.map(str::to_owned),
            organization_id: None,
            deleted: false,
            archived: false,
            reprompt: false,
            restricted: false,
            revision_date: String::new(),
            website_host: None,
        })
        .collect::<Vec<_>>();
    let mut index = Index::new(&items);
    for (id, expected) in [
        ("0", vec![0]),
        ("1", vec![1]),
        ("2", vec![2]),
        ("", vec![3]),
    ] {
        assert_eq!(
            index.visible(&items, Filter::All, Some(id), Sort::NameAsc, ""),
            expected
        );
    }
    assert_eq!(
        folders.name("0", Some(&overview)),
        folders.name("2", Some(&overview))
    );
}

#[test]
fn folder_display_abbreviates_long_unicode_without_changing_full_name_access() {
    let long = format!("开始/{}结束", "😀甲".repeat(200));
    let overview = overview(&[&long, "   "]);
    let mut folders = Folders::default();
    folders.refresh(1, Some(&overview));
    let row = &folders.rows[folders.by_id["0"]];
    assert!(row.full_display.chars().count() <= 160);
    assert!(row.full_display.starts_with("开始/"));
    assert!(row.full_display.ends_with("结束"));
    assert!(row.full_display.contains('…'));
    assert_eq!(row.full_name.as_ref(), long);
    assert_eq!(folders.name("0", Some(&overview)), long);
    assert_eq!(folders.rows[folders.by_id["1"]].full_display.as_ref(), "—");
}
