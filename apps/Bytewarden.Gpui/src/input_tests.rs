use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn read_only_input_rejects_native_edits_and_composition_but_can_be_unlocked(
    cx: &mut TestAppContext,
) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::multiline("notes", false, cx);
        input.set("draft snapshot", cx);
        input.focus(window);
        input
    });
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_read_only(true, cx);
            input.replace_text_in_range(Some(0..5), "changed", window, cx);
            input.replace_and_mark_text_in_range(None, " composition", None, window, cx);
            assert_eq!(input.value(), "draft snapshot");
            assert!(input.committed());

            input.set_read_only(false, cx);
            input.replace_text_in_range(Some(0..5), "saved", window, cx);
            assert_eq!(input.value(), "saved snapshot");
        });
    });
}

#[gpui::test]
fn multiline_native_input_keeps_crlf_and_moves_across_unicode_graphemes(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Input::multiline("notes", true, cx));
    window
        .update(cx, |input, window, cx| {
            input.set("甲\r\n😀e\u{301}\n", cx);
            assert_eq!(line_ranges(input.value()), [0..3, 5..12, 13..13]);
            input.position(12, false, cx);
            input.erase(false, cx);
            assert_eq!(input.value(), "甲\r\n😀\n"); // e + combining accent delete together.
            input.position(5, false, cx);
            input.erase(false, cx);
            assert_eq!(input.value(), "甲😀\n"); // CRLF is a single grapheme.
            input.replace_and_mark_text_in_range(Some(1..3), "中文😀", Some(2..4), window, cx);
            assert_eq!(input.value(), "甲中文😀\n");
            assert!(!input.committed());
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                3..5
            );
            input.replace_text_in_range(None, "终", window, cx);
            assert_eq!(input.value(), "甲终\n");
            assert!(input.committed());
        })
        .unwrap();
}

#[gpui::test]
fn revealed_secrets_deny_surrounding_text_copy_and_cut_but_allow_paste(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::new("secret", true, cx);
        input.set("synthetic secret", cx);
        input.focus(window);
        input
    });
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_revealed(true, cx);
            input.anchor = 0;
            input.cursor = input.value.len();
            assert!(
                input
                    .text_for_range(0..100, &mut None, window, cx)
                    .is_none()
            );
        });
        cx.write_to_clipboard(ClipboardItem::new_string("clipboard sentinel".into()));
    });
    cx.dispatch_action(Copy);
    cx.dispatch_action(Cut);
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("clipboard sentinel")
        );
        assert_eq!(input.read(cx).value(), "synthetic secret");
    });
    cx.dispatch_action(Paste);
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), "clipboard sentinel"));
}

#[gpui::test]
fn ordinary_copy_does_not_delete_and_single_line_paste_rejects_newlines(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::new("name", false, cx);
        input.set("甲😀乙", cx);
        input.focus(window);
        input.anchor = 0;
        input
    });
    cx.dispatch_action(Copy);
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("甲😀乙")
        );
        assert_eq!(input.read(cx).value(), "甲😀乙");
        cx.write_to_clipboard(ClipboardItem::new_string("two\nlines".into()));
    });
    cx.dispatch_action(Paste);
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), "甲😀乙"));
    cx.dispatch_action(Cut);
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), ""));
}

#[gpui::test]
fn multiline_enter_inserts_a_newline_and_masks_keep_logical_line_offsets(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::multiline("notes", true, cx);
        input.set("甲😀", cx);
        input.focus(window);
        input
    });
    cx.simulate_keystrokes("enter");
    cx.update(|_, cx| {
        assert_eq!(input.read(cx).value(), "甲😀\n");
        let input = input.read(cx);
        let g = input.geometry.as_ref().unwrap();
        assert_eq!(g.lines.len(), 2);
        assert_eq!(
            g.position(input.value(), input.value.len(), true).y,
            g.line_height
        );
        assert_eq!(
            g.offset(
                input.value(),
                g.bounds.origin + point(px(0.), g.line_height) - g.shift,
                true
            ),
            input.value.len()
        );
    });
}

#[gpui::test]
fn windows_word_key_bindings_move_extend_and_delete(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::new("name", false, cx);
        input.set("alpha  beta", cx);
        input.focus(window);
        input
    });
    cx.simulate_keystrokes("ctrl-left");
    cx.update(|_, cx| assert_eq!(input.read(cx).selected(), 7..7));
    cx.simulate_keystrokes("ctrl-shift-left");
    cx.update(|_, cx| {
        let input = input.read(cx);
        assert_eq!((input.anchor, input.cursor), (7, 0));
    });
    cx.simulate_keystrokes("ctrl-shift-right");
    cx.update(|_, cx| assert_eq!(input.read(cx).selected(), 7..7));
    cx.simulate_keystrokes("ctrl-right");
    cx.update(|_, cx| assert_eq!(input.read(cx).selected(), 11..11));
    cx.simulate_keystrokes("ctrl-delete");
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), "alpha  beta"));
    cx.simulate_keystrokes("ctrl-home");
    cx.simulate_keystrokes("ctrl-delete");
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), "beta"));
    cx.simulate_keystrokes("ctrl-backspace");
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), "beta"));
    cx.simulate_keystrokes("ctrl-end");
    cx.simulate_keystrokes("ctrl-backspace");
    cx.update(|_, cx| assert_eq!(input.read(cx).value(), ""));
}

