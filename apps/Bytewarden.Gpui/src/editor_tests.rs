//! State-transition tests using GPUI's isolated test platform, not a real account.
use super::*;
use gpui::{EntityInputHandler, TestAppContext};

fn draft(kind: u8) -> Draft {
    let mut draft = Draft::default();
    draft.item_type = kind;
    draft.name = "synthetic editor".into();
    draft
}

fn write(editor: &Editor, key: &str, value: &str, cx: &mut gpui::App) {
    editor
        .fields
        .iter()
        .find(|(field, _)| *field == key)
        .unwrap()
        .1
        .input
        .update(cx, |input, cx| input.set(value, cx));
}

#[gpui::test]
fn cloning_clears_server_identity_and_passkeys_while_preserving_editable_content(
    cx: &mut TestAppContext,
) {
    cx.update(|_| {
        let mut source = draft(1);
        source.id = Some("synthetic-source-id".into());
        source.revision_date = Some("synthetic-revision".into());
        source.folder_id = Some("synthetic-folder-id".into());
        source.favorite = true;
        source.notes = Some("synthetic notes".into());
        source
            .values
            .insert("username".into(), "synthetic user".into());
        source.uris.push(DraftUri {
            uri: Some("https://example.test".into()),
            r#match: Some(2),
        });
        source.custom_fields.push(DraftField {
            name: Some("label".into()),
            value: Some("synthetic value".into()),
            r#type: 0,
            linked_id: None,
        });
        source.passkeys = Some(vec![bytewarden_core::desktop::Passkey {
            relying_party: "example.test".into(),
            user_name: Some("synthetic user".into()),
            created_at: "synthetic timestamp".into(),
        }]);
        source.remove_saved_passkey = true;
        source.can_edit = false;
        source.is_deleted = true;
        source.edit_blocked_reason = Some("synthetic restriction".into());

        let cloned = clone_draft(source);
        assert_eq!(cloned.id, None);
        assert_eq!(cloned.revision_date, None);
        assert_eq!(cloned.name, "synthetic editor - 副本");
        assert_eq!(cloned.folder_id.as_deref(), Some("synthetic-folder-id"));
        assert!(cloned.favorite);
        assert_eq!(cloned.notes.as_deref(), Some("synthetic notes"));
        assert_eq!(cloned.values.get("username").unwrap(), "synthetic user");
        assert_eq!(cloned.uris[0].uri.as_deref(), Some("https://example.test"));
        assert_eq!(
            cloned.custom_fields[0].value.as_deref(),
            Some("synthetic value")
        );
        assert!(cloned.passkeys.is_none());
        assert!(!cloned.remove_saved_passkey);
        assert!(cloned.can_edit);
        assert!(!cloned.is_deleted);
        assert_eq!(cloned.edit_blocked_reason, None);
    });
}

#[gpui::test]
fn type_round_trips_preserve_independent_values_and_save_only_the_active_schema(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        let mut editor = Editor::new(draft(1), Zeroizing::default(), cx);
        write(&editor, "username", "login username", cx);
        write(&editor, "password", "synthetic password", cx);
        editor.collect(cx); // A failed save can already have populated draft.values.
        assert!(editor.switch_kind(4, cx));
        write(&editor, "username", "identity username", cx);
        write(&editor, "firstName", "名字", cx);
        editor.collect(cx);
        assert_eq!(
            editor.draft.values.get("username").unwrap(),
            "identity username"
        );
        assert!(!editor.draft.values.contains_key("password"));
        assert!(editor.switch_kind(1, cx));
        editor.collect(cx);
        assert_eq!(
            editor.draft.values.get("username").unwrap(),
            "login username"
        );
        assert_eq!(
            editor.draft.values.get("password").unwrap(),
            "synthetic password"
        );
        assert!(!editor.draft.values.contains_key("firstName"));
        assert!(editor.switch_kind(4, cx));
        editor.collect(cx);
        assert_eq!(editor.draft.values.get("firstName").unwrap(), "名字");
        assert_eq!(
            editor.draft.values.get("username").unwrap(),
            "identity username"
        );
    });
}

#[gpui::test]
fn multiline_notes_and_keys_can_be_kept_edited_and_explicitly_cleared(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut source = draft(5);
        source.notes = Some("甲\r\n😀乙\n".into());
        source.values.insert(
            "privateKey".into(),
            "-----BEGIN KEY-----\r\npublic synthetic fixture\r\n-----END KEY-----".into(),
        );
        let mut editor = Editor::new(source, Zeroizing::default(), cx);
        editor.collect(cx);
        assert_eq!(editor.draft.notes.as_deref(), Some("甲\r\n😀乙\n"));
        assert!(editor.draft.values["privateKey"].contains("\r\n"));
        write(&editor, "privateKey", "new\nsynthetic\nkey", cx);
        editor.notes.input.update(cx, |input, cx| input.set("", cx));
        editor.collect(cx);
        assert_eq!(editor.draft.values["privateKey"], "new\nsynthetic\nkey");
        assert_eq!(editor.draft.notes.as_deref(), Some(""));
    });
}

#[gpui::test]
fn unsafe_save_results_keep_the_owned_draft_and_block_further_submission(cx: &mut TestAppContext) {
    cx.update(|cx| {
        for code in ["conflict", "notSynced", "writeUncertain", "network"] {
            let mut editor = Editor::new(draft(1), Zeroizing::default(), cx);
            write(&editor, "password", "preserved synthetic draft", cx);
            editor.collect(cx);
            let sent = std::mem::take(&mut editor.draft);
            editor.restore_after_error(sent, code);
            assert_eq!(editor.draft.name, "synthetic editor");
            assert_eq!(editor.draft.values["password"], "preserved synthetic draft");
            assert_eq!(editor.must_resync, code != "network");
        }
    });
}

#[gpui::test]
fn null_custom_values_follow_the_original_type_and_booleans_always_serialize(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        let original = DraftField {
            name: None,
            value: None,
            r#type: 0,
            linked_id: None,
        };
        let mut field = CustomEditor::new(Some(&original), 1, cx);
        assert_eq!(field.collect(cx).value, None);
        field.set_kind(1, 1, cx);
        assert_eq!(field.collect(cx).value.as_deref(), Some(""));
        field.set_kind(2, 1, cx);
        assert_eq!(field.collect(cx).value.as_deref(), Some("false"));
        field.boolean = true;
        assert_eq!(field.collect(cx).value.as_deref(), Some("true"));
        field.set_kind(3, 1, cx);
        assert_eq!(field.collect(cx).linked_id, Some(100));
        assert_eq!(field.collect(cx).value, None);
    });
}

#[gpui::test]
fn native_composition_prevents_custom_and_item_type_changes(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Input::new("test host", false, cx));
    window
        .update(cx, |_, window, cx| {
            let mut editor = Editor::new(draft(1), Zeroizing::default(), cx);
            let mut field = CustomEditor::new(None, 1, cx);
            field.value.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "中文", None, window, cx);
            });
            assert!(!field.committed(cx));
            field.set_kind(1, 1, cx);
            assert_eq!(field.kind, 0);
            assert!(!field.value.read(cx).committed());
            editor.custom_fields.push(field);
            assert!(!editor.committed(cx));
            assert!(!editor.switch_kind(4, cx));
            assert_eq!(editor.draft.item_type, 1);
            editor.custom_fields[0]
                .value
                .update(cx, |input, cx| input.unmark_text(window, cx));
            assert!(editor.committed(cx));
            assert!(editor.switch_kind(4, cx));
        })
        .unwrap();
}
