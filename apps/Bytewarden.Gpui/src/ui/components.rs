//! Reusable Fluent 2 GPUI components: buttons, toggle switches, cards, chips,
//! avatars, badges, syntax-colored passwords, canvas TOTP ring & spinner,
//! InfoBars, EmptyStates, Modals, and floating animated Toasts.

use std::f32::consts::TAU;

use gpui::{
    AnyView, App, Bounds, BoxShadow, Context, Corners, Div, ElementId, Hsla, SharedString,
    Stateful, Window, canvas, div, fill, hsla, point, prelude::*, px, size,
};

use crate::theme::{self, hex};

use super::{
    spring::clamp_opacity,
    toast::{ToastFrame, ToastKind},
    tokens::{family, font, glyph, monogram_color, monogram_letter, radius, size, space},
};

/// Crisp Segoe Fluent Icons glyph element.
pub fn glyph_icon(code: &'static str, icon_size: f32) -> Div {
    div()
        .font_family(family::ICONS)
        .text_size(px(icon_size))
        .line_height(px(icon_size))
        .flex_shrink_0()
        .child(code)
}

/// Sleek Fluent tooltip with optional keyboard shortcut pill.
pub struct TooltipView {
    label: SharedString,
    shortcut: Option<&'static str>,
}

impl Render for TooltipView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette();
        elevated_card()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(radius::CONTROL))
            .flex()
            .items_center()
            .gap(px(space::S))
            .text_size(px(font::CAPTION))
            .text_color(hex(pal.text))
            .child(self.label.clone())
            .when_some(self.shortcut, |el, keys| {
                el.child(
                    div()
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(radius::SM))
                        .bg(hex(pal.layer))
                        .border_1()
                        .border_color(hex(pal.border))
                        .text_size(px(font::MICRO))
                        .text_color(hex(pal.text_secondary))
                        .font_family(family::MONO)
                        .child(keys),
                )
            })
    }
}

pub fn make_tooltip(
    label: impl Into<SharedString>,
    shortcut: Option<&'static str>,
    cx: &mut App,
) -> AnyView {
    let label = label.into();
    cx.new(move |_| TooltipView { label, shortcut }).into()
}

/// Soft elevation shadow appropriate for popovers, modals, and toasts.
pub fn overlay_shadow() -> Vec<BoxShadow> {
    let dark = theme::is_dark();
    vec![
        BoxShadow {
            color: hsla(0., 0., 0., if dark { 0.45 } else { 0.14 }),
            offset: point(px(0.), px(8.)),
            blur_radius: px(24.),
            spread_radius: px(0.),
        },
        BoxShadow {
            color: hsla(0., 0., 0., if dark { 0.28 } else { 0.06 }),
            offset: point(px(0.), px(2.)),
            blur_radius: px(6.),
            spread_radius: px(0.),
        },
    ]
}

/// Standard Fluent 2 surface card with subtle elevation depth.
pub fn surface_card() -> Div {
    let pal = theme::palette();
    div()
        .bg(hex(pal.card))
        .border_1()
        .border_color(hex(pal.border))
        .rounded(px(radius::CARD))
        .when(!pal.dark, |el| {
            el.shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.04),
                offset: point(px(0.), px(1.)),
                blur_radius: px(3.),
                spread_radius: px(0.),
            }])
        })
}

/// Elevated floating card (modals, popovers, command palette, toasts).
pub fn elevated_card() -> Div {
    let pal = theme::palette();
    div()
        .bg(hex(pal.card_elevated))
        .border_1()
        .border_color(hex(pal.border))
        .rounded(px(radius::OVERLAY))
        .shadow(overlay_shadow())
}

