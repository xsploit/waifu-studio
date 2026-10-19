//! Settings building blocks: a centered column of titled cards whose rows put a
//! label and one-line hint on the left and a single control on the right.
use bevy_egui::egui::{self, Color32, RichText, Stroke};

const CARD: Color32 = Color32::from_rgb(31, 34, 32);
const CARD_LINE: Color32 = Color32::from_rgb(48, 52, 50);
const TITLE: Color32 = Color32::from_rgb(232, 233, 230);
const HINT: Color32 = Color32::from_rgb(140, 145, 142);
const ON: Color32 = Color32::from_rgb(214, 104, 112);
pub const COLUMN: f32 = 680.0;

/// Small label above a card.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).size(14.0).color(TITLE));
    ui.add_space(4.0);
}

/// Rounded card; draw rows with [`Card::row`]. Separators go between rows only.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut Card) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut card = Card { ui, rows: 0 };
            add(&mut card)
        })
        .inner
}

pub struct Card<'a> {
    pub ui: &'a mut egui::Ui,
    rows: usize,
}
impl Card<'_> {
    fn divider(&mut self) {
        if self.rows > 0 {
            let width = self.ui.available_width();
            let (rect, _) = self
                .ui
                .allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
            self.ui.painter().rect_filled(rect, 0.0, CARD_LINE);
        }
        self.rows += 1;
    }
    /// Label and hint on the left; `control` is laid out right-to-left on the right.
    pub fn row<R>(
        &mut self,
        title: &str,
        hint: &str,
        control: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        self.divider();
        row_body(self.ui, title, hint, control)
    }
    /// Free-form content spanning the card width (lists, editors).
    pub fn block<R>(&mut self, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.divider();
        let ui = &mut *self.ui;
        ui.add_space(10.0);
        let inner = add(ui);
        ui.add_space(10.0);
        inner
    }
}

fn row_body<R>(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.add_space(10.0);
    let (_, inner) = egui::Sides::new().shrink_left().wrap().show(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if hint.is_empty() {
                    // Line a lone title up with the control's text.
                    ui.add_space(5.0);
                }
                ui.label(RichText::new(title).size(14.0).color(TITLE));
                if !hint.is_empty() {
                    ui.label(RichText::new(hint).size(12.0).color(HINT));
                }
            });
        },
        |ui| {
            ui.set_max_width(ui.available_width().min(280.0));
            control(ui)
        },
    );
    ui.add_space(10.0);
    inner
}

/// Hairline between stacked rows; skipped at the top of a container.
fn divider_if_needed(ui: &mut egui::Ui) {
    if ui.cursor().top() - ui.min_rect().top() > 2.0 {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, CARD_LINE);
    }
}

/// A settings row anywhere (inside [`group`] or a plain container).
pub fn row<R>(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    divider_if_needed(ui);
    row_body(ui, title, hint, control)
}

/// Row with a switch on the right; drop-in for `ui.checkbox(value, title)`.
pub fn toggle_row(
    ui: &mut egui::Ui,
    title: impl AsRef<str>,
    hint: &str,
    value: &mut bool,
) -> egui::Response {
    row(ui, title.as_ref(), hint, |ui| toggle(ui, value))
}

/// Row with a slider on the right; pass the slider without `.text(...)`.
pub fn slider_row(
    ui: &mut egui::Ui,
    title: impl AsRef<str>,
    hint: &str,
    slider: impl egui::Widget,
) -> egui::Response {
    row(ui, title.as_ref(), hint, |ui| {
        ui.spacing_mut().slider_width = 160.0;
        ui.add(slider)
    })
}

/// Row with a single-line text box on the right.
pub fn text_row(
    ui: &mut egui::Ui,
    title: impl AsRef<str>,
    hint: &str,
    value: &mut String,
    placeholder: &str,
) -> egui::Response {
    row(ui, title.as_ref(), hint, |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(placeholder)
                .desired_width(240.0),
        )
    })
}

/// Row with a masked text box on the right (keys, tokens).
pub fn secret_row(
    ui: &mut egui::Ui,
    title: impl AsRef<str>,
    hint: &str,
    value: &mut String,
) -> egui::Response {
    row(ui, title.as_ref(), hint, |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .password(true)
                .hint_text("Session only")
                .desired_width(240.0),
        )
    })
}

/// Muted explanatory line; use instead of paragraphs of body text.
pub fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).size(12.0).color(HINT));
}

/// Collapsible card with a title, open by default; holds rows.
pub fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) {
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::CollapsingHeader::new(RichText::new(title).size(14.0).color(TITLE))
                .default_open(true)
                .show_unindented(ui, add);
        });
}

/// iOS-style switch. Returns a response whose `changed()` reports a flip.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let off = if response.hovered() {
            Color32::from_rgb(78, 83, 80)
        } else {
            Color32::from_rgb(62, 66, 64)
        };
        let fill = lerp_color(off, ON, t);
        let fill = if ui.is_enabled() {
            fill
        } else {
            fill.gamma_multiply(0.4)
        };
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, fill);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter().circle_filled(
            egui::pos2(x, rect.center().y),
            radius - 3.0,
            Color32::from_rgb(245, 245, 243),
        );
    }
    response
}
fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Centers settings content in a fixed-width column like a native preferences page.
pub fn column<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let width = ui.available_width().min(COLUMN);
    let margin = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}
