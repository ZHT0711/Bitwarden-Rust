//! Controller and keyboard regressions on GPUI's isolated test platform.
use super::*;

#[gpui::test]
fn native_titlebar_presses_do_not_cancel_windows_default_actions(cx: &mut TestAppContext) {
    let (view, cx) = window(cx);
    cx.simulate_resize(size(px(1260.), px(780.)));
    cx.run_until_parked();
    for unlocked in [false, true] {
        if unlocked {
            view.update(cx, |view, cx| {
                ready(view);
                cx.notify();
            });
            cx.run_until_parked();
        }
        for x in [100., 1260. - 115., 1260. - 69., 1260. - 23.] {
            let (propagated, default_prevented) =
                cx.simulate_event_with_result(gpui::MouseDownEvent {
                    position: gpui::point(px(x), px(20.)),
                    button: MouseButton::Left,
                    modifiers: gpui::Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                });
            assert!(propagated, "caption press at {x} must propagate");
            assert!(
                !default_prevented,
                "caption press at {x} must preserve the Windows default action"
            );
        }
    }
}

#[gpui::test]
fn native_titlebar_hit_test_uses_requested_coordinates_after_repaint(cx: &mut TestAppContext) {
    let (view, cx) = window(cx);
    cx.simulate_resize(size(px(1260.), px(780.)));
    cx.run_until_parked();
    for unlocked in [false, true] {
        if unlocked {
            view.update(cx, |view, cx| {
                ready(view);
                cx.notify();
            });
            cx.run_until_parked();
        }
        for round in 0..3 {
            for (x, expected) in [
                (40., WindowControlArea::Drag),
                (1260. - 115., WindowControlArea::Min),
                (1260. - 69., WindowControlArea::Max),
                (1260. - 23., WindowControlArea::Close),
            ] {
                assert_eq!(
                    cx.window_control_area_at(gpui::point(px(x), px(20.))),
                    Some(expected),
                    "requested area at {x}, repaint {round}, unlocked {unlocked}"
                );
            }
            assert_eq!(
                cx.window_control_area_at(gpui::point(px(600.), px(500.))),
                None
            );
            // Leave the cached mouse position over Min, then repaint. Subsequent
            // queries must still use their own positions and the new frame IDs.
            cx.simulate_mouse_move(
                gpui::point(px(1260. - 115.), px(20.)),
                None,
                gpui::Modifiers::none(),
            );
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
        }
    }
}

#[gpui::test]
fn body_clicks_still_focus_the_shortcut_scope(cx: &mut TestAppContext) {
    let (view, cx) = window(cx);
    cx.simulate_resize(size(px(1260.), px(780.)));
    view.update(cx, |view, cx| {
        ready(view);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.read(cx).client.search.read(cx).focus(window);
        assert!(!view.read(cx).focus.is_focused(window));
    });
    cx.simulate_mouse_down(
        gpui::point(px(1000.), px(740.)),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.update(|window, cx| {
        assert!(view.read(cx).focus.is_focused(window));
    });
    cx.simulate_keystrokes("ctrl-k");
    view.read_with(cx, |view, _| assert!(view.ui.palette_open));
}

use bytewarden_core::desktop::{Account, Detail, Overview};
use gpui::{Entity, EntityInputHandler, TestAppContext, VisualTestContext, WindowControlArea};

pub(super) fn window(cx: &mut TestAppContext) -> (Entity<VaultPreview>, &mut VisualTestContext) {
    cx.update(bind_shortcuts);
    cx.add_window_view(|window, cx| {
        let focus = cx.focus_handle();
        focus.focus(window);
        let mut view =
            VaultPreview::initial_state(window, settings::Settings::default(), focus, cx);
        view.animate = false;
        view.settings.website_icons = false;
        view
    })
}

pub(super) fn ready(view: &mut VaultPreview) {
    view.client.initialized = true;
    view.client.accounts = vec![
        serde_json::from_value::<Account>(serde_json::json!({
            "id": "review-synthetic-account", "email": "review@example.test", "serverMode": 0,
            "serverUrl": "", "state": 1, "active": true, "needsSignIn": false
        }))
        .unwrap(),
    ];
    view.client.set_overview(Overview {
        items: vec![item("alpha", false), item("beta", true)],
        folders: Vec::new(),
        last_sync: String::new(),
        failures: 0,
        read_only: false,
    });
}

fn item(name: &str, archived: bool) -> Item {
    Item {
        id: name.into(),
        name: name.into(),
        subtitle: String::new(),
        item_type: 1,
        favorite: false,
        folder_id: None,
        organization_id: None,
        deleted: false,
        archived,
        reprompt: false,
        restricted: false,
        revision_date: "synthetic-revision".into(),
        website_host: None,
    }
}

fn detail() -> Detail {
    Detail {
        name: "synthetic".into(),
        fields: Vec::new(),
        item_type: 1,
        revision_date: String::new(),
        creation_date: String::new(),
        restricted: false,
        password_hidden: false,
        totp_request_id: None,
        attachment_request_id: None,
        totp: None,
        passkeys: Vec::new(),
        attachment_files: Vec::new(),
        can_upload_attachments: false,
        can_delete_attachments: false,
        attachment_failures: 0,
        password_revision_date: None,
        password_strength: None,
        password_history: Vec::new(),
    }
}

#[gpui::test]
fn palette_submit_ignores_empty_results_and_rechecks_state_after_opening(
    gpui: &mut TestAppContext,
) {
    let (view, cx) = window(gpui);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            ready(view);
            view.open_command_palette(window, cx);
            view.ui.palette_query = "synthetic-no-such-command-497125".into();
            view.ui.palette_selected = usize::MAX;
            assert!(view.collect_palette_entries().is_empty());
            view.submit_command_palette(window, cx);
            assert!(view.ui.palette_open);
            assert!(!view.ui.shortcuts_open);
            view.ui.palette_query = "快捷键指南".into();
            view.lock_queued = true;
            view.submit_command_palette(window, cx);
            assert!(!view.ui.shortcuts_open);
            view.lock_queued = false;
            view.client.form = Some(client::Form::EditorPassword);
            view.submit_command_palette(window, cx);
            assert!(!view.ui.shortcuts_open);
            assert!(view.client.busy.is_none());
            view.client.form = None;
            view.submit_command_palette(window, cx);
            assert!(
                view.ui.shortcuts_open,
                "out-of-range selection is clamped only after all guards pass"
            );
            assert!(!view.ui.palette_open);
        })
    });
}