/// Grouped Fluent 2 card with subtle title header.
pub fn card_group(title: &'static str, icon_glyph: Option<&'static str>) -> Div {
    let pal = theme::palette();
    surface_card().flex().flex_col().overflow_hidden().child(
        div()
            .px(px(space::L))
            .py(px(10.))
            .border_b_1()
            .border_color(hex(pal.border_subtle))
            .flex()
            .items_center()
            .gap(px(space::S))
            .when_some(icon_glyph, |el, g| {
                el.child(
                    div()
                        .text_color(hex(pal.accent_text))
                        .child(glyph_icon(g, 13.)),
                )
            })
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(hex(pal.text_secondary))
                    .child(title),
            ),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnVariant {
    Primary,
    Secondary,
    Subtle,
    Tinted,
    Danger,
}

/// Tactile Fluent 2 button with press/hover styling.
pub fn btn(id: impl Into<ElementId>, variant: BtnVariant) -> Stateful<Div> {
    let pal = theme::palette();
    let base = div()
        .id(id)
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(size::CONTROL))
        .px(px(space::M))
        .rounded(px(radius::CONTROL))
        .text_size(px(font::BODY))
        .flex_shrink_0();

    match variant {
        BtnVariant::Primary => {
            let fill: Hsla = hex(pal.accent_fill).into();
            let hover_fill = fill.opacity(0.90);
            let active_fill = fill.opacity(0.80);
            base.bg(fill)
                .text_color(hex(pal.on_accent))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .hover(move |s| s.bg(hover_fill))
                .active(move |s| s.bg(active_fill).top(px(1.)))
        }
        BtnVariant::Secondary => base
            .bg(hex(pal.card))
            .border_1()
            .border_color(hex(pal.border))
            .text_color(hex(pal.text))
            .hover(move |s| s.bg(hex(pal.hover)))
            .active(move |s| s.bg(hex(pal.selected)).top(px(1.))),
        BtnVariant::Subtle => base
            .text_color(hex(pal.text))
            .hover(move |s| s.bg(hex(pal.hover)))
            .active(move |s| s.bg(hex(pal.selected)).top(px(1.))),
        BtnVariant::Tinted => {
            let bg = pal.tint(pal.accent_fill);
            let hover =
                Hsla::from(hex(pal.accent_fill)).opacity(if pal.dark { 0.24 } else { 0.17 });
            base.bg(bg)
                .border_1()
                .border_color(pal.tint_border(pal.accent_fill))
                .text_color(hex(pal.accent_text))
                .font_weight(gpui::FontWeight::MEDIUM)
                .hover(move |s| s.bg(hover))
                .active(move |s| s.top(px(1.)))
        }
        BtnVariant::Danger => {
            let bg = pal.tint(pal.danger);
            let hover = Hsla::from(hex(pal.danger)).opacity(if pal.dark { 0.26 } else { 0.18 });
            base.bg(bg)
                .border_1()
                .border_color(pal.tint_border(pal.danger))
                .text_color(hex(pal.danger))
                .font_weight(gpui::FontWeight::MEDIUM)
                .hover(move |s| s.bg(hover))
                .active(move |s| s.top(px(1.)))
        }
    }
}

/// Compact square icon button with tooltip and shortcut hint.
pub fn icon_btn(
    id: impl Into<ElementId>,
    code: &'static str,
    tooltip: impl Into<SharedString>,
    shortcut: Option<&'static str>,
) -> Stateful<Div> {
    let pal = theme::palette();
    let tooltip = tooltip.into();
    div()
        .id(id)
        .cursor_pointer()
        .size(px(size::ICON_BTN))
        .rounded(px(radius::CONTROL))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_color(hex(pal.text_secondary))
        .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
        .active(move |s| s.bg(hex(pal.selected)).top(px(1.)))
        .child(glyph_icon(code, 14.))
        .tooltip(move |_, cx| make_tooltip(tooltip.clone(), shortcut, cx))
}