#[gpui::test]
fn word_boundaries_keep_unicode_clusters_and_skip_whitespace(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Input::multiline("notes", false, cx));
    window
        .update(cx, |input, _, cx| {
            let text = "can't\t\u{a0}e\u{301}lan, 😀\r\nnext";
            input.set(text, cx);
            let accent = text.find('e').unwrap();
            let comma = text.find(',').unwrap();
            let emoji = text.find('😀').unwrap();
            let next = text.find("next").unwrap();
            input.position(0, false, cx);
            for expected in [accent, comma, emoji, next, text.len()] {
                input.word_step(true, false, cx);
                assert_eq!(input.cursor, expected);
            }
            for expected in [next, emoji, comma, accent, 0, 0] {
                input.word_step(false, false, cx);
                assert_eq!(input.cursor, expected);
            }
            input.set("👨‍👩‍👧‍👦 next", cx);
            input.word_step(false, false, cx);
            input.word_step(false, false, cx);
            assert_eq!(input.cursor, 0);
            input.erase_word(true, cx);
            assert_eq!(input.value(), "next");
            input.set("甲乙", cx);
            input.word_step(false, false, cx);
            assert_eq!(input.cursor, "甲".len());
            input.word_step(false, false, cx);
            assert_eq!(input.cursor, 0);
        })
        .unwrap();
}

#[gpui::test]
fn word_deletion_prefers_exact_selection_in_both_directions(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Input::new("name", false, cx));
    window
        .update(cx, |input, _, cx| {
            for right in [false, true] {
                for reversed in [false, true] {
                    input.set("one  e\u{301}lan two", cx);
                    let start = 5;
                    let stop = "one  e\u{301}".len();
                    let (anchor, cursor) = if reversed {
                        (stop, start)
                    } else {
                        (start, stop)
                    };
                    input.position(anchor, false, cx);
                    input.position(cursor, true, cx);
                    input.erase_word(right, cx);
                    assert_eq!(input.value(), "one  lan two");
                    assert_eq!(input.selected(), start..start);
                }
            }
            input.set("one  two", cx);
            input.position(5, false, cx);
            input.erase_word(false, cx);
            assert_eq!(input.value(), "two");
            input.set("one  two", cx);
            input.position(2, false, cx);
            input.erase_word(true, cx);
            assert_eq!(input.value(), "ontwo");
        })
        .unwrap();
}

#[gpui::test]
fn hard_line_home_end_preserve_crlf_and_selection_anchor(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Input::multiline("notes", false, cx));
    window
        .update(cx, |input, _, cx| {
            input.set("first\r\n甲😀\nlast\n", cx);
            input.position("first\r\n甲".len(), false, cx);
            input.line_edge(false, false, cx);
            assert_eq!(input.cursor, 7);
            input.line_edge(true, true, cx);
            assert_eq!(input.selected(), 7.."first\r\n甲😀".len());
            input.line_edge(false, true, cx);
            assert_eq!(input.selected(), 7..7);
            input.position(input.value.len(), false, cx);
            input.line_edge(false, false, cx);
            assert_eq!(input.cursor, input.value.len()); // Trailing empty line.
        })
        .unwrap();
}

#[gpui::test]
fn wrapped_home_end_keep_the_caret_on_the_requested_visual_line(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::multiline("notes", false, cx);
        input.set(&"word ".repeat(200), cx);
        input.focus(window);
        input
    });
    cx.simulate_keystrokes("ctrl-home");
    let first_wrap = cx.update(|_, cx| {
        let input = input.read(cx);
        let row = &input.geometry.as_ref().unwrap().lines[0];
        let boundary = row.line.wrap_boundaries.first().expect("long text wraps");
        row.line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
    });
    cx.simulate_keystrokes("end");
    cx.update(|_, cx| {
        let input = input.read(cx);
        let g = input.geometry.as_ref().unwrap();
        assert_eq!(input.cursor, first_wrap);
        assert!(input.cursor_at_wrap_end);
        let position = g.position_with_affinity(input.value(), input.cursor, false, true);
        assert_eq!(position.y, px(0.));
        assert!(position.x > px(0.));
        let hit =
            g.offset_with_affinity(input.value(), g.bounds.origin + position - g.shift, false);
        assert_eq!(hit, (first_wrap, true));
    });
    cx.simulate_keystrokes("home");
    cx.update(|_, cx| assert_eq!(input.read(cx).cursor, 0));
    cx.simulate_keystrokes("shift-end");
    cx.update(|_, cx| assert_eq!(input.read(cx).selected(), 0..first_wrap));
    cx.simulate_keystrokes("shift-home");
    cx.update(|_, cx| assert_eq!(input.read(cx).selected(), 0..0));
    cx.simulate_keystrokes("end");
    cx.simulate_keystrokes("right");
    cx.simulate_keystrokes("home");
    cx.update(|_, cx| assert_eq!(input.read(cx).cursor, first_wrap));
    cx.simulate_keystrokes("ctrl-end");
    cx.simulate_keystrokes("ctrl-shift-home");
    cx.update(|_, cx| {
        let input = input.read(cx);
        assert_eq!(input.selected(), 0..input.value.len());
        assert_eq!(input.cursor, 0);
    });
}

