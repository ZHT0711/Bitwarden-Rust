//! Exact closed-form spring physics for lively, interruptible UI animations.
//!
//! Solves `x'' + 2ζω x' + ω² (x - target) = 0` analytically without per-frame
//! integration drift or heap allocations. Retargeting mid-flight preserves both
//! position and instantaneous velocity so interrupted transitions feel silky
//! and natural. Settled springs snap cleanly to their target and stop
//! requesting frames (0% idle CPU).

use std::time::{Duration, Instant};

/// Physical spring parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfig {
    /// Spring stiffness `k` (higher = snappier launch).
    pub stiffness: f32,
    /// Damping coefficient `c` (lower = more playful bounce).
    pub damping: f32,
    /// Position tolerance for settling.
    pub epsilon: f32,
}

#[allow(dead_code)]
impl SpringConfig {
    /// Crisp with subtle, playful overshoot (~6%). Ideal for toasts, modals,
    /// command palette pop-ins, active indicator pills, and favorite pulses.
    pub const SNAPPY: Self = Self {
        stiffness: 420.,
        damping: 26.,
        epsilon: 0.001,
    };

    /// Fast launch with magnetic settle (~1.5% micro-settle). Ideal for detail
    /// cards, page transitions, inline editor, and dropdown popovers.
    pub const FLUID: Self = Self {
        stiffness: 320.,
        damping: 29.,
        epsilon: 0.001,
    };

    /// Plush critically-damped glide (0% overshoot). Ideal for sidebar width
    /// and dimmed overlay backdrops.
    pub const GENTLE: Self = Self {
        stiffness: 220.,
        damping: 29.5,
        epsilon: 0.001,
    };

    /// Ultra-responsive micro-feedback (~160ms). Ideal for copy checkmark
    /// morphs, toggle switch thumbs, and hover emphasis.
    pub const QUICK: Self = Self {
        stiffness: 560.,
        damping: 34.,
        epsilon: 0.001,
    };
}

/// Analytical single-axis spring state.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    start_pos: f32,
    start_vel: f32,
    target: f32,
    started_at: Instant,
    config: SpringConfig,
    settled: bool,
}

impl Spring {
    /// Create a spring already resting at `value`.
    pub fn settled(value: f32, config: SpringConfig, now: Instant) -> Self {
        Self {
            start_pos: value,
            start_vel: 0.,
            target: value,
            started_at: now,
            config,
            settled: true,
        }
    }

    /// Launch a spring from `from` to `to` starting at `now`.
    pub fn launch(from: f32, to: f32, config: SpringConfig, now: Instant) -> Self {
        Self::launch_with_velocity(from, to, 0., config, now)
    }

    /// Launch a spring with an initial impulse velocity (e.g. playful pop or bounce).
    pub fn launch_with_velocity(
        from: f32,
        to: f32,
        velocity: f32,
        config: SpringConfig,
        now: Instant,
    ) -> Self {
        let settled = (from - to).abs() <= config.epsilon && velocity.abs() <= config.epsilon;
        Self {
            start_pos: if settled { to } else { from },
            start_vel: if settled { 0. } else { velocity },
            target: to,
            started_at: now,
            config,
            settled,
        }
    }

    #[allow(dead_code)]
    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// Snap immediately (e.g. when reduced motion is enabled).
    pub fn snap(&mut self, value: f32, now: Instant) {
        self.start_pos = value;
        self.start_vel = 0.;
        self.target = value;
        self.started_at = now;
        self.settled = true;
    }

    /// Smoothly redirect the spring toward `new_target`, preserving current
    /// position and instantaneous velocity for seamless reversal.
    pub fn retarget(&mut self, new_target: f32, now: Instant, animate: bool) {
        if !animate {
            self.snap(new_target, now);
            return;
        }
        if (self.target - new_target).abs() <= self.config.epsilon {
            return;
        }
        let (pos, vel) = self.evaluate(now);
        self.start_pos = pos;
        self.start_vel = vel;
        self.target = new_target;
        self.started_at = now;
        self.settled = false;
    }

    /// Add a lively impulse kick (e.g. star pulse or copy bounce) while returning to `target`.
    pub fn impulse(&mut self, velocity: f32, now: Instant, animate: bool) {
        if !animate {
            return;
        }
        let (pos, vel) = self.evaluate(now);
        self.start_pos = pos;
        self.start_vel = vel + velocity;
        self.started_at = now;
        self.settled = false;
    }

    /// Sample `(value, still_animating)` at `now`.
    pub fn sample(&mut self, now: Instant, animate: bool) -> (f32, bool) {
        let (pos, _, moving) = self.sample_with_velocity(now, animate);
        (pos, moving)
    }

    /// Sample both current position and instantaneous velocity (useful for Fluent
    /// elastic indicator stretch during cross-section flights).
    pub fn sample_with_velocity(&mut self, now: Instant, animate: bool) -> (f32, f32, bool) {
        if !animate {
            self.snap(self.target, now);
            return (self.target, 0., false);
        }
        if self.settled {
            return (self.target, 0., false);
        }
        let elapsed = now.saturating_duration_since(self.started_at).as_secs_f32();
        // Safety cap so a spring can never spin frames indefinitely.
        if elapsed >= 2.0 {
            self.snap(self.target, now);
            return (self.target, 0., false);
        }
        let (pos, vel) = self.evaluate_at(elapsed);
        let disp = (pos - self.target).abs();
        let eps = self.config.epsilon;
        if disp <= eps && vel.abs() <= eps * 12. {
            self.snap(self.target, now);
            (self.target, 0., false)
        } else {
            (pos, vel, true)
        }
    }