/// Capsule filter chip (used for vault type filters & segmented pills).
pub fn pill_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    count: Option<usize>,
    selected: bool,
) -> Stateful<Div> {
    let pal = theme::palette();
    let label = label.into();
    let base = div()
        .id(id)
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(28.))
        .px(px(10.))
        .rounded(px(radius::PILL))
        .text_size(px(font::CAPTION))
        .flex_shrink_0();

    let styled = if selected {
        base.bg(pal.tint(pal.accent_fill))
            .border_1()
            .border_color(pal.tint_border(pal.accent_fill))
            .text_color(hex(pal.accent_text))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .active(|s| s.top(px(1.)))
    } else {
        base.bg(hex(pal.card))
            .border_1()
            .border_color(hex(pal.border))
            .text_color(hex(pal.text_secondary))
            .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
            .active(move |s| s.bg(hex(pal.selected)).top(px(1.)))
    };

    styled.child(label).when_some(count, |el, n| {
        el.child(
            div()
                .text_size(px(font::MICRO))
                .opacity(if selected { 0.88 } else { 0.72 })
                .child(n.to_string()),
        )
    })
}

/// Linearly interpolate two `Rgba` colors in sRGB space for smooth spring color morphs.
pub fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0., 1.);
    let ra: gpui::Rgba = a.into();
    let rb: gpui::Rgba = b.into();
    gpui::Rgba {
        r: ra.r + (rb.r - ra.r) * t,
        g: ra.g + (rb.g - ra.g) * t,
        b: ra.b + (rb.b - ra.b) * t,
        a: ra.a + (rb.a - ra.a) * t,
    }
    .into()
}

/// Fluent 2 pill ToggleSwitch control with continuous spring thumb glide and color morph.
pub fn toggle_switch_animated(
    id: impl Into<ElementId>,
    checked: bool,
    progress: f32,
) -> Stateful<Div> {
    let pal = theme::palette();
    let p = progress.clamp(0., 1.);
    let off_bg: Hsla = hex(pal.input).into();
    let on_bg: Hsla = hex(pal.accent_fill).into();
    let track_bg = lerp_hsla(off_bg, on_bg, p);
    let border_col = lerp_hsla(hex(pal.border).into(), on_bg, p);
    let thumb_color = lerp_hsla(hex(pal.text_secondary).into(), hex(pal.on_accent).into(), p);
    let thumb_w = 12. + 4. * (1. - (2. * p - 1.).abs());
    let thumb_x = 3. + (20. - (thumb_w - 12.) * 0.5) * progress.clamp(-0.08, 1.08);

    div()
        .id(id)
        .cursor_pointer()
        .relative()
        .w(px(40.))
        .h(px(20.))
        .rounded(px(radius::PILL))
        .bg(track_bg)
        .border_1()
        .border_color(border_col)
        .flex_shrink_0()
        .when(!checked, |el| el.hover(move |s| s.bg(hex(pal.hover))))
        .active(|s| s.opacity(0.9))
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(thumb_x))
                .w(px(thumb_w))
                .h(px(12.))
                .rounded(px(radius::PILL))
                .bg(thumb_color),
        )
}

/// Convenience wrapper for static ToggleSwitch rendering.
#[allow(dead_code)]
pub fn toggle_switch(id: impl Into<ElementId>, checked: bool) -> Stateful<Div> {
    toggle_switch_animated(id, checked, if checked { 1. } else { 0. })
}

