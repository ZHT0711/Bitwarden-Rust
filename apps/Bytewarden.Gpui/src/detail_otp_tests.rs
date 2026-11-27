//! Presentation/controller fixtures only. Generation stays in the Rust core.
//! RFC 6238 appendix B uses the public synthetic SHA-1 seed
//! GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ (ASCII "12345678901234567890").
//! At Unix second 59 its eight-digit output is 94287082. The core's
//! reproduces_rfc_codes_and_preserves_the_rotation_boundary test covers generation.

use super::*;
use std::cell::Cell;

fn rfc_code() -> TotpCode {
    TotpCode {
        code: "94287082".into(),
        period: 30,
        valid_from: 30,
        valid_until: 60,
    }
}

fn identity() -> TotpIdentity {
    TotpIdentity {
        request_id: 7,
        account: "synthetic-account".into(),
    }
}

fn scope() -> TotpScope<'static> {
    TotpScope {
        request_id: Some(7),
        account: Some("synthetic-account"),
        allowed: true,
    }
}

fn watch() -> TotpWatch {
    TotpWatch {
        identity: identity(),
        boundary: Some(Task::ready(())),
        countdown: Some(Task::ready(())),
        observation: None,
        release: None,
        refreshing: true,
        stopped: false,
    }
}

#[test]
fn rfc6238_fixture_is_displayable_and_copyable_only_inside_its_window() {
    let code = rfc_code();
    assert_eq!(code.code, "94287082");
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(30))),
        TotpTiming::Current {
            seconds: 30,
            boundary: Duration::from_secs(30),
        }
    );
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(59))),
        TotpTiming::Current {
            seconds: 1,
            boundary: Duration::from_secs(1),
        }
    );
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(60))),
        TotpTiming::Refresh
    );
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(29))),
        TotpTiming::Refresh
    );
}

#[test]
fn fractional_wall_time_schedules_the_boundary_without_a_one_second_delay() {
    let code = rfc_code();
    let now = Duration::from_millis(59_875);
    assert_eq!(
        TotpTiming::at(&code, Some(now)),
        TotpTiming::Current {
            seconds: 1,
            boundary: Duration::from_millis(125),
        }
    );
    for millis in [30_000, 30_999, 45_100, 59_999] {
        let now = Duration::from_millis(millis);
        let TotpTiming::Current { boundary, .. } = TotpTiming::at(&code, Some(now)) else {
            panic!("public RFC fixture must be current");
        };
        assert_eq!(now + boundary, Duration::from_secs(60));
    }
}

#[test]
fn system_clock_failure_and_jumps_do_not_authorize_a_stale_code() {
    let code = rfc_code();
    assert_eq!(TotpTiming::at(&code, None), TotpTiming::Unavailable);
    for second in [0, 29, 60, 3_600, u64::MAX] {
        assert_eq!(
            TotpTiming::at(&code, Some(Duration::from_secs(second))),
            TotpTiming::Refresh
        );
    }
}

#[test]
fn countdown_uses_the_period_from_core_for_non_default_periods() {
    for period in [5, 30, 90] {
        let code = TotpCode {
            code: "synthetic-output".into(),
            period,
            valid_from: 0,
            valid_until: i64::from(period),
        };
        assert_eq!(
            TotpTiming::at(&code, Some(Duration::from_secs(1))),
            TotpTiming::Current {
                seconds: u64::from(period) - 1,
                boundary: Duration::from_secs(u64::from(period) - 1),
            }
        );
    }
}

#[test]
fn malformed_core_windows_fail_closed_without_refresh_loops() {
    let mut code = rfc_code();
    code.period = 0;
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(59))),
        TotpTiming::Unavailable
    );
    code.period = 30;
    code.valid_until = code.valid_from;
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(59))),
        TotpTiming::Unavailable
    );
    code.valid_from = i64::MIN;
    code.valid_until = i64::MAX;
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(59))),
        TotpTiming::Unavailable
    );
    code = rfc_code();
    code.code.clear();
    assert_eq!(
        TotpTiming::at(&code, Some(Duration::from_secs(59))),
        TotpTiming::Unavailable
    );
}

#[test]
fn closed_deleted_or_replaced_detail_rejects_the_previous_watch() {
    assert!(identity().matches(scope()));
    let mut closed = scope();
    closed.request_id = None;
    assert!(!identity().matches(closed));
    let mut replaced = scope();
    replaced.request_id = Some(8);
    assert!(!identity().matches(replaced));
}

#[test]
fn account_switch_and_revoked_access_reject_late_refresh_completion() {
    let mut switched = scope();
    switched.account = Some("another-synthetic-account");
    assert!(!identity().matches(switched));
    let mut signed_out = scope();
    signed_out.account = None;
    assert!(!identity().matches(signed_out));
    let mut locked_or_restricted = scope();
    locked_or_restricted.allowed = false;
    assert!(!identity().matches(locked_or_restricted));
}

#[test]
fn stop_drops_both_timer_handles_and_subscriptions_and_is_idempotent() {
    let drops = Rc::new(Cell::new(0));
    let mut watch = watch();
    for slot in [&mut watch.observation, &mut watch.release] {
        let drops = drops.clone();
        *slot = Some(Subscription::new(move || drops.set(drops.get() + 1)));
    }
    watch.stop();
    watch.stop();
    assert!(watch.stopped);
    assert!(!watch.refreshing);
    assert!(watch.boundary.is_none());
    assert!(watch.countdown.is_none());
    assert!(watch.observation.is_none());
    assert!(watch.release.is_none());
    assert_eq!(drops.get(), 2);
}

#[test]
fn stop_breaks_the_notification_subscription_ownership_cycle() {
    let watch = Rc::new(RefCell::new(watch()));
    let weak = Rc::downgrade(&watch);
    let observed = watch.clone();
    watch.borrow_mut().observation = Some(Subscription::new(move || drop(observed)));
    assert_eq!(Rc::strong_count(&watch), 2);
    watch.borrow_mut().stop();
    assert_eq!(Rc::strong_count(&watch), 1);
    drop(watch);
    assert!(weak.upgrade().is_none());
}
