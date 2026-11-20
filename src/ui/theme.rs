//! Minimalist Swiss / Big-Tech monochrome palette and animated hover components.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Margin, Sense, Stroke, Vec2, Visuals,
};

use crate::win32::HideStrategy;

pub const BG_CANVAS: Color32 = Color32::from_rgb(10, 10, 11); // #0A0A0B
pub const BG_SURFACE: Color32 = Color32::from_rgb(18, 18, 21); // #121215
pub const BG_SURFACE_HOVER: Color32 = Color32::from_rgb(24, 24, 29); // #18181D
pub const BG_TRACK: Color32 = Color32::from_rgb(14, 14, 17); // #0E0E11
pub const BG_PILL_IDLE: Color32 = Color32::from_rgb(24, 24, 29); // #18181D
pub const BG_PILL_HOVER: Color32 = Color32::from_rgb(37, 37, 45); // #25252D
pub const BG_ACTIVE_WHITE: Color32 = Color32::from_rgb(242, 242, 244); // #F2F2F4

pub const BORDER_HAIRLINE: Color32 = Color32::from_rgb(33, 33, 39); // #212127
pub const BORDER_HOVER: Color32 = Color32::from_rgb(62, 62, 74); // #3E3E4A

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(242, 242, 244); // #F2F2F4
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(134, 134, 146); // #868692
pub const TEXT_MUTED: Color32 = Color32::from_rgb(86, 86, 98); // #565662
pub const TEXT_DARK: Color32 = Color32::from_rgb(10, 10, 11); // #0A0A0B

pub const DOT_LIVE: Color32 = Color32::from_rgb(52, 211, 153); // #34D399
pub const DOT_GHOST: Color32 = Color32::from_rgb(251, 191, 36); // #FBBF24
pub const DOT_MIN: Color32 = Color32::from_rgb(110, 125, 148); // #6E7D94
pub const BADGE_WARN: Color32 = Color32::from_rgb(248, 113, 113); // #F87171

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillTone {
    Primary,
    AmberGhost,
    SlateMin,
    Danger,
    Neutral,
}

#[inline]
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgba_premultiplied(
        (a.r() as f32 * inv + b.r() as f32 * t).round() as u8,
        (a.g() as f32 * inv + b.g() as f32 * t).round() as u8,
        (a.b() as f32 * inv + b.b() as f32 * t).round() as u8,
        (a.a() as f32 * inv + b.a() as f32 * t).round() as u8,
    )
}

pub fn apply_minimal_theme(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = BG_CANVAS;
    visuals.window_fill = Color32::from_rgb(20, 20, 24);
    visuals.extreme_bg_color = BG_TRACK;
    visuals.faint_bg_color = BG_SURFACE;
    visuals.window_stroke = Stroke::new(1.0, BORDER_HAIRLINE);
    visuals.window_corner_radius = CornerRadius::same(8);

    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_HAIRLINE);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);

    visuals.widgets.inactive.bg_fill = BG_PILL_IDLE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_HAIRLINE);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(5);

    visuals.widgets.hovered.bg_fill = BG_PILL_HOVER;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_HOVER);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(5);

    visuals.widgets.active.bg_fill = BG_PILL_HOVER;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.active.corner_radius = CornerRadius::same(5);

    visuals.selection.bg_fill = Color32::from_rgb(48, 48, 58);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(76, 76, 92));

    ctx.set_theme(egui::Theme::Dark);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = visuals;
    style.animation_time = 0.12;
    style.spacing.item_spacing = Vec2::new(6.0, 6.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.window_margin = Margin::same(12);
    style.spacing.scroll.bar_width = 5.0;
    style.spacing.scroll.handle_min_length = 24.0;
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Crisp custom-painted pill with smooth animated hover & active states.
pub fn animated_pill(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    active: bool,
    tone: PillTone,
    min_size: Vec2,
) -> egui::Response {
    let font_id = FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_id, TEXT_PRIMARY);
    let desired = Vec2::new(
        (galley.size().x + 16.0).max(min_size.x),
        min_size.y.max(21.0),
    );
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let hover_t = ui.ctx().animate_bool_with_time(
            ui.make_persistent_id((&id_salt, "hov")),
            resp.hovered(),
            0.11,
        );
        let active_t = ui.ctx().animate_bool_with_time(
            ui.make_persistent_id((&id_salt, "act")),
            active,
            0.14,
        );
        let press_dim = if resp.is_pointer_button_down_on() {
            0.85
        } else {
            1.0
        };

        let (act_fill, act_border, act_text) = match tone {
            PillTone::Primary => (BG_ACTIVE_WHITE, BG_ACTIVE_WHITE, TEXT_DARK),
            PillTone::AmberGhost => (
                Color32::from_rgb(42, 33, 18),
                Color32::from_rgb(168, 128, 36),
                Color32::from_rgb(252, 211, 77),
            ),
            PillTone::SlateMin => (
                Color32::from_rgb(30, 36, 46),
                Color32::from_rgb(88, 102, 124),
                Color32::from_rgb(203, 213, 225),
            ),
            PillTone::Danger => (
                Color32::from_rgb(38, 22, 24),
                Color32::from_rgb(160, 68, 72),
                BADGE_WARN,
            ),
            PillTone::Neutral => (
                Color32::from_rgb(38, 38, 46),
                Color32::from_rgb(76, 76, 92),
                TEXT_PRIMARY,
            ),
        };

        let idle_fill = lerp_color(BG_PILL_IDLE, BG_PILL_HOVER, hover_t);
        let idle_border = lerp_color(BORDER_HAIRLINE, BORDER_HOVER, hover_t);
        let idle_text = lerp_color(TEXT_SECONDARY, TEXT_PRIMARY, hover_t);

        let mut fill = lerp_color(idle_fill, act_fill, active_t);
        let border = lerp_color(
            idle_border,
            lerp_color(act_border, TEXT_PRIMARY, hover_t * 0.25),
            active_t,
        );
        let text_col = lerp_color(idle_text, act_text, active_t);

        if press_dim < 1.0 {
            fill = lerp_color(fill, BG_CANVAS, 0.15);
        }

        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(4), fill);
        painter.rect_stroke(
            rect.shrink(0.5),
            CornerRadius::same(4),
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(11.0),
            text_col,
        );
    }

    resp
}