/// Zero-Path chevron indicator (`0.0` = right `›`, `1.0` = down `⌄`).
/// Uses cached DirectWrite glyph sprites instead of `PathBuilder` so GPUI never
/// allocates full-window 4x MSAA `PathResources` textures (~80-160 MB on High-DPI).
pub fn rotating_chevron(progress: f32, icon_size: f32, color: Hsla) -> Div {
    let p = progress.clamp(0., 1.);
    let base = div()
        .relative()
        .size(px(icon_size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color);
    if p <= 0.02 {
        base.child(glyph_icon(glyph::CHEVRON_RIGHT, icon_size * 0.85))
    } else if p >= 0.98 {
        base.child(glyph_icon(glyph::CHEVRON_DOWN, icon_size * 0.85))
    } else {
        base.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(1. - p)
                .child(glyph_icon(glyph::CHEVRON_RIGHT, icon_size * 0.85)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(p)
                .child(glyph_icon(glyph::CHEVRON_DOWN, icon_size * 0.85)),
        )
    }
}

fn paint_orbital_dots(
    bounds: Bounds<gpui::Pixels>,
    angle_rad: f32,
    color: Hsla,
    window: &mut Window,
) {
    const DOTS: usize = 8;
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let cx = f32::from(bounds.origin.x) + w * 0.5;
    let cy = f32::from(bounds.origin.y) + h * 0.5;
    let min_dim = w.min(h);
    let dot_d = (min_dim * 0.19).clamp(2.0, 3.6);
    let dot_r = dot_d * 0.5;
    let orbit_r = (min_dim * 0.5 - dot_r - 0.5).max(2.0);
    for i in 0..DOTS {
        let theta = (i as f32 / DOTS as f32) * TAU - std::f32::consts::FRAC_PI_2;
        let mut delta = (angle_rad - theta).rem_euclid(TAU);
        if delta > TAU {
            delta -= TAU;
        }
        let trail = (1.0 - delta / TAU).powi(2);
        let alpha = (0.16 + 0.84 * trail).clamp(0.16, 1.0);
        let mut quad = fill(
            Bounds::new(
                point(
                    px(cx + orbit_r * theta.cos() - dot_r),
                    px(cy + orbit_r * theta.sin() - dot_r),
                ),
                size(px(dot_d), px(dot_d)),
            ),
            color.opacity(alpha),
        );
        quad.corner_radii = Corners::all(px(dot_r));
        window.paint_quad(quad);
    }
}

/// Zero-Path refresh icon: renders a crisp glyph at rest and an instanced 8-dot
/// orbital `PaintQuad` ring while spinning (`0` MSAA path textures).
pub fn rotating_refresh_icon(angle_rad: f32, icon_size: f32, color: Hsla) -> Div {
    let rem = angle_rad.rem_euclid(std::f32::consts::PI);
    let settled = rem < 0.03 || (std::f32::consts::PI - rem) < 0.03;
    let base = div()
        .size(px(icon_size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color);
    if settled {
        base.child(glyph_icon(glyph::SYNC, icon_size))
    } else {
        base.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    paint_orbital_dots(bounds, angle_rad, color, window);
                },
            )
            .size(px(icon_size)),
        )
    }
}

