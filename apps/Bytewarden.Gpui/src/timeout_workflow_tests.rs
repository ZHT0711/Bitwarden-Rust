use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn system_event_entry_preserves_a_nonmatching_worker_and_revokes_a_pending_active_target(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = crate::workflow_tests::window(gpui);
    view.update(cx, |view, cx| {
        crate::workflow_tests::ready(view);
        let background: Account = serde_json::from_value(serde_json::json!({
            "id": "synthetic-background", "email": "background@example.test", "serverMode": 0,
            "serverUrl": "", "state": 1, "active": false, "needsSignIn": false
        }))
        .unwrap();
        view.client.accounts.push(background);
        let now = host::now();
        for account in &view.client.accounts {
            view.timeouts.accounts.insert(
                account.id.clone(),
                Activity {
                    scope: account.settings_scope().expect("synthetic account scope"),
                    last_activity: now,
                    state: Session::Unlocked,
                    policy: Policy {
                        mode: if account.active {
                            Mode::Never
                        } else {
                            Mode::SystemLock
                        },
                        ..Policy::default()
                    },
                },
            );
        }
        view.client.busy = Some("synthetic in-flight write");
        view.client.pending = Some(desktop::Request::begin());
        view.on_timeout_event(Event::Lock, cx);
        assert!(
            view.client.pending.is_some(),
            "background policy must not cancel the active worker"
        );
        assert!(view.client.overview.is_some());
        assert!(
            view.timeouts.events.contains(&Event::Lock),
            "event survives the busy worker"
        );
        view.on_timeout_event(Event::Sleep, cx);
        assert!(view.client.pending.is_some());
        assert!(view.timeouts.events.contains(&Event::Sleep));
        view.timeouts
            .pending
            .insert("review-synthetic-account".into(), Action::Lock);
        view.on_timeout_event(Event::Lock, cx);
        assert!(
            view.client.pending.is_none(),
            "a pending active transition must revoke the worker"
        );
        assert!(view.client.overview.is_none());
        // No native account transition or persistence is needed by this test.
        view.timeouts.pending.clear();
        view.client.busy = None;
    });
}
