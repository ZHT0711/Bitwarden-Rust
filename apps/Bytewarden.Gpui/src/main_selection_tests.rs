use super::*;

fn item(id: &str, name: &str) -> Item {
    Item {
        id: id.into(),
        name: name.into(),
        subtitle: String::new(),
        item_type: 1,
        favorite: false,
        folder_id: None,
        organization_id: None,
        deleted: false,
        archived: false,
        reprompt: false,
        restricted: false,
        revision_date: "2026-11-26T00:00:00Z".into(),
        website_host: None,
    }
}

fn selection(ids: &[&str]) -> VaultSelection {
    VaultSelection {
        active: true,
        ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        ..VaultSelection::default()
    }
}

fn retain_projection(
    selection: &mut VaultSelection,
    items: &[Item],
    filter: Filter,
    folder: Option<&str>,
    sort: Sort,
    query: &str,
) {
    let mut index = vault_list::Index::new(items);
    let visible = index.visible(items, filter, folder, sort, query);
    selection.retain_visible(visible.iter().map(|index| items[*index].id.as_str()));
}

#[test]
fn empty_trash_batches_are_bounded_and_preserve_every_item_once() {
    let ids = (0..1_001)
        .map(|index| index.to_string())
        .collect::<Vec<_>>();
    let batches = ids.chunks(500).collect::<Vec<_>>();
    assert_eq!(
        batches.iter().map(|batch| batch.len()).collect::<Vec<_>>(),
        vec![500, 500, 1]
    );
    assert_eq!(
        batches.into_iter().flatten().cloned().collect::<Vec<_>>(),
        ids
    );
}

#[test]
fn empty_trash_uses_only_deleted_unrestricted_ids_in_stable_order() {
    let mut items = vec![
        item("z", "Zulu"),
        item("active", "Active"),
        item("locked", "Locked"),
        item("a", "Alpha"),
    ];
    items[0].deleted = true;
    items[2].deleted = true;
    items[2].restricted = true;
    items[3].deleted = true;

    assert_eq!(eligible_trash_ids(&items), vec!["a", "z"]);
}

#[test]
fn selection_uses_ids_after_sort_and_snapshot_reordering() {
    let mut selected = selection(&["a", "b"]);
    let mut items = vec![item("a", "Zulu"), item("b", "Alpha"), item("c", "Other")];
    retain_projection(&mut selected, &items, Filter::All, None, Sort::NameAsc, "");
    assert_eq!(
        selected.ids,
        HashSet::from(["a".to_owned(), "b".to_owned()])
    );
    items.swap(0, 2);
    items[0].name = "Updated".into();
    retain_projection(&mut selected, &items, Filter::All, None, Sort::NameDesc, "");
    assert_eq!(
        selected.ids,
        HashSet::from(["a".to_owned(), "b".to_owned()])
    );
    assert!(!selected.ids.contains("c"));
}

#[test]
fn selection_retains_only_visible_ids_under_search_and_folder_filters() {
    let mut items = vec![
        item("a", "Alpha"),
        item("b", "Beta"),
        item("c", "Alpha note"),
    ];
    items[0].folder_id = Some("folder".into());
    items[2].folder_id = Some("other".into());
    let mut selected = selection(&["a", "b", "c"]);
    retain_projection(
        &mut selected,
        &items,
        Filter::All,
        None,
        Sort::NameAsc,
        "alpha",
    );
    assert_eq!(
        selected.ids,
        HashSet::from(["a".to_owned(), "c".to_owned()])
    );
    retain_projection(
        &mut selected,
        &items,
        Filter::All,
        Some("folder"),
        Sort::Recent,
        "",
    );
    assert_eq!(selected.ids, HashSet::from(["a".to_owned()]));
    retain_projection(&mut selected, &items, Filter::All, None, Sort::NameAsc, "");
    assert_eq!(selected.ids, HashSet::from(["a".to_owned()]));
    retain_projection(
        &mut selected,
        &items,
        Filter::All,
        None,
        Sort::NameAsc,
        "no match",
    );
    assert!(selected.ids.is_empty());
    assert!(selected.active);
}

#[test]
fn snapshot_refresh_drops_missing_ids_without_selecting_replacement_indices() {
    let mut selected = selection(&["gone", "retained"]);
    let items = vec![
        item("replacement", "Same position"),
        item("retained", "Changed title"),
    ];
    retain_projection(&mut selected, &items, Filter::All, None, Sort::Oldest, "");
    assert_eq!(selected.ids, HashSet::from(["retained".to_owned()]));
}

#[test]
fn partial_success_removes_only_confirmed_ids_and_empty_error_keeps_selection() {
    let mut selected = selection(&["done", "failed", "unattempted"]);
    selected.confirmed(&[]);
    assert_eq!(selected.ids.len(), 3);
    selected.confirmed(&["done".into()]);
    assert_eq!(
        selected.ids,
        HashSet::from(["failed".to_owned(), "unattempted".to_owned()])
    );
    selected.confirmed(&["done".into()]);
    assert_eq!(selected.ids.len(), 2);
}

#[test]
fn leaving_selection_mode_clears_pending_confirmation_but_keeps_sync_gate() {
    let mut selected = selection(&["a"]);
    selected.blocked_generation = Some(42);
    selected.pending = Some(SelectionOperation {
        action: SelectionAction::Item(ItemAction::Delete),
        items: vec![("a".into(), "revision".into())],
        folder: None,
        empty_trash: false,
    });
    selected.needs_password = true;
    selected.move_picker = true;
    selected.clear();
    assert!(!selected.active);
    assert!(selected.ids.is_empty());
    assert!(selected.pending.is_none());
    assert!(!selected.needs_password);
    assert!(!selected.move_picker);
    assert_eq!(selected.blocked_generation, Some(42));
}

#[test]
fn destructive_actions_require_correct_deleted_state_and_reject_restricted_items() {
    let mut entry = item("a", "Alpha");
    assert!(SelectionAction::Item(ItemAction::Trash).allowed(&entry));
    assert!(!SelectionAction::Item(ItemAction::Delete).allowed(&entry));
    assert!(!SelectionAction::Item(ItemAction::Restore).allowed(&entry));
    entry.deleted = true;
    assert!(!SelectionAction::Move.allowed(&entry));
    assert!(!SelectionAction::Item(ItemAction::Archive).allowed(&entry));
    assert!(SelectionAction::Item(ItemAction::Delete).allowed(&entry));
    assert!(SelectionAction::Item(ItemAction::Restore).allowed(&entry));
    entry.restricted = true;
    for action in [
        ItemAction::Favorite,
        ItemAction::Unfavorite,
        ItemAction::Archive,
        ItemAction::Unarchive,
        ItemAction::Trash,
        ItemAction::Restore,
        ItemAction::Delete,
    ] {
        assert!(!SelectionAction::Item(action).allowed(&entry));
    }
}

#[test]
fn uncertainty_and_conflicts_require_sync_but_password_errors_allow_explicit_reverify() {
    for code in [
        "cancelled",
        "timedOut",
        "writeUncertain",
        "conflict",
        "notSynced",
    ] {
        assert!(selection_requires_sync(code));
    }
    for code in [
        "passwordRequired",
        "wrongPassword",
        "permissionDenied",
        "validation",
    ] {
        assert!(!selection_requires_sync(code));
    }
}
