use super::*;
use gpui::{EntityInputHandler, TestAppContext};

fn detail() -> SendDetail {
    SendDetail {
        summary: SendSummary {
            id: "synthetic-send".into(),
            name: "Synthetic send".into(),
            kind: 0,
            disabled: false,
            deletion_date: "2026-12-02T00:00:00Z".into(),
            expiration_date: None,
            revision: "2026-11-25T00:00:00Z".into(),
        },
        notes: Some("甲\r\n😀乙\n".into()),
        text: Some("第一行\r\nsecond line😀".into()),
        file_name: None,
        hidden: true,
        hide_email: true,
        max_access_count: Some(5),
        access_count: 1,
        auth_type: 0,
        has_password: true,
        emails: Vec::new(),
    }
}

#[gpui::test]
fn multiline_send_content_is_visible_to_the_editor_and_can_be_replaced_or_cleared(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        let original = detail();
        let editor = SendEditor::new(Some(&original), None, cx);
        let draft = editor.draft(cx).unwrap();
        assert_eq!(draft.text, original.text);
        assert_eq!(draft.notes, original.notes);
        assert!(draft.password.is_none());
        assert!(draft.emails.is_none()); // Leave existing access authentication unchanged.
        editor
            .text
            .update(cx, |input, cx| input.set("new\n文本😀", cx));
        editor.notes.update(cx, |input, cx| input.set("", cx));
        let draft = editor.draft(cx).unwrap();
        assert_eq!(draft.text.as_deref(), Some("new\n文本😀"));
        assert!(draft.notes.is_none()); // Clearing must never restore original multiline data.
        assert!(draft.hidden);
        assert!(draft.hide_email);
        assert_eq!(draft.max_access_count, Some(5));
        assert_eq!(original.notes.as_deref(), Some("甲\r\n😀乙\n"));
    });
}

#[gpui::test]
fn invalid_or_uncertain_send_drafts_cannot_submit_and_keep_all_inputs(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut editor = SendEditor::new(Some(&detail()), None, cx);
        editor
            .count
            .update(cx, |input, cx| input.set("not a count", cx));
        assert!(editor.draft(cx).is_none());
        assert_eq!(editor.text.read(cx).value(), "第一行\r\nsecond line😀");
        editor.count.update(cx, |input, cx| input.set("0", cx));
        editor
            .expiration
            .update(cx, |input, cx| input.set("invalid date", cx));
        assert!(editor.draft(cx).is_none());
        editor.expiration.update(cx, |input, cx| input.set("", cx));
        assert!(editor.draft(cx).is_some());
        editor.must_resync = true;
        assert!(editor.draft(cx).is_none());
        assert_eq!(editor.notes.read(cx).value(), "甲\r\n😀乙\n");
    });
}

#[gpui::test]
fn leaving_a_send_editor_clears_even_retained_input_handles(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let editor = SendEditor::new(Some(&detail()), None, cx);
        editor
            .password
            .update(cx, |input, cx| input.set("public synthetic password", cx));
        let retained = editor.inputs().into_iter().cloned().collect::<Vec<_>>();
        let mut page = SendPage {
            editor: Some(editor),
            ..Default::default()
        };
        page.clear(cx);
        assert!(!page.editing());
        for input in retained {
            assert!(input.read(cx).value().is_empty());
        }
    });
}

#[gpui::test]
fn uncertain_send_writes_hide_stale_data_until_refresh_succeeds(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let editor_source = detail();
        let editor = SendEditor::new(Some(&editor_source), None, cx);
        let current = detail();
        let mut page = SendPage {
            list: Some(Sends {
                items: vec![current.summary.clone()],
                failures: 0,
            }),
            detail: Some(current),
            editor: Some(editor),
            ..Default::default()
        };
        page.mark_refresh_required();
        assert!(page.refresh_required);
        assert!(page.list.is_none());
        assert!(page.detail.is_none());
        assert!(page.editing()); // Preserve the submitted draft for explicit cancellation.

        page.begin_refresh(cx);
        assert!(page.refresh_required); // A started or failed refresh is not confirmation.
        assert!(!page.editing());
        page.apply_refresh(Sends {
            items: Vec::new(),
            failures: 0,
        });
        assert!(!page.refresh_required);
        assert!(page.list.is_some());
    });
}

#[gpui::test]
fn composing_send_content_cannot_be_saved(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Input::multiline("composition host", false, cx));
    window
        .update(cx, |_, window, cx| {
            let editor = SendEditor::new(None, None, cx);
            editor.text.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "中文", None, window, cx)
            });
            assert!(!editor.committed(cx));
            assert!(editor.draft(cx).is_none());
            editor.text.update(cx, |input, cx| {
                input.replace_text_in_range(None, "中文", window, cx)
            });
            assert!(editor.committed(cx));
            assert_eq!(editor.draft(cx).unwrap().text.as_deref(), Some("中文"));
        })
        .unwrap();
}
