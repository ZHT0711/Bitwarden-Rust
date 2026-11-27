//! Synthetic controller regressions. No native profile or account is initialized.
use super::*;
use gpui::{EntityInputHandler, TestAppContext};

fn source() -> Draft {
    let mut draft = Draft::default();
    draft.item_type = 1;
    draft.can_edit = true;
    draft.name = "synthetic unsaved draft".into();
    draft.notes = Some("synthetic retained notes".into());
    draft
        .values
        .insert("password".into(), "synthetic retained password".into());
    draft.uris.push(DraftUri {
        uri: Some("https://example.test/?token=synthetic".into()),
        r#match: None,
    });
    draft.custom_fields.push(DraftField {
        name: Some("synthetic label".into()),
        value: Some("synthetic custom secret".into()),
        r#type: 1,
        linked_id: None,
    });
    draft
}

#[gpui::test]
fn ordinary_and_unconfirmed_drafts_reject_list_and_palette_navigation(gpui: &mut TestAppContext) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    for must_resync in [false, true] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                crate::workflow_tests::ready(view);
                view.selected = Some(0);
                view.selected_item_id = Some("alpha".into());
                let mut editor = Editor::new(source(), Zeroizing::default(), cx);
                editor.must_resync = must_resync;
                let name = editor.name.clone();
                view.client.editor = Some(editor);
                let generation = view.client.overview_generation;
                view.choose_item(1, cx);
                view.open_command_palette(window, cx);
                view.execute_palette_action(
                    crate::ui::palette::PaletteAction::OpenItem(1),
                    window,
                    cx,
                );
                assert_eq!(view.selected, Some(0));
                assert_eq!(view.selected_item_id.as_deref(), Some("alpha"));
                assert_eq!(view.client.overview_generation, generation);
                assert!(view.client.busy.is_none());
                assert!(!view.ui.palette_open);
                assert_eq!(name.read(cx).value(), "synthetic unsaved draft");
                let editor = view
                    .client
                    .editor
                    .as_ref()
                    .expect("draft survives every navigation entry");
                assert_eq!(editor.must_resync, must_resync);
                assert_eq!(
                    editor.custom_fields[0].value.read(cx).value(),
                    "synthetic custom secret"
                );
                view.close_form(cx);
            })
        });
    }
}

#[gpui::test]
fn removed_uri_and_custom_rows_wipe_retained_inputs_before_another_frame(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    view.update(cx, |view, cx| {
        crate::workflow_tests::ready(view);
        let mut editor = Editor::new(source(), Zeroizing::default(), cx);
        let retained = [
            editor.uris[0].input.clone(),
            editor.custom_fields[0].name.clone(),
            editor.custom_fields[0].value.clone(),
        ];
        assert!(
            retained
                .iter()
                .all(|input| !input.read(cx).value().is_empty())
        );
        editor.remove_uri(usize::MAX, cx);
        editor.remove_custom(usize::MAX, cx);
        assert_eq!(editor.uris.len(), 1);
        assert_eq!(editor.custom_fields.len(), 1);
        assert!(
            retained
                .iter()
                .all(|input| !input.read(cx).value().is_empty())
        );
        editor.remove_uri(0, cx);
        editor.remove_custom(0, cx);
        assert!(editor.uris.is_empty());
        assert!(editor.custom_fields.is_empty());
        assert!(
            retained
                .iter()
                .all(|input| input.read(cx).value().is_empty())
        );
        view.client.editor = Some(editor);
        view.close_form(cx);
        assert!(
            retained
                .iter()
                .all(|input| input.read(cx).value().is_empty())
        );
    });
}

#[gpui::test]
fn closing_and_account_cleanup_wipe_all_retained_input_entities_before_another_frame(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    for account_cleanup in [false, true] {
        view.update(cx, |view, cx| {
            crate::workflow_tests::ready(view);
            let mut editor = Editor::new(source(), Zeroizing::new("synthetic reprompt".into()), cx);
            assert!(editor.switch_kind(5, cx));
            editor
                .fields
                .iter()
                .find(|(key, _)| *key == "privateKey")
                .unwrap()
                .1
                .input
                .update(cx, |input, cx| input.set("synthetic private key", cx));
            let mut retained = Vec::new();
            editor.for_each_input(|input| retained.push(input.clone()));
            assert!(
                retained
                    .iter()
                    .filter(|input| !input.read(cx).value().is_empty())
                    .count()
                    >= 6
            );
            view.client.editor = Some(editor);
            if account_cleanup {
                view.clear_account_view(cx);
            } else {
                view.close_form(cx);
            }
            assert!(view.client.editor.is_none());
            for input in &retained {
                assert_eq!(
                    input.read(cx).value(),
                    "",
                    "strong entity references must be empty before drawing"
                );
            }
        });
    }
}

