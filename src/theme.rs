//! Look and feel: colours for light and dark mode, the Inter font, widget styling and a few
//! hand-drawn widgets (avatar, round send button, typing dots) that do not depend on icon glyphs.

use eframe::egui::{
    self, epaint::Shadow, Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Response, Rounding, Sense, Stroke,
    TextStyle, Theme, Ui, Vec2,
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hi: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub code_bg: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: rgb(15, 17, 23),
            panel: rgb(21, 24, 33),
            surface: rgb(26, 30, 41),
            surface_hi: rgb(35, 40, 54),
            border: rgb(44, 50, 66),
            text: rgb(230, 233, 241),
            weak: rgb(139, 147, 167),
            accent: rgb(124, 92, 255),
            accent_hover: rgb(143, 116, 255),
            accent_soft: Color32::from_rgba_unmultiplied(124, 92, 255, 40),
            on_accent: Color32::WHITE,
            success: rgb(52, 199, 123),
            warning: rgb(242, 184, 75),
            danger: rgb(239, 90, 90),
            code_bg: rgb(12, 14, 19),
        }
    } else {
        Palette {
            bg: rgb(246, 247, 251),
            panel: rgb(255, 255, 255),
            surface: rgb(255, 255, 255),
            surface_hi: rgb(238, 240, 246),
            border: rgb(225, 229, 238),
            text: rgb(27, 31, 42),
            weak: rgb(107, 115, 133),
            accent: rgb(98, 71, 234),
            accent_hover: rgb(116, 92, 242),
            accent_soft: Color32::from_rgba_unmultiplied(98, 71, 234, 28),
            on_accent: Color32::WHITE,
            success: rgb(30, 160, 95),
            warning: rgb(200, 140, 20),
            danger: rgb(214, 64, 64),
            code_bg: rgb(244, 245, 249),
        }
    }
}

pub fn of(ui: &Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

/// Apply the Appearance setting ("system", "light" or "dark").
pub fn set_appearance(ctx: &egui::Context, appearance: &str) {
    let pref = match appearance {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    };
    ctx.options_mut(|o| o.theme_preference = pref);
}

/// The semibold face, for names, headings and **bold** text.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf")));
    fonts.font_data.insert("inter-semibold".into(), FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")));
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    let mut bold_family = vec!["inter-semibold".to_string()];
    bold_family.extend(fallback);
    fonts.families.insert(bold(), bold_family);
    ctx.set_fonts(fonts);

    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |s| style(s, theme == Theme::Dark));
    }
}

fn style(s: &mut egui::Style, dark: bool) {
    let p = palette(dark);

    s.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(15.0)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Heading, FontId::new(20.0, bold())),
        (TextStyle::Monospace, FontId::monospace(13.5)),
    ]
    .into();

    s.spacing.item_spacing = egui::vec2(8.0, 8.0);
    s.spacing.button_padding = egui::vec2(12.0, 7.0);
    s.spacing.interact_size.y = 30.0;
    s.spacing.window_margin = egui::Margin::same(20.0);
    s.spacing.scroll = egui::style::ScrollStyle::floating();

    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.surface_hi;
    v.code_bg_color = p.surface_hi;
    v.hyperlink_color = p.accent_hover;
    v.window_rounding = Rounding::same(16.0);
    v.menu_rounding = Rounding::same(10.0);
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_shadow = Shadow { offset: egui::vec2(0.0, 10.0), blur: 32.0, spread: 0.0, color: Color32::from_black_alpha(if dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: egui::vec2(0.0, 6.0), blur: 16.0, spread: 0.0, color: Color32::from_black_alpha(if dark { 90 } else { 30 }) };
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.text_cursor.stroke = Stroke::new(2.0_f32, p.accent);

    let r = Rounding::same(10.0);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.noninteractive.rounding = r;

    w.inactive.bg_fill = p.surface_hi;
    w.inactive.weak_bg_fill = p.surface_hi;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.rounding = r;

    let hover = if dark { rgb(45, 51, 68) } else { rgb(228, 231, 240) };
    w.hovered.bg_fill = hover;
    w.hovered.weak_bg_fill = hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.hovered.rounding = r;
    w.hovered.expansion = 0.0;

    let active = if dark { rgb(54, 61, 80) } else { rgb(218, 222, 233) };
    w.active.bg_fill = active;
    w.active.weak_bg_fill = active;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.active.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.active.rounding = r;
    w.active.expansion = 0.0;

    w.open = w.active;
    s.visuals = v;
}

// ------------------------------------------------------------------ small widgets

// ------------------------------------------------------------------ hover-aware buttons

/// Mix two colours: `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// 0 → 1 as the pointer rests on `id` (short fade in and out).
pub fn hover_anim(ui: &Ui, id: egui::Id, hovered: bool) -> f32 {
    ui.ctx().animate_bool_with_time(id.with("hover"), hovered, 0.12)
}

