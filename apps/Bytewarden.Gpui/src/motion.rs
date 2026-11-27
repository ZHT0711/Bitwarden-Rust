use std::time::{Duration, Instant};

pub const RAIL: f32 = 48.;
pub const PANEL: f32 = 216.;
pub const VAULT_COMPACT_WIDTH: f32 = 720.;
pub const NAV_MINIMAL_MAX_WIDTH: f32 = 640.;
pub const NAV_COMPACT_MAX_WIDTH: f32 = 1008.;
pub const SIDEBAR_OPEN_TIME: Duration = Duration::from_millis(350);
pub const SIDEBAR_CLOSE_TIME: Duration = Duration::from_millis(120);
pub const CONTENT_TIME: Duration = Duration::from_millis(167);

/// Match NavigationView Auto's minimal / compact / expanded width breakpoints.
pub fn nav_compact(viewport_width: gpui::Pixels) -> bool {
    viewport_width < gpui::px(NAV_COMPACT_MAX_WIDTH)
}

pub fn nav_minimal(viewport_width: gpui::Pixels) -> bool {
    viewport_width <= gpui::px(NAV_MINIMAL_MAX_WIDTH)
}

/// HomePage switches to its single-column VisualState when its outer Grid is
/// below 720 DIP; the viewport width must also exclude the reserved nav pane.
pub fn vault_compact(viewport_width: gpui::Pixels, sidebar_width: f32) -> bool {
    viewport_width - gpui::px(sidebar_width) < gpui::px(VAULT_COMPACT_WIDTH)
}

/// The pane can overlay the page while reserving only its compact rail. Keep
/// this separate from the animated width so opening never changes page layout.
pub struct NavigationFrame {
    pub width: f32,
    pub reserved_width: f32,
    /// Minimal-mode items slide from offscreen; the compact rail stays fixed.
    pub content_offset: f32,
    pub overlay: bool,
    pub animating: bool,
}

#[derive(Default)]
pub struct Sidebar {
    wide_collapsed: bool,
    compact_expanded: bool,
    current: Option<Tween>,
}

impl Sidebar {
    pub fn expanded(&self, compact: bool) -> bool {
        if compact {
            self.compact_expanded
        } else {
            !self.wide_collapsed
        }
    }

    pub fn frame(
        &mut self,
        compact: bool,
        minimal: bool,
        now: Instant,
        animate: bool,
    ) -> NavigationFrame {
        let (width, animating) = self.width(compact, minimal, now, animate);
        let closed_width = if minimal { 0. } else { RAIL };
        NavigationFrame {
            width,
            reserved_width: if compact { closed_width } else { width },
            content_offset: if minimal { width - PANEL } else { 0. },
            overlay: compact && width > closed_width + 0.1,
            animating,
        }
    }

    pub fn width(
        &mut self,
        compact: bool,
        minimal: bool,
        now: Instant,
        animate: bool,
    ) -> (f32, bool) {
        let expanded = self.expanded(compact);
        let target = if expanded {
            PANEL
        } else if minimal {
            0.
        } else {
            RAIL
        };
        let Some(previous) = self.current else {
            self.current = Some(Tween::settled(target, now));
            return (target, false);
        };
        if !animate {
            self.current = Some(Tween::settled(target, now));
            return (target, false);
        }
        if previous.to != target {
            self.current = Some(Tween::navigation(previous.sample(now).0, target, now));
        }
        self.current.expect("sidebar tween initialized").sample(now)
    }

    pub fn dismiss(&mut self) -> bool {
        std::mem::replace(&mut self.compact_expanded, false)
    }

    pub fn toggle(&mut self, compact: bool) {
        if compact {
            self.compact_expanded = !self.compact_expanded;
        } else {
            self.wide_collapsed = !self.wide_collapsed;
        }
    }
}

#[derive(Clone, Copy)]
enum Curve {
    Fluent,
    Navigation,
}

#[derive(Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    curve: Curve,
}