#[gpui::test]
fn palette_enter_uses_the_bound_submit_action_exactly_once(cx: &mut TestAppContext) {
    let (view, cx) = window(cx);
    view.update(cx, |view, cx| {
        ready(view);
        cx.notify();
    });
    cx.simulate_keystrokes("ctrl-k");
    cx.simulate_input("快捷键指南");
    // initial_state deliberately omits production observers; update the same
    // non-sensitive query state that the production input observer maintains.
    view.update(cx, |view, cx| {
        view.ui.palette_query = "快捷键指南".into();
        cx.notify();
    });
    cx.simulate_keystrokes("down enter");
    view.read_with(cx, |view, _| {
        assert!(!view.ui.palette_open);
        assert!(
            view.ui.shortcuts_open,
            "a double execution would toggle the modal closed again"
        );
        assert!(view.client.busy.is_none());
    });
}

#[gpui::test]
fn palette_enter_during_ime_composition_does_not_execute(cx: &mut TestAppContext) {
    let (view, cx) = window(cx);
    view.update(cx, |view, cx| {
        ready(view);
        cx.notify();
    });
    cx.simulate_keystrokes("ctrl-k");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.ui.palette_query = "快捷键指南".into();
            view.ui.palette_input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "快捷键指南", Some(0..5), window, cx);
            });
            cx.notify();
        })
    });
    cx.simulate_keystrokes("enter");
    view.read_with(cx, |view, _| {
        assert!(view.ui.palette_open);
        assert!(!view.ui.shortcuts_open);
    });
}

#[gpui::test]
fn palette_jump_clears_stale_search_and_keeps_archived_target_in_projection(
    cx: &mut TestAppContext,
) {
    let (view, cx) = window(cx);
    for index in [0, 1] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                ready(view);
                view.query = "nonmatching main search".into();
                view.client
                    .search
                    .update(cx, |input, cx| input.set("nonmatching main search", cx));
                view.selected = Some(index);
                view.selected_item_id = Some(if index == 0 { "alpha" } else { "beta" }.into());
                view.client.detail = Some(detail());
                view.execute_palette_action(
                    ui::palette::PaletteAction::OpenItem(index),
                    window,
                    cx,
                );
                assert!(view.query.is_empty());
                assert_eq!(view.client.search.read(cx).value(), "");
                assert!(
                    view.filter
                        == if index == 0 {
                            Filter::All
                        } else {
                            Filter::Archived
                        }
                );
                assert!(view.indices().contains(&index));
                assert_eq!(view.selected, Some(index));
                assert!(view.client.detail.is_some());
                assert!(
                    view.client.busy.is_none(),
                    "same selected detail must not issue a native request"
                );
            })
        });
    }
}