/// Hover amount for a container drawn before its response is known: uses last frame's state.
pub fn remembered_hover(ui: &Ui, id: egui::Id) -> f32 {
    let was: bool = ui.data(|d| d.get_temp(id.with("was-hovered")).unwrap_or(false));
    hover_anim(ui, id, was)
}

pub fn remember_hover(ui: &Ui, id: egui::Id, hovered: bool) {
    ui.data_mut(|d| d.insert_temp(id.with("was-hovered"), hovered));
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Filled accent colour: the main action.
    Primary,
    /// Soft grey pill/box with a border that lights up.
    Secondary,
    /// No background until hovered (icons, Copy, ✖).
    Ghost,
    /// Large outlined card (suggestions on the start screen); text is left-aligned.
    Card,
}

pub struct Btn {
    text: egui::RichText,
    kind: Kind,
    min_size: Vec2,
    rounding: f32,
    padding: Vec2,
    enabled: bool,
}

impl Btn {
    pub fn new(kind: Kind, text: impl Into<egui::RichText>) -> Self {
        let (min_size, rounding, padding) = match kind {
            Kind::Primary | Kind::Secondary => (egui::vec2(0.0, 34.0), 10.0, egui::vec2(14.0, 6.0)),
            Kind::Ghost => (egui::vec2(28.0, 28.0), 8.0, egui::vec2(8.0, 4.0)),
            Kind::Card => (egui::vec2(0.0, 46.0), 14.0, egui::vec2(14.0, 8.0)),
        };
        Self { text: text.into(), kind, min_size, rounding, padding, enabled: true }
    }
    pub fn min_size(mut self, s: Vec2) -> Self {
        self.min_size = s;
        self
    }
    pub fn rounding(mut self, r: f32) -> Self {
        self.rounding = r;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
}

impl egui::Widget for Btn {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = of(ui);
        let dark = ui.visuals().dark_mode;
        let wrap_width = ui.available_width() - 2.0 * self.padding.x;
        let galley = egui::WidgetText::from(self.text.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), wrap_width, TextStyle::Button);
        let size = (galley.size() + 2.0 * self.padding).max(self.min_size);
        let enabled = self.enabled && ui.is_enabled();
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let (rect, resp) = ui.allocate_exact_size(size, sense);
        let hovered = enabled && resp.hovered();
        let pressed = enabled && resp.is_pointer_button_down_on();
        let t = hover_anim(ui, resp.id, hovered);

        let hover_bg = if dark { rgb(45, 51, 68) } else { rgb(228, 231, 240) };
        let (fill, stroke, text_color) = match self.kind {
            Kind::Primary => (
                mix(p.accent, p.accent_hover, t),
                Stroke::new(1.0_f32, mix(Color32::TRANSPARENT, Color32::from_white_alpha(60), t)),
                p.on_accent,
            ),
            Kind::Secondary => (mix(p.surface_hi, hover_bg, t), Stroke::new(1.0_f32, mix(p.border, p.accent, t * 0.8)), p.text),
            Kind::Ghost => (mix(Color32::TRANSPARENT, hover_bg, t), Stroke::NONE, mix(p.weak, p.text, t)),
            Kind::Card => (mix(p.surface, mix(p.surface, p.accent, 0.08), t), Stroke::new(1.0_f32, mix(p.border, p.accent, t)), p.text),
        };
        let (fill, text_color) = if !enabled {
            (fill.gamma_multiply(0.6), p.weak)
        } else if pressed {
            (mix(fill, Color32::BLACK, 0.12), text_color)
        } else {
            (fill, text_color)
        };

        if ui.is_rect_visible(rect) {
            // cards rise a little when hovered
            let rect = if self.kind == Kind::Card { rect.translate(egui::vec2(0.0, -1.5 * t)) } else { rect };
            if self.kind == Kind::Card && t > 0.0 {
                let shadow = Shadow { offset: egui::vec2(0.0, 4.0), blur: 12.0, spread: 0.0, color: Color32::from_black_alpha((if dark { 70.0 } else { 22.0 } * t) as u8) };
                ui.painter().add(shadow.as_shape(rect, Rounding::same(self.rounding)));
            }
            ui.painter().rect(rect, self.rounding, fill, stroke);
            let pos = match self.kind {
                Kind::Card => egui::pos2(rect.left() + self.padding.x, rect.center().y - galley.size().y / 2.0),
                _ => rect.center() - galley.size() / 2.0,
            };
            ui.painter().galley(pos, galley, text_color);
        }
        if enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
        }
    }
}

/// Filled accent button for the main action of a dialog.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(Btn::new(Kind::Primary, egui::RichText::new(text).family(bold())))
}

/// Grey button for other actions.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(Btn::new(Kind::Secondary, text))
}

