use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn sync_clears_the_global_guard_but_cannot_resubmit_an_uncertain_folder_form(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    for code in ["conflict", "notSynced", "writeUncertain"] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                crate::workflow_tests::ready(view);
                view.page = crate::Page::Folders;
                view.start_folder(FolderOp::Create, window, cx);
                let form = view
                    .folders
                    .form
                    .as_mut()
                    .expect("normal create entry opens the form");
                form.name
                    .update(cx, |input, cx| input.set("synthetic accepted folder", cx));
                // Same error transition used by the production completion callback.
                form.record_error(code);
                view.client.write_guard = true;
                view.client.overview_error = Some("synthetic uncertain write".into());
                view.accept_synced_overview(Overview {
                    items: Vec::new(),
                    folders: vec![Folder {
                        id: "server-created-id".into(),
                        name: "synthetic accepted folder".into(),
                    }],
                    last_sync: "synthetic reconciliation".into(),
                    failures: 0,
                    read_only: false,
                });
                assert!(!view.client.write_guard);
                assert!(view.client.overview_error.is_none());
                assert!(view.folders.form.as_ref().unwrap().must_reconcile);
                view.save_folder(cx);
                assert!(
                    view.client.busy.is_none(),
                    "old create form must not launch a second write"
                );
                assert!(view.client.notice.contains("避免重复提交"));
                assert_eq!(view.client.overview.as_ref().unwrap().folders.len(), 1);
                cx.notify();
            })
        });
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("cancel-folder")
            .expect("guarded form must remain dismissible");
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        view.read_with(cx, |view, _| assert!(view.folders.form.is_none()));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.start_folder(FolderOp::Create, window, cx);
                assert!(!view.folders.form.as_ref().unwrap().must_reconcile);
                // Duplicated names are legal: fresh operations are independent of the old form.
                view.folders.form = None;
            })
        });
    }
}