/// Subtle radial halo status dot.
pub fn draw_status_dot(ui: &mut egui::Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    let center = rect.center();
    let painter = ui.painter();
    painter.circle_filled(center, radius * 2.1, color.gamma_multiply(0.16));
    painter.circle_filled(center, radius, color);
}

/// Sleek segmented control for switching HideStrategy with smooth hover transitions.
pub fn segmented_strategy(
    ui: &mut egui::Ui,
    current: &mut HideStrategy,
    has_mon2: bool,
) {
    egui::Frame::NONE
        .fill(BG_TRACK)
        .stroke(Stroke::new(1.0, BORDER_HAIRLINE))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let items = [
                    (
                        HideStrategy::StealthBackground,
                        "Stealth",
                        true,
                        "Hide from Alt+Tab & Taskbar, keep live behind windows",
                    ),
                    (
                        HideStrategy::CornerPeek,
                        "1px",
                        true,
                        "Hide from Alt+Tab & tuck 1px at monitor corner",
                    ),
                    (
                        HideStrategy::SecondaryMonitor,
                        "Mon2",
                        has_mon2,
                        if has_mon2 {
                            "Move un-occluded to Secondary/Virtual (IddCx) Monitor"
                        } else {
                            "Connect a 2nd or Virtual (IddCx) monitor to enable"
                        },
                    ),
                ];

                for (strat, text, enabled, tip) in items {
                    let selected = *current == strat;
                    let size = Vec2::new(46.0, 20.0);
                    let sense = if enabled { Sense::click() } else { Sense::hover() };
                    let (rect, resp) = ui.allocate_exact_size(size, sense);
                    let resp = if enabled {
                        resp.on_hover_cursor(CursorIcon::PointingHand)
                    } else {
                        resp
                    };

                    if ui.is_rect_visible(rect) {
                        let hov_t = ui.ctx().animate_bool_with_time(
                            ui.make_persistent_id(("seg_hov", text)),
                            enabled && resp.hovered(),
                            0.11,
                        );
                        let sel_t = ui.ctx().animate_bool_with_time(
                            ui.make_persistent_id(("seg_sel", text)),
                            selected,
                            0.13,
                        );

                        let fill = lerp_color(
                            lerp_color(Color32::TRANSPARENT, BG_PILL_IDLE, hov_t),
                            Color32::from_rgb(36, 36, 44),
                            sel_t,
                        );
                        let border = lerp_color(
                            Color32::TRANSPARENT,
                            Color32::from_rgb(64, 64, 78),
                            sel_t,
                        );
                        let fg = if !enabled {
                            TEXT_MUTED
                        } else {
                            lerp_color(
                                lerp_color(TEXT_SECONDARY, TEXT_PRIMARY, hov_t),
                                TEXT_PRIMARY,
                                sel_t,
                            )
                        };

                        let p = ui.painter();
                        if fill.a() > 0 {
                            p.rect_filled(rect, CornerRadius::same(3), fill);
                        }
                        if border.a() > 0 {
                            p.rect_stroke(
                                rect.shrink(0.5),
                                CornerRadius::same(3),
                                Stroke::new(1.0, border),
                                egui::StrokeKind::Inside,
                            );
                        }
                        p.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            text,
                            FontId::proportional(10.5),
                            fg,
                        );
                    }

                    if resp.on_hover_text(tip).clicked() && enabled {
                        *current = strat;
                    }
                }
            });
        });
}
