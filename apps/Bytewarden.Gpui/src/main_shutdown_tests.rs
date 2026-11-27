use super::{Duration, Instant, should_retry_clipboard_clear};

#[test]
fn clipboard_exit_cleanup_retries_only_inside_the_bounded_window() {
    let now = Instant::now();
    let deadline = now + Duration::from_secs(5);

    assert!(should_retry_clipboard_clear(false, now, deadline));
    assert!(!should_retry_clipboard_clear(true, now, deadline));
    assert!(!should_retry_clipboard_clear(false, deadline, deadline));
    assert!(!should_retry_clipboard_clear(
        false,
        deadline + Duration::from_secs(1),
        deadline
    ));
}