#[gpui::test]
fn linked_field_type_switch_blocks_save_and_preserves_the_original_choice(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    view.update(cx, |view, cx| {
        crate::workflow_tests::ready(view);
        let mut draft = source();
        draft.custom_fields.push(DraftField {
            name: Some("linked password".into()),
            value: None,
            r#type: 3,
            linked_id: Some(101),
        });
        view.client.editor = Some(Editor::new(draft, Zeroizing::default(), cx));
        for kind in [3, 2] {
            assert!(view.client.editor.as_mut().unwrap().switch_kind(kind, cx));
            view.save_editor(cx);
            assert!(
                view.client.busy.is_none(),
                "incompatible fields must not reach a native worker"
            );
            assert!(view.client.notice.contains("关联字段与当前条目类型不兼容"));
            let editor = view.client.editor.as_ref().unwrap();
            assert_eq!(editor.custom_fields.last().unwrap().linked_id, Some(101));
            assert_eq!(editor.name.read(cx).value(), "synthetic unsaved draft");
        }
        let editor = view.client.editor.as_mut().unwrap();
        assert!(editor.switch_kind(1, cx));
        assert!(editor.linked_fields_compatible());
        editor.collect(cx);
        assert_eq!(
            editor.draft.custom_fields.last().unwrap().linked_id,
            Some(101)
        );
    });
}

#[gpui::test]
fn uri_construct_collect_keeps_missing_empty_and_present_values_distinct(cx: &mut TestAppContext) {
    cx.update(|cx| {
        for value in [None, Some(""), Some("https://example.test/synthetic")] {
            let source = DraftUri {
                uri: value.map(str::to_owned),
                r#match: Some(2),
            };
            let editor = UriEditor::new(Some(&source), cx);
            assert_eq!(editor.original_missing, value.is_none());
            let collected = editor.collect(cx);
            assert_eq!(collected.uri.as_deref(), value);
            assert_eq!(collected.r#match, Some(2));
        }
    });
}

#[gpui::test]
fn a_queued_editor_reply_cannot_reopen_a_dismissed_prompt_for_the_same_item(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    view.update(cx, |view, cx| {
        crate::workflow_tests::ready(view);
        view.selected = Some(0);
        view.selected_item_id = Some("alpha".into());
        view.client.form = Some(Form::EditorPassword);
        let token = EditorLoad {
            form_generation: view.client.form_generation,
            item_id: "alpha".into(),
            clone_requested: false,
        };
        view.run(
            "synthetic queued editor",
            None,
            cx,
            source,
            move |view, draft, cx| {
                view.finish_editor_load(token, Ok(draft), Zeroizing::default(), cx);
            },
        );
        view.close_form(cx);
        view.selected_item_id = Some("alpha".into());
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.client.busy.is_none());
        assert!(view.client.editor.is_none());
        assert!(view.client.form.is_none());
    });
}

#[gpui::test]
fn saving_keeps_inputs_reachable_frozen_and_recovers_a_failed_native_request(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            crate::workflow_tests::ready(view);
            view.client.editor = Some(Editor::new(source(), Zeroizing::default(), cx));
            view.save_editor(cx);
            let editor = view
                .client
                .editor
                .as_ref()
                .expect("pending editor remains clearable");
            assert_eq!(editor.draft.item_type, 1);
            assert_eq!(editor.draft.name, "synthetic unsaved draft");
            editor.name.update(cx, |input, cx| {
                input.replace_text_in_range(None, "should not be inserted", window, cx)
            });
            assert_eq!(editor.name.read(cx).value(), "synthetic unsaved draft");
        })
    });
    // This process never initializes native accounts; saving can only fail locally.
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.client.busy.is_none());
            let editor = view
                .client
                .editor
                .as_ref()
                .expect("failed save retains the draft");
            assert_eq!(
                editor.draft.values["password"],
                "synthetic retained password"
            );
            editor.name.update(cx, |input, cx| {
                input.replace_text_in_range(None, "now editable", window, cx)
            });
            assert!(editor.name.read(cx).value().contains("now editable"));
        })
    });
}

#[gpui::test]
fn a_completed_save_updates_the_overview_but_a_reply_after_cleanup_is_discarded(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    for cleanup in [false, true] {
        view.update(cx, |view, cx| {
            crate::workflow_tests::ready(view);
            view.client.editor = Some(Editor::new(source(), Zeroizing::default(), cx));
            let mut retained = Vec::new();
            view.client
                .editor
                .as_ref()
                .unwrap()
                .for_each_input(|input| retained.push(input.clone()));
            let form_generation = view.client.form_generation;
            let overview_generation = view.client.overview_generation;
            if cleanup {
                view.clear_account_view(cx);
            }
            let overview = bytewarden_core::desktop::Overview {
                items: Vec::new(),
                folders: Vec::new(),
                last_sync: "synthetic saved overview".into(),
                failures: 0,
                read_only: false,
            };
            view.finish_editor_save(
                form_generation,
                Ok(bytewarden_core::desktop::Mutation { overview, id: None }),
                source(),
                cx,
            );
            assert!(view.client.editor.is_none());
            assert!(
                retained
                    .iter()
                    .all(|input| input.read(cx).value().is_empty())
            );
            if cleanup {
                assert!(view.client.overview.is_none());
            } else {
                assert!(view.client.overview_generation > overview_generation);
                assert_eq!(
                    view.client.overview.as_ref().unwrap().last_sync,
                    "synthetic saved overview"
                );
            }
        });
    }
}
