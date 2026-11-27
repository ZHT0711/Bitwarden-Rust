use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn rendered_results_and_history_are_revoked_before_the_next_frame(gpui: &mut TestAppContext) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    for cleanup in 0..3 {
        view.update(cx, |view, cx| {
            crate::workflow_tests::ready(view);
            view.page = Page::Generator;
            view.generator.result = Some(Zeroizing::new("synthetic-output-91!中文".into()));
            view.generator
                .history
                .record("synthetic-history-37!😀", "密码");
            cx.notify();
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            let retained = view.generator.rendered_secrets.retained_buffers_for_test();
            assert!(
                retained
                    .iter()
                    .any(|buffer| buffer.borrow().as_str() == "synthetic-output-91!中文")
            );
            assert!(
                retained
                    .iter()
                    .any(|buffer| buffer.borrow().as_str() == "synthetic-history-37!😀")
            );
            match cleanup {
                0 => view.generator.cancel_generation(),
                1 => view.generator.clear_history(),
                _ => view.clear_account_view(cx),
            }
            assert!(
                retained.iter().all(|buffer| buffer.borrow().is_empty()),
                "even buffers retained by the rendered frame must be revoked synchronously"
            );
            if cleanup != 0 {
                assert!(view.generator.history.0.is_empty());
            }
            if cleanup != 1 {
                assert!(view.generator.result.is_none());
            }
        });
        cx.update(|window, _| {
            assert!(
                !window
                    .text_system()
                    .contains_cached_text_for_test("synthetic-output-91!中文")
            );
            assert!(
                !window
                    .text_system()
                    .contains_cached_text_for_test("synthetic-history-37!😀")
            );
        });
    }
}

#[gpui::test]
fn account_cleanup_zeros_username_inputs_kept_alive_by_ime_or_hidden_frames(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    view.update(cx, |view, cx| {
        crate::workflow_tests::ready(view);
        view.ensure_username_fields(cx);
        let fields = view.generator.username_fields.as_ref().unwrap();
        let retained = [
            fields.email.clone(),
            fields.domain.clone(),
            fields.website.clone(),
        ];
        for input in &retained {
            input.update(cx, |input, cx| {
                input.set("synthetic-private@example.test", cx)
            });
        }
        view.clear_account_view(cx);
        assert!(view.generator.username_fields.is_none());
        assert!(
            retained
                .iter()
                .all(|input| input.read(cx).value().is_empty())
        );
    });
}