impl Tween {
    pub fn new(from: f32, to: f32, start: Instant, duration: Duration) -> Self {
        Self {
            from,
            to,
            start,
            duration,
            curve: Curve::Fluent,
        }
    }

    fn navigation(from: f32, to: f32, now: Instant) -> Self {
        Self {
            from,
            to,
            start: now,
            duration: if to >= from {
                SIDEBAR_OPEN_TIME
            } else {
                SIDEBAR_CLOSE_TIME
            },
            curve: Curve::Navigation,
        }
    }

    fn settled(value: f32, now: Instant) -> Self {
        Self::new(value, value, now, Duration::ZERO)
    }

    pub fn sample(self, now: Instant) -> (f32, bool) {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed >= self.duration || self.from == self.to {
            return (self.to, false);
        }
        let progress = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let eased = match self.curve {
            Curve::Navigation => navigation_curve(progress),
            Curve::Fluent if self.to >= self.from => entrance(progress),
            Curve::Fluent => exit(progress),
        };
        (self.from + (self.to - self.from) * eased, true)
    }
}

// WinUI 2.7 SplitView's KeySpline="0.1,0.9 0.2,1.0". Here x(s)=0.3s+0.7s³
// has a derivative >= 0.3, so six bounded Newton steps recover s across [0,1]
// without allocations, a frame timer, or an unbounded convergence loop.
fn navigation_curve(progress: f32) -> f32 {
    let x = progress.clamp(0., 1.);
    let mut s = x;
    for _ in 0..6 {
        s -= (s * (0.3 + 0.7 * s * s) - x) / (0.3 + 2.1 * s * s);
    }
    s * (2.7 + s * (-2.4 + 0.7 * s))
}

// Fluent's entering curve is cubic-bezier(0, 0, 0, 1). Its x(s)=s³,
// so the inverse is one cube root; evaluate the Bezier y(s) directly.
// https://learn.microsoft.com/windows/apps/design/motion/timing-and-easing
fn entrance(progress: f32) -> f32 {
    let parameter = progress.clamp(0., 1.).cbrt();
    parameter * parameter * (3. - 2. * parameter)
}

// Fluent's exit curve is cubic-bezier(1, 0, 1, 1): x(s)=1-(1-s)³.
fn exit(progress: f32) -> f32 {
    let parameter = 1. - (1. - progress.clamp(0., 1.)).cbrt();
    parameter * parameter * (3. - 2. * parameter)
}

/// Use the Windows Accessibility → Visual effects → Animation effects preference.
#[cfg(windows)]
pub fn effects_enabled() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(action: u32, param: u32, value: *mut i32, flags: u32) -> i32;
    }
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
    let mut enabled = 0;
    // This SPI action writes exactly one Win32 BOOL to our valid out pointer.
    unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut enabled, 0) != 0 && enabled != 0
    }
}