#[gpui::test]
fn page_keys_move_by_the_visible_rows_and_extend_selection(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::multiline("notes", false, cx);
        input.set(&"abcdef\n".repeat(40), cx);
        input.focus(window);
        input
    });
    cx.simulate_keystrokes("ctrl-home");
    for _ in 0..3 {
        cx.simulate_keystrokes("right");
    }
    let rows = cx.update(|_, cx| {
        let g = input.read(cx).geometry.as_ref().unwrap();
        (g.bounds.size.height / g.line_height).floor().max(1.) as usize
    });
    cx.simulate_keystrokes("shift-pagedown");
    cx.update(|_, cx| {
        let input = input.read(cx);
        assert_eq!(input.selected(), 3..3 + rows * 7);
        assert_eq!(input.anchor, 3);
        assert!(input.scroll_y > px(0.));
    });
    cx.simulate_keystrokes("shift-pageup");
    cx.update(|_, cx| assert_eq!(input.read(cx).selected(), 3..3));
    cx.simulate_keystrokes("up");
    cx.update(|_, cx| assert_eq!(input.read(cx).cursor, 0));
    cx.simulate_keystrokes("ctrl-end");
    cx.simulate_keystrokes("shift-pageup");
    cx.update(|_, cx| {
        let input = input.read(cx);
        assert_eq!(input.anchor, input.value.len());
        assert_eq!(input.cursor, input.value.len() - rows * 7);
    });
    cx.simulate_keystrokes("ctrl-end");
    cx.simulate_keystrokes("pagedown");
    cx.update(|_, cx| {
        let input = input.read(cx);
        assert_eq!(input.selected(), input.value.len()..input.value.len());
    });
}

#[gpui::test]
fn hit_testing_never_splits_a_unicode_grapheme_even_when_masked(_: &mut TestAppContext) {
    for text in ["e\u{301}", "👨‍👩‍👧‍👦", "🇨🇳"] {
        for index in 0..text.len() {
            assert_eq!(source_index(text, index, false), 0);
        }
        for bullet in 0..text.chars().count() {
            assert_eq!(source_index(text, bullet * '•'.len_utf8(), true), 0);
        }
        assert_eq!(source_index(text, text.len(), false), text.len());
        assert_eq!(
            source_index(text, text.chars().count() * '•'.len_utf8(), true),
            text.len()
        );
    }
}

#[gpui::test]
fn marked_text_survives_editor_actions_until_native_commit(cx: &mut TestAppContext) {
    cx.update(init);
    let (input, cx) = cx.add_window_view(|window, cx| {
        let mut input = Input::multiline("notes", false, cx);
        input.set("before after", cx);
        input.position(7, false, cx);
        input.replace_and_mark_text_in_range(None, "中文😀", Some(2..4), window, cx);
        input.focus(window);
        input
    });
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
    });
    for key in [
        "left",
        "right",
        "ctrl-left",
        "ctrl-right",
        "ctrl-shift-left",
        "ctrl-shift-right",
        "backspace",
        "delete",
        "ctrl-backspace",
        "ctrl-delete",
        "home",
        "end",
        "shift-home",
        "shift-end",
        "ctrl-home",
        "ctrl-end",
        "ctrl-shift-home",
        "ctrl-shift-end",
        "ctrl-a",
        "ctrl-x",
        "enter",
        "up",
        "down",
        "shift-up",
        "shift-down",
        "pageup",
        "pagedown",
        "shift-pageup",
        "shift-pagedown",
    ] {
        cx.simulate_keystrokes(key);
        cx.update(|_, cx| {
            let input = input.read(cx);
            assert_eq!(input.value(), "before 中文😀after", "{key}");
            assert_eq!(input.marked, Some(7..17), "{key}");
            assert_eq!((input.anchor, input.cursor), (13, 17), "{key}");
        });
    }
    cx.update(|window, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("sentinel")
        );
        input.update(cx, |input, cx| {
            assert_eq!(input.marked_text_range(window, cx), Some(7..11));
            input.replace_text_in_range(None, "终", window, cx);
            assert!(input.committed());
            assert_eq!(input.value(), "before 终after");
            assert_eq!(input.selected(), 10..10);
        });
    });
}