    /// Sample a delayed (staggered) view of this spring's trajectory without extra state!
    /// Ideal for staggering cards (Hero at 0ms, Credentials at 35ms, Notes at 70ms).
    pub fn sample_staggered(&self, now: Instant, delay: Duration, animate: bool) -> (f32, bool) {
        if !animate {
            return (self.target, false);
        }
        let total = now.saturating_duration_since(self.started_at);
        if total < delay {
            return (self.start_pos, !self.settled);
        }
        let elapsed = (total - delay).as_secs_f32();
        if elapsed >= 2.0 {
            return (self.target, false);
        }
        let (pos, vel) = self.evaluate_at(elapsed);
        let eps = self.config.epsilon;
        if (pos - self.target).abs() <= eps && vel.abs() <= eps * 12. {
            (self.target, false)
        } else {
            (pos, true)
        }
    }

    fn evaluate(&self, now: Instant) -> (f32, f32) {
        if self.settled {
            return (self.target, 0.);
        }
        let t = now.saturating_duration_since(self.started_at).as_secs_f32();
        self.evaluate_at(t)
    }

    fn evaluate_at(&self, t: f32) -> (f32, f32) {
        let x0 = self.start_pos - self.target;
        let v0 = self.start_vel;
        let k = self.config.stiffness.max(1.);
        let c = self.config.damping.max(0.1);
        let omega = k.sqrt();
        let zeta = c / (2. * omega);

        let (x, v) = if zeta < 0.999 {
            // Underdamped (lively spring with controlled overshoot).
            let wd = omega * (1. - zeta * zeta).sqrt();
            let decay = (-zeta * omega * t).exp();
            let a = x0;
            let b = (v0 + zeta * omega * x0) / wd;
            let cos = (wd * t).cos();
            let sin = (wd * t).sin();
            let x = decay * (a * cos + b * sin);
            let v = decay * ((b * wd - a * zeta * omega) * cos - (a * wd + b * zeta * omega) * sin);
            (x, v)
        } else if zeta <= 1.001 {
            // Critically damped.
            let decay = (-omega * t).exp();
            let a = x0;
            let b = v0 + omega * x0;
            let x = decay * (a + b * t);
            let v = decay * (b - omega * (a + b * t));
            (x, v)
        } else {
            // Overdamped.
            let root = (zeta * zeta - 1.).sqrt();
            let r1 = -omega * (zeta - root);
            let r2 = -omega * (zeta + root);
            let c1 = (v0 - r2 * x0) / (r1 - r2);
            let c2 = x0 - c1;
            let e1 = (r1 * t).exp();
            let e2 = (r2 * t).exp();
            let x = c1 * e1 + c2 * e2;
            let v = c1 * r1 * e1 + c2 * r2 * e2;
            (x, v)
        };
        (self.target + x, v)
    }
}

/// Convert a spring value (which may overshoot slightly past 1.0) into a safe opacity [0, 1].
pub fn clamp_opacity(value: f32) -> f32 {
    value.clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snappy_spring_overshoots_gently_and_settles_without_spinning() {
        let start = Instant::now();
        let mut spring = Spring::launch(0., 1., SpringConfig::SNAPPY, start);
        let mut max_value = 0.0f32;
        let mut settled_ms = None;
        for ms in 0..=1000 {
            let (value, active) = spring.sample(start + Duration::from_millis(ms), true);
            max_value = max_value.max(value);
            if !active {
                settled_ms = Some(ms);
                assert_eq!(value, 1.);
                break;
            }
        }
        assert!(max_value > 1.01 && max_value < 1.15, "max={max_value}");
        assert!(
            settled_ms.is_some_and(|ms| ms < 750),
            "settled_ms={settled_ms:?}"
        );
    }

    #[test]
    fn retarget_mid_flight_is_continuous_and_reduced_motion_snaps() {
        let start = Instant::now();
        let mut spring = Spring::launch(0., 100., SpringConfig::FLUID, start);
        let mid = start + Duration::from_millis(60);
        let (before, active) = spring.sample(mid, true);
        assert!(active && before > 0. && before < 100.);
        spring.retarget(0., mid, true);
        let (after, active_after) = spring.sample(mid, true);
        assert!(active_after);
        assert!((before - after).abs() < 0.01);

        let (snapped, active_reduced) = spring.sample(mid + Duration::from_millis(10), false);
        assert_eq!((snapped, active_reduced), (0., false));
    }

    #[test]
    fn staggered_sampling_delays_start_and_reaches_target() {
        let start = Instant::now();
        let spring = Spring::launch(0., 1., SpringConfig::FLUID, start);
        let delay = Duration::from_millis(45);
        assert_eq!(
            spring.sample_staggered(start + Duration::from_millis(20), delay, true),
            (0., true)
        );
        let (mid, active) =
            spring.sample_staggered(start + Duration::from_millis(100), delay, true);
        assert!(active && mid > 0.);
        assert_eq!(
            spring.sample_staggered(start + Duration::from_millis(900), delay, true),
            (1., false)
        );
    }
}