/// Soft animated skeleton placeholder aligned with Detail Hero + FieldGroup card geometry.
pub fn detail_skeleton(pulse_phase: f32) -> Div {
    let pal = theme::palette();
    let alpha = 0.42 + 0.22 * (pulse_phase * std::f32::consts::TAU).sin().abs();
    let bar = |w: f32, h: f32| {
        div()
            .w(px(w))
            .h(px(h))
            .rounded(px(radius::SM))
            .bg(Hsla::from(hex(pal.border)).opacity(alpha))
    };
    div()
        .w_full()
        .max_w(px(size::DETAIL_MAX))
        .mx_auto()
        .p(px(space::XL))
        .flex()
        .flex_col()
        .gap(px(space::L))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .pb(px(space::M))
                .border_b_1()
                .border_color(hex(pal.border_subtle))
                .child(
                    div()
                        .size(px(size::HERO_ICON))
                        .rounded(px(12.))
                        .bg(Hsla::from(hex(pal.border)).opacity(alpha)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(bar(180., 18.))
                        .child(bar(110., 12.)),
                ),
        )
        .child(
            surface_card()
                .p(px(space::L))
                .flex()
                .flex_col()
                .gap(px(space::L))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(bar(72., 10.))
                        .child(bar(240., 14.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(bar(64., 10.))
                        .child(bar(200., 14.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(bar(84., 10.))
                        .child(bar(280., 14.)),
                ),
        )
}

/// Neutral segmented control container with a spring-gliding elevated pill behind the active segment.
pub fn segmented_track(count: usize, position: f32) -> Div {
    let pal = theme::palette();
    let count = count.max(1);
    let frac = 1. / count as f32;
    let clamped = position.clamp(0., (count - 1) as f32);
    div()
        .relative()
        .p(px(3.))
        .rounded(px(radius::CONTROL + 2.))
        .bg(hex(pal.layer))
        .border_1()
        .border_color(hex(pal.border_subtle))
        .flex()
        .items_center()
        .child(
            div()
                .absolute()
                .top(px(3.))
                .bottom(px(3.))
                .left(gpui::relative(clamped * frac))
                .w(gpui::relative(frac))
                .px(px(3.))
                .child(
                    div()
                        .size_full()
                        .rounded(px(radius::CONTROL))
                        .bg(hex(pal.card_elevated))
                        .border_1()
                        .border_color(hex(pal.border))
                        .shadow(vec![BoxShadow {
                            color: hsla(0., 0., 0., if pal.dark { 0.24 } else { 0.06 }),
                            offset: point(px(0.), px(1.)),
                            blur_radius: px(3.),
                            spread_radius: px(0.),
                        }]),
                ),
        )
}

/// Deterministic circular user avatar with optional corner server badge.
pub fn avatar(label: &str, corner_badge: Option<&'static str>, px_size: f32) -> Div {
    let pal = theme::palette();
    let bg_hex = monogram_color(label);
    let letter = monogram_letter(label);
    div()
        .relative()
        .size(px(px_size))
        .rounded(px(radius::PILL))
        .bg(hex(bg_hex))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_color(hex(0xffffff))
        .text_size(px((px_size * 0.44).max(11.)))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(letter)
        .when_some(corner_badge, |el, tag| {
            el.child(
                div()
                    .absolute()
                    .bottom(px(-2.))
                    .right(px(-3.))
                    .px(px(3.))
                    .rounded(px(4.))
                    .bg(hex(pal.card_elevated))
                    .border_1()
                    .border_color(hex(pal.border))
                    .text_size(px(8.5))
                    .line_height(px(11.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(hex(pal.text_secondary))
                    .child(tag),
            )
        })
}

/// Vibrant rounded icon tile for vault items without a website favicon.
pub fn item_monogram(
    name: &str,
    item_type: u8,
    fallback_glyph: &'static str,
    tile_size: f32,
    corner: f32,
) -> Div {
    let pal = theme::palette();
    let tint_hex = monogram_color(name);
    let base: Hsla = hex(tint_hex).into();
    let bg = base.opacity(if pal.dark { 0.24 } else { 0.14 });
    let border = base.opacity(if pal.dark { 0.40 } else { 0.24 });
    let fg = if pal.dark {
        base.blend(gpui::hsla(0., 0., 1., 0.55))
    } else {
        base
    };
    let use_letter = item_type == 1 && !name.trim().is_empty();
    div()
        .size(px(tile_size))
        .rounded(px(corner))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_color(fg)
        .when(use_letter, |el| {
            el.text_size(px((tile_size * 0.46).max(13.)))
                .font_weight(gpui::FontWeight::BOLD)
                .child(monogram_letter(name))
        })
        .when(!use_letter, |el| {
            el.child(glyph_icon(fallback_glyph, (tile_size * 0.48).max(14.)))
        })
}

/// Compact semantic pill badge.
pub fn badge(label: impl Into<SharedString>, color_hex: u32) -> Div {
    let pal = theme::palette();
    div()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(radius::PILL))
        .bg(pal.tint(color_hex))
        .border_1()
        .border_color(pal.tint_border(color_hex))
        .text_size(px(font::MICRO))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(hex(color_hex))
        .flex_shrink_0()
        .child(label.into())
}

/// Semantic InfoBar for persistent warnings, errors, or active operations.
pub fn info_bar(kind: ToastKind, message: impl Into<SharedString>) -> Div {
    let pal = theme::palette();
    let (color, code) = match kind {
        ToastKind::Success => (pal.success, glyph::CHECK),
        ToastKind::Info => (pal.info, glyph::INFO),
        ToastKind::Warning => (pal.warning, glyph::WARNING),
        ToastKind::Error => (pal.danger, glyph::ERROR),
    };
    div()
        .px(px(space::M))
        .py(px(9.))
        .rounded(px(radius::CARD))
        .bg(pal.tint(color))
        .border_1()
        .border_color(pal.tint_border(color))
        .flex()
        .items_center()
        .gap(px(space::S))
        .child(div().text_color(hex(color)).child(glyph_icon(code, 15.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(font::CAPTION))
                .text_color(hex(pal.text))
                .child(message.into()),
        )
}

/// Polished centered empty state with icon halo.
pub fn empty_state(
    icon_glyph: &'static str,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
) -> Div {
    let pal = theme::palette();
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(space::M))
        .p(px(space::XL))
        .child(
            div()
                .size(px(64.))
                .rounded(px(radius::PILL))
                .bg(pal.tint(pal.accent_fill))
                .border_1()
                .border_color(pal.tint_border(pal.accent_fill))
                .flex()
                .items_center()
                .justify_center()
                .text_color(hex(pal.accent_text))
                .child(glyph_icon(icon_glyph, 26.)),
        )
        .child(
            div()
                .text_size(px(font::BODY_LG))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(hex(pal.text))
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(300.))
                .text_center()
                .text_size(px(font::CAPTION))
                .text_color(hex(pal.text_secondary))
                .child(subtitle.into()),
        )
}

/// Render a password in Cascadia Mono with syntax coloring:
/// letters = primary text, digits = accent blue, symbols = warm amber.
pub fn password_colored(
    registry: &crate::sensitive_text::Registry,
    password: &str,
    font_size: f32,
) -> Div {
    div()
        .font_family(family::MONO)
        .text_size(px(font_size))
        .child(registry.text(password, true))
}

/// 5-segment password strength meter bar with semantic color & label.
pub fn strength_meter(score: u8) -> Div {
    let pal = theme::palette();
    let clamped = score.min(4);
    let active_segments = usize::from(clamped) + 1;
    let (label, color) = match clamped {
        0 => ("非常弱", pal.danger),
        1 => ("弱", pal.danger),
        2 => ("一般", pal.warning),
        3 => ("强", pal.info),
        _ => ("非常强", pal.success),
    };
    div()
        .flex()
        .items_center()
        .gap(px(space::S))
        .child(
            div()
                .flex_1()
                .flex()
                .gap(px(4.))
                .children((0..5).map(|index| {
                    div().flex_1().h(px(5.)).rounded(px(radius::PILL)).bg(
                        if index < active_segments {
                            Hsla::from(hex(color))
                        } else {
                            Hsla::from(hex(pal.border))
                        },
                    )
                })),
        )
        .child(
            div()
                .text_size(px(font::CAPTION))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(hex(color))
                .child(label),
        )
}

/// Format a 6-digit or 8-digit TOTP code into readable chunks (`482 913`).
pub fn format_totp(code: &str) -> String {
    let trimmed = code.trim();
    if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        format!("{} {}", &trimmed[..3], &trimmed[3..])
    } else if trimmed.len() == 8 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        format!("{} {}", &trimmed[..4], &trimmed[4..])
    } else {
        trimmed.to_owned()
    }
}

/// Zero-Path hardware-instanced `PaintQuad` circular TOTP countdown ring (12 radial dots).
/// Avoids `PathBuilder` so viewing a TOTP item never allocates 4x MSAA full-window textures.
pub fn totp_ring(fraction: f32, urgent: bool, ring_size: f32) -> impl IntoElement {
    let pal = theme::palette();
    let track_color: Hsla = Hsla::from(hex(pal.border)).opacity(0.55);
    let active_color: Hsla = hex(if urgent { pal.danger } else { pal.accent_fill }).into();
    let frac = fraction.clamp(0., 1.);

    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            const TICKS: usize = 12;
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let cx = f32::from(bounds.origin.x) + w * 0.5;
            let cy = f32::from(bounds.origin.y) + h * 0.5;
            let dot_d = (w.min(h) * 0.14).clamp(2.4, 4.0);
            let dot_r = dot_d * 0.5;
            let r = (w.min(h) * 0.5 - dot_r - 1.0).max(3.);
            let lit = frac * TICKS as f32;

            for i in 0..TICKS {
                let theta = -std::f32::consts::FRAC_PI_2 + (i as f32 / TICKS as f32) * TAU;
                let strength = (lit - i as f32).clamp(0., 1.);
                let col = if strength <= 0.02 {
                    track_color
                } else if strength >= 0.98 {
                    active_color
                } else {
                    lerp_hsla(track_color, active_color, strength)
                };
                let mut quad = fill(
                    Bounds::new(
                        point(
                            px(cx + r * theta.cos() - dot_r),
                            px(cy + r * theta.sin() - dot_r),
                        ),
                        size(px(dot_d), px(dot_d)),
                    ),
                    col,
                );
                quad.corner_radii = Corners::all(px(dot_r));
                window.paint_quad(quad);
            }
        },
    )
    .size(px(ring_size))
    .flex_shrink_0()
}

