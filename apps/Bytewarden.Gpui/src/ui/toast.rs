//! Animated floating toast notifications with live clipboard countdown bar.

use std::time::{Duration, Instant};

use super::spring::{Spring, SpringConfig};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub kind: ToastKind,
    pub message: String,
    pub created_at: Instant,
    pub duration: Duration,
    /// Optional countdown duration (e.g., clipboard auto-clear timer) shown as a live bar.
    pub countdown: Option<Duration>,
    spring: Spring,
    dismissing: bool,
}

#[derive(Default)]
pub struct ToastState {
    active: Option<Toast>,
}

pub struct ToastFrame<'a> {
    pub toast: &'a Toast,
    /// Spring progress in ~[0, 1] (may briefly overshoot past 1.0 on snappy pop-in).
    pub progress: f32,
    /// Remaining fraction [0, 1] for live progress bar.
    pub bar_fraction: f32,
    /// Remaining seconds for display when countdown is active.
    pub remaining_secs: Option<u64>,
}

impl ToastState {
    pub fn show(
        &mut self,
        kind: ToastKind,
        message: impl Into<String>,
        now: Instant,
        animate: bool,
    ) {
        self.show_with_duration(
            kind,
            message,
            Duration::from_millis(3200),
            None,
            now,
            animate,
        );
    }

    pub fn show_clipboard(
        &mut self,
        label: &str,
        clear_after: Duration,
        now: Instant,
        animate: bool,
    ) {
        let secs = clear_after.as_secs();
        let message = format!("已复制{label} · {secs} 秒后自动清除剪贴板");
        self.show_with_duration(
            ToastKind::Success,
            message,
            Duration::from_millis(4200),
            Some(clear_after),
            now,
            animate,
        );
    }

    pub fn show_with_duration(
        &mut self,
        kind: ToastKind,
        message: impl Into<String>,
        duration: Duration,
        countdown: Option<Duration>,
        now: Instant,
        animate: bool,
    ) {
        let spring = if animate {
            Spring::launch(0., 1., SpringConfig::SNAPPY, now)
        } else {
            Spring::settled(1., SpringConfig::SNAPPY, now)
        };
        self.active = Some(Toast {
            kind,
            message: message.into(),
            created_at: now,
            duration,
            countdown,
            spring,
            dismissing: false,
        });
    }

    pub fn dismiss(&mut self, now: Instant, animate: bool) {
        let Some(toast) = self.active.as_mut() else {
            return;
        };
        if !animate {
            self.active = None;
            return;
        }
        if !toast.dismissing {
            toast.dismissing = true;
            toast.spring.retarget(0., now, true);
        }
    }

    /// Advance animation & expiration. Returns `(Option<ToastFrame>, needs_animation_frame)`.
    pub fn sample(&mut self, now: Instant, animate: bool) -> (Option<ToastFrame<'_>>, bool) {
        let should_remove = if let Some(toast) = self.active.as_mut() {
            let elapsed = now.saturating_duration_since(toast.created_at);
            if !toast.dismissing && elapsed >= toast.duration {
                if animate {
                    toast.dismissing = true;
                    toast.spring.retarget(0., now, true);
                    false
                } else {
                    true
                }
            } else {
                false
            }
        } else {
            return (None, false);
        };

        if should_remove {
            self.active = None;
            return (None, false);
        }

        let (progress, spring_active, expired_after_exit) = {
            let toast = self.active.as_mut().expect("toast present");
            let (progress, active) = toast.spring.sample(now, animate);
            let done_exit = toast.dismissing && (!active || progress <= 0.01);
            (progress, active, done_exit)
        };

        if expired_after_exit {
            self.active = None;
            return (None, false);
        }

        let toast = self.active.as_ref().expect("toast present");
        let elapsed = now.saturating_duration_since(toast.created_at);
        let bar_total = toast.countdown.unwrap_or(toast.duration);
        let bar_fraction = if bar_total.is_zero() {
            0.
        } else {
            (1. - elapsed.as_secs_f32() / bar_total.as_secs_f32()).clamp(0., 1.)
        };
        let remaining_secs = toast.countdown.map(|total| {
            total
                .saturating_sub(elapsed)
                .as_secs()
                .saturating_add(u64::from(!total.saturating_sub(elapsed).is_zero()))
        });
        // Keep animating smoothly while toast is live so the progress bar glides smoothly,
        // and stop completely once dismissed/removed.
        let needs_frame = animate && (spring_active || !toast.dismissing);
        (
            Some(ToastFrame {
                toast,
                progress,
                bar_fraction,
                remaining_secs,
            }),
            needs_frame,
        )
    }

    /// True if a toast is visible and needs a timer wake-up even when animations are off.
    pub fn pending_expiry(&self, now: Instant) -> Option<Duration> {
        let toast = self.active.as_ref()?;
        if toast.dismissing {
            return None;
        }
        let elapsed = now.saturating_duration_since(toast.created_at);
        Some(toast.duration.saturating_sub(elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_enters_counts_down_and_auto_dismisses_cleanly() {
        let start = Instant::now();
        let mut state = ToastState::default();
        state.show_clipboard("密码", Duration::from_secs(30), start, true);
        let (frame, animating) = state.sample(start + Duration::from_millis(50), true);
        assert!(animating);
        let frame = frame.expect("toast visible");
        assert!(frame.bar_fraction > 0.9);
        assert!(frame.remaining_secs.is_some());

        // Past display duration triggers exit spring and eventually removes toast (0 idle frames).
        let (_, _) = state.sample(start + Duration::from_secs(5), true);
        let (after, still_animating) = state.sample(start + Duration::from_secs(7), true);
        assert!(after.is_none());
        assert!(!still_animating);
    }
}