/// Quiet button that only shows a background on hover.
pub fn ghost_button(ui: &mut Ui, text: impl Into<egui::RichText>) -> Response {
    ui.add(Btn::new(Kind::Ghost, text))
}

/// Round avatar with the character's initials.
pub fn avatar(ui: &mut Ui, name: &str, size: f32) -> Response {
    let p = of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let c = rect.center();
    let painter = ui.painter();
    painter.circle_filled(c, size / 2.0, p.accent);
    painter.circle_stroke(c, size / 2.0 - 0.5, Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    painter.text(c, egui::Align2::CENTER_CENTER, initials(name), FontId::new(size * 0.4, bold()), p.on_accent);
    resp
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let first = |w: &str| w.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    match words.as_slice() {
        [] => "AI".into(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [a, .., b] => format!("{}{}", first(a), first(b)),
    }
}

/// On/off switch. Returns the response; `on` is flipped when clicked.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = of(ui);
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let hover = hover_anim(ui, resp.id, resp.hovered());
    let track = mix(mix(p.surface_hi, p.border, 0.6 + 0.4 * hover), p.accent, t);
    ui.painter().rect_filled(rect, 11.0, track);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Coloured status dot.
pub fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    resp
}

pub enum RoundIcon {
    Send,
    Stop,
    Attach,
}

/// Round icon button drawn with shapes (no font glyphs needed).
pub fn round_button(ui: &mut Ui, icon: RoundIcon, enabled: bool, size: f32) -> Response {
    let p = of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), if enabled { Sense::click() } else { Sense::hover() });
    let t = hover_anim(ui, resp.id, enabled && resp.hovered());
    let pressed = enabled && resp.is_pointer_button_down_on();
    let c = rect.center();
    let painter = ui.painter();
    let (fill, fg) = match icon {
        RoundIcon::Attach => (mix(Color32::TRANSPARENT, p.accent_soft, t), mix(p.weak, p.accent, t)),
        _ if !enabled => (p.surface_hi, p.weak),
        _ => (mix(p.accent, p.accent_hover, t), p.on_accent),
    };
    let fill = if pressed { mix(fill, Color32::BLACK, 0.15) } else { fill };
    // soft glow ring while hovered
    if enabled && t > 0.0 && !matches!(icon, RoundIcon::Attach) {
        painter.circle_filled(c, size / 2.0 + 3.0 * t, mix(Color32::TRANSPARENT, p.accent_soft, t));
    }
    painter.circle_filled(c, size / 2.0, fill);
    let s = size / 40.0;
    let stroke = Stroke::new(2.2 * s, fg);
    match icon {
        RoundIcon::Send => {
            // arrow pointing up
            let top = c + egui::vec2(0.0, -8.0 * s);
            painter.line_segment([c + egui::vec2(0.0, 8.0 * s), top], stroke);
            painter.line_segment([top, c + egui::vec2(-6.5 * s, -1.5 * s)], stroke);
            painter.line_segment([top, c + egui::vec2(6.5 * s, -1.5 * s)], stroke);
        }
        RoundIcon::Stop => {
            painter.rect_filled(egui::Rect::from_center_size(c, Vec2::splat(11.0 * s)), 2.5 * s, fg);
        }
        RoundIcon::Attach => {
            // plus sign
            painter.line_segment([c + egui::vec2(-7.0 * s, 0.0), c + egui::vec2(7.0 * s, 0.0)], stroke);
            painter.line_segment([c + egui::vec2(0.0, -7.0 * s), c + egui::vec2(0.0, 7.0 * s)], stroke);
        }
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Three dots that pulse one after another ("the bot is typing").
pub fn typing_dots(ui: &mut Ui) {
    let p = of(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 16.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    for k in 0..3 {
        let phase = (t * 5.0 - k as f32 * 0.9).sin() * 0.5 + 0.5;
        let center = Pos2::new(rect.left() + 6.0 + k as f32 * 11.0, rect.center().y - phase * 3.0);
        let a = (110.0 + 145.0 * phase) as u8;
        ui.painter().circle_filled(center, 3.5, Color32::from_rgba_unmultiplied(p.weak.r(), p.weak.g(), p.weak.b(), a));
    }
    ui.ctx().request_repaint();
}

/// Rounded "pill" label (used for file chips and suggestions).
pub fn chip_frame(ui: &Ui) -> egui::Frame {
    let p = of(ui);
    egui::Frame::none()
        .fill(p.surface_hi)
        .rounding(Rounding::same(999.0))
        .inner_margin(egui::Margin::symmetric(10.0, 4.0))
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn makes_initials() {
        assert_eq!(initials("VibeCoding-Assitent für WPU-FEEL"), "VF");
        assert_eq!(initials("Helper"), "HE");
        assert_eq!(initials(""), "AI");
    }
}