/// Zero-Path hardware-instanced `PaintQuad` orbital spinner for active sync/network jobs.
pub fn sync_spinner(angle_rad: f32, spinner_size: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            paint_orbital_dots(bounds, angle_rad, color, window);
        },
    )
    .size(px(spinner_size))
    .flex_shrink_0()
}

/// Floating bottom-center animated Toast pill with live countdown bar.
pub fn toast_pill(
    frame: &ToastFrame<'_>,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let pal = theme::palette();
    let (color, code) = match frame.toast.kind {
        ToastKind::Success => (pal.success, glyph::CHECK),
        ToastKind::Info => (pal.info, glyph::INFO),
        ToastKind::Warning => (pal.warning, glyph::WARNING),
        ToastKind::Error => (pal.danger, glyph::ERROR),
    };
    let y_offset = 26. * (1. - frame.progress);
    let opacity = clamp_opacity(frame.progress);
    let bar_pct = (frame.bar_fraction * 100.).clamp(0., 100.);

    let display_text = if let Some(secs) = frame.remaining_secs {
        // Show crisp live ticking countdown on clipboard toasts.
        let prefix = frame
            .toast
            .message
            .split('·')
            .next()
            .unwrap_or(&frame.toast.message)
            .trim();
        format!("{prefix} · {secs}s 后清除剪贴板")
    } else {
        frame.toast.message.clone()
    };

    div()
        .absolute()
        .bottom(px(20. - y_offset))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .opacity(opacity)
        .child(
            elevated_card()
                .occlude()
                .overflow_hidden()
                .min_w(px(240.))
                .max_w(px(480.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(14.))
                        .py(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .size(px(22.))
                                .rounded(px(radius::PILL))
                                .bg(pal.tint(color))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(hex(color))
                                .child(glyph_icon(code, 12.)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(font::CAPTION))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(hex(pal.text))
                                .child(display_text),
                        )
                        .child(
                            div()
                                .id("dismiss-toast")
                                .cursor_pointer()
                                .size(px(22.))
                                .rounded(px(radius::SM))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(hex(pal.text_muted))
                                .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                                .child(glyph_icon(glyph::CLOSE, 10.))
                                .on_click(move |_, window, cx| on_dismiss(window, cx)),
                        ),
                )
                .child(
                    div().h(px(2.5)).w_full().bg(hex(pal.border_subtle)).child(
                        div()
                            .h_full()
                            .w(gpui::relative(bar_pct / 100.))
                            .bg(hex(color)),
                    ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_totp_groups_six_and_eight_digit_codes() {
        assert_eq!(format_totp("482913"), "482 913");
        assert_eq!(format_totp("12345678"), "1234 5678");
        assert_eq!(format_totp("ABC"), "ABC");
    }
}
