/// SSHerald CRT hacker theme -- green phosphor on black.
///
/// All colors and visuals are defined here for consistency.

// ── Color palette ──

pub const BG:           egui::Color32 = egui::Color32::from_rgb(0x08, 0x08, 0x08);
pub const BG_PANEL:     egui::Color32 = egui::Color32::from_rgb(0x0c, 0x0c, 0x0c);
pub const BG_WIDGET:    egui::Color32 = egui::Color32::from_rgb(0x12, 0x12, 0x12);
pub const BG_HOVER:     egui::Color32 = egui::Color32::from_rgb(0x1a, 0x2a, 0x1a);
pub const BG_ACTIVE:    egui::Color32 = egui::Color32::from_rgb(0x0a, 0x30, 0x0a);
pub const BG_SELECTION: egui::Color32 = egui::Color32::from_rgb(0x14, 0x3a, 0x14);

pub const GREEN:        egui::Color32 = egui::Color32::from_rgb(0x00, 0xff, 0x41);
pub const GREEN_DIM:    egui::Color32 = egui::Color32::from_rgb(0x00, 0x99, 0x28);
pub const GREEN_DARK:   egui::Color32 = egui::Color32::from_rgb(0x00, 0x55, 0x18);
pub const GREEN_BRIGHT: egui::Color32 = egui::Color32::from_rgb(0x39, 0xff, 0x14);
pub const AMBER:        egui::Color32 = egui::Color32::from_rgb(0xff, 0xb0, 0x00);
pub const RED:          egui::Color32 = egui::Color32::from_rgb(0xff, 0x33, 0x33);
pub const CYAN:         egui::Color32 = egui::Color32::from_rgb(0x00, 0xdd, 0xcc);
pub const GREY:         egui::Color32 = egui::Color32::from_rgb(0x44, 0x55, 0x44);

pub fn apply(ctx: &egui::Context) {
    // Force everything to monospace
    let mut style = (*ctx.style()).clone();
    style.override_font_id = Some(egui::FontId::monospace(13.0));
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    ctx.set_style(style);

    let mut visuals = egui::Visuals::dark();

    // Window / panel backgrounds
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = BG_WIDGET;

    // Borders
    visuals.window_stroke = egui::Stroke::new(1.0, GREEN_DARK);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, GREEN_DARK);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, GREEN_DIM);

    // Selection
    visuals.selection.bg_fill = BG_SELECTION;
    visuals.selection.stroke = egui::Stroke::new(1.0, GREEN);

    // Text
    visuals.override_text_color = Some(GREEN);

    // Hyperlinks
    visuals.hyperlink_color = CYAN;

    // Widgets — inactive
    visuals.widgets.inactive.bg_fill = BG_WIDGET;
    visuals.widgets.inactive.weak_bg_fill = BG_WIDGET;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, GREEN_DARK);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, GREEN_DIM);
    visuals.widgets.inactive.rounding = egui::Rounding::same(2.0);

    // Widgets — hovered
    visuals.widgets.hovered.bg_fill = BG_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, GREEN);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, GREEN_BRIGHT);
    visuals.widgets.hovered.rounding = egui::Rounding::same(2.0);

    // Widgets — active (clicked)
    visuals.widgets.active.bg_fill = BG_ACTIVE;
    visuals.widgets.active.weak_bg_fill = BG_ACTIVE;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, GREEN_BRIGHT);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, GREEN_BRIGHT);
    visuals.widgets.active.rounding = egui::Rounding::same(2.0);

    // Widgets — open (combobox, menu)
    visuals.widgets.open.bg_fill = BG_ACTIVE;
    visuals.widgets.open.weak_bg_fill = BG_ACTIVE;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, GREEN);
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, GREEN);

    // Separators
    visuals.widgets.noninteractive.bg_fill = BG;

    // Window shadow
    visuals.window_shadow = egui::Shadow {
        offset: egui::Vec2::new(0.0, 0.0),
        blur: 8.0,
        spread: 0.0,
        color: egui::Color32::from_rgba_premultiplied(0, 40, 0, 60),
    };
    visuals.popup_shadow = visuals.window_shadow;

    // Window rounding
    visuals.window_rounding = egui::Rounding::same(1.0);
    visuals.menu_rounding = egui::Rounding::same(1.0);

    ctx.set_visuals(visuals);
}