#[cfg(not(windows))]
pub fn effects_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_layout_matches_the_winui_outer_grid_breakpoint() {
        // HomePage measures the outer Grid: 720 DIP plus the active NavigationView pane.
        assert!(vault_compact(gpui::px(935.), PANEL));
        assert!(!vault_compact(gpui::px(936.), PANEL));
        assert!(vault_compact(gpui::px(767.), RAIL));
        assert!(!vault_compact(gpui::px(768.), RAIL));
        // 800 DIP with the compact rail leaves 752 DIP for HomePage: keep both panes.
        assert!(!vault_compact(gpui::px(800.), RAIL));
    }

    #[test]
    fn winui_navigationview_auto_breakpoints_are_preserved() {
        assert!(nav_minimal(gpui::px(640.)));
        assert!(!nav_minimal(gpui::px(641.)));
        assert!(nav_compact(gpui::px(1007.)));
        assert!(!nav_compact(gpui::px(1008.)));
    }

    #[test]
    fn sidebar_animates_and_reverses_from_its_current_position() {
        let mut bar = Sidebar::default();
        let now = Instant::now();
        assert_eq!(bar.width(false, false, now, true), (PANEL, false));
        bar.toggle(false);
        assert_eq!(bar.width(false, false, now, true), (PANEL, true));
        let middle = now + SIDEBAR_CLOSE_TIME / 2;
        let (width, active) = bar.width(false, false, middle, true);
        assert!(active && (RAIL..PANEL).contains(&width));
        assert!((width - (PANEL + (RAIL - PANEL) * navigation_curve(0.5))).abs() < 0.001);
        bar.toggle(false);
        assert_eq!(bar.width(false, false, middle, true), (width, true));
        assert_eq!(
            bar.width(false, false, middle + SIDEBAR_OPEN_TIME, true),
            (PANEL, false)
        );
    }

    #[test]
    fn compact_wide_and_minimal_pane_preferences_remain_independent() {
        let mut bar = Sidebar::default();
        let now = Instant::now();
        assert_eq!(bar.width(false, false, now, false), (PANEL, false));
        assert_eq!(bar.width(true, false, now, false), (RAIL, false));
        assert_eq!(bar.width(true, true, now, false), (0., false));
        bar.toggle(true);
        assert_eq!(bar.width(true, false, now, false), (PANEL, false));
        assert_eq!(bar.width(true, true, now, false), (PANEL, false));
        bar.toggle(false);
        assert_eq!(bar.width(false, false, now, false), (RAIL, false));
        assert_eq!(bar.width(true, true, now, false), (PANEL, false));
        bar.toggle(true);
        assert_eq!(bar.width(true, true, now, false), (0., false));
    }

    #[test]
    fn compact_overlay_keeps_page_width_through_open_close_and_reversal() {
        let now = Instant::now();
        let mut bar = Sidebar::default();
        let closed = bar.frame(true, false, now, true);
        assert_eq!((closed.width, closed.reserved_width), (48., 48.));
        assert!(!closed.overlay && !closed.animating);
        bar.toggle(true);
        bar.frame(true, false, now, true);
        let middle = now + SIDEBAR_OPEN_TIME / 2;
        let open = bar.frame(true, false, middle, true);
        assert!(open.overlay && open.animating);
        assert_eq!(open.reserved_width, 48.);
        assert!(!vault_compact(gpui::px(800.), open.reserved_width));
        assert!((48.0..216.0).contains(&open.width));
        assert!(bar.dismiss());
        assert!(!bar.dismiss()); // Repeated dismissal cannot reopen an exiting pane.
        let reversed = bar.frame(true, false, middle, true);
        assert_eq!(reversed.width, open.width);
        let closed = bar.frame(true, false, middle + SIDEBAR_CLOSE_TIME, true);
        assert_eq!((closed.width, closed.reserved_width), (48., 48.));
        assert!(!closed.overlay && !closed.animating);
        bar.toggle(true);
        let no_motion = bar.frame(true, false, middle, false);
        assert_eq!((no_motion.width, no_motion.reserved_width), (216., 48.));
        assert!(no_motion.overlay && !no_motion.animating);
    }

    #[test]
    fn minimal_overlay_reserves_no_page_width_and_settles_without_frames() {
        let now = Instant::now();
        let mut bar = Sidebar::default();
        assert_eq!(bar.frame(true, true, now, false).reserved_width, 0.);
        bar.toggle(true);
        let open = bar.frame(true, true, now, false);
        assert_eq!((open.width, open.reserved_width), (216., 0.));
        assert!(open.overlay && !open.animating);
        bar.dismiss();
        let closed = bar.frame(true, true, now, false);
        assert_eq!((closed.width, closed.reserved_width), (0., 0.));
        assert!(!closed.overlay && !closed.animating);
    }

    #[test]
    fn minimal_items_slide_while_compact_items_keep_their_position() {
        let now = Instant::now();
        let mut minimal = Sidebar::default();
        let closed = minimal.frame(true, true, now, true);
        assert_eq!(closed.content_offset, -PANEL);
        minimal.toggle(true);
        minimal.frame(true, true, now, true);
        let middle = now + Duration::from_millis(40);
        let entering = minimal.frame(true, true, middle, true);
        assert!(entering.content_offset > -PANEL && entering.content_offset < 0.);
        // The translated right edge and the clip boundary move together.
        assert_eq!(entering.content_offset + PANEL, entering.width);
        assert_eq!(entering.reserved_width, 0.);
        minimal.dismiss();
        let reversal = minimal.frame(true, true, middle, true);
        assert_eq!(reversal.content_offset, entering.content_offset);
        let closed = minimal.frame(true, true, middle + SIDEBAR_CLOSE_TIME, true);
        assert_eq!(closed.content_offset, -PANEL);
        assert!(!closed.animating && !closed.overlay);

        let mut compact = Sidebar::default();
        compact.frame(true, false, now, true);
        compact.toggle(true);
        compact.frame(true, false, now, true);
        assert_eq!(compact.frame(true, false, middle, true).content_offset, 0.);
        minimal.toggle(true);
        assert_eq!(minimal.frame(true, true, middle, false).content_offset, 0.);
    }

    #[test]
    fn navigation_spline_matches_winui_template_parametric_samples() {
        for step in 0..=1000 {
            let s = step as f32 / 1000.;
            let r = 1. - s;
            let x = 3. * r * r * s * 0.1 + 3. * r * s * s * 0.2 + s * s * s;
            let y = 3. * r * r * s * 0.9 + 3. * r * s * s + s * s * s;
            assert!((navigation_curve(x) - y).abs() < 0.000002, "s={s}");
        }
        let now = Instant::now();
        let opening = Tween::navigation(RAIL, PANEL, now);
        let closing = Tween::navigation(PANEL, RAIL, now);
        assert!(
            opening
                .sample(now + SIDEBAR_OPEN_TIME - Duration::from_nanos(1))
                .1
        );
        assert_eq!(opening.sample(now + SIDEBAR_OPEN_TIME), (PANEL, false));
        assert_eq!(closing.sample(now + SIDEBAR_CLOSE_TIME), (RAIL, false));
    }

    #[test]
    fn fluent_entrance_and_exit_curves_are_monotone_with_exact_endpoints() {
        assert_eq!(entrance(0.), 0.);
        assert_eq!(entrance(1.), 1.);
        assert_eq!(exit(0.), 0.);
        assert_eq!(exit(1.), 1.);
        let mut previous_entrance = 0.;
        let mut previous_exit = 0.;
        for frame in 0..=1000 {
            let progress = frame as f32 / 1000.;
            let entering = entrance(progress);
            let leaving = exit(progress);
            assert!((previous_entrance..=1.).contains(&entering));
            assert!((previous_exit..=1.).contains(&leaving));
            previous_entrance = entering;
            previous_exit = leaving;
        }
        assert!((entrance(0.015625) - 0.15625).abs() < 0.00001);
        assert!((entrance(0.125) - 0.5).abs() < 0.00001);
        assert!((exit(0.5) - 0.110118).abs() < 0.00001);
    }

    #[test]
    fn fluent_curves_match_independent_parametric_bezier_samples() {
        // Evaluate Bernstein basis at known parameters; no inverse or production
        // easing helper is used to derive the expected curve ordinates.
        for step in 0..=1000 {
            let s = step as f32 / 1000.;
            let remaining = 1. - s;
            let y = 3. * remaining * s * s + s * s * s;
            let entering_x = s * s * s;
            let exiting_x = 1. - remaining * remaining * remaining;
            assert!((entrance(entering_x) - y).abs() < 0.00001);
            // The x=1 cusp magnifies f32 quantization of the input.
            assert!((exit(exiting_x) - y).abs() < 0.00003);
        }
    }

    #[test]
    fn content_ramps_up_then_stops_requesting_frames() {
        let now = Instant::now();
        let fade = Tween::new(0.55, 1., now, CONTENT_TIME);
        assert_eq!(fade.sample(now), (0.55, true));
        assert!((0.55..1.).contains(&fade.sample(now + CONTENT_TIME / 2).0));
        assert_eq!(fade.sample(now + CONTENT_TIME), (1., false));
    }
}
