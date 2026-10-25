//! Compact settings cards with bounded labels and right-aligned controls.
//! Narrow rows stack their controls below the label without changing widget salts.
use bevy_egui::egui::{self, Color32, RichText, Stroke};

const CARD: Color32 = Color32::from_rgb(28, 28, 28);
const CARD_LINE: Color32 = Color32::from_rgb(42, 42, 42);
const TITLE: Color32 = Color32::from_rgb(232, 232, 232);
const HINT: Color32 = Color32::from_rgb(145, 145, 145);
const ON: Color32 = Color32::from_rgb(214, 104, 112);
pub const COLUMN: f32 = 680.0;

/// Small label above a card.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(13.0).color(TITLE));
    ui.add_space(4.0);
}

/// Rounded card; draw rows with [`Card::row`]. Separators go between rows only.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut Card) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 4))
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
        row_body(self.ui, title, hint, control).0
    }
    /// Free-form content spanning the card width (lists, editors).
    pub fn block<R>(&mut self, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.divider();
        let ui = &mut *self.ui;
        ui.add_space(4.0);
        let inner = add(ui);
        ui.add_space(4.0);
        inner
    }
}

/// Return the label bounds as well so a stacked switch keeps a clickable label.
fn row_body<R>(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    control: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, egui::Rect) {
    ui.add_space(4.0);
    let rect = ui.available_rect_before_wrap();
    let width = rect.width().max(0.0);
    let stacked = width < 480.0;
    let control_width = width.min(280.0);
    let height = ui.spacing().interact_size.y.max(22.0);
    // Measure stacked text before drawing controls so the control-first child
    // order (and existing automatic widget ids) stays the same at every width.
    let top = if stacked {
        let text_height = |text: &str, size, color| {
            egui::WidgetText::from(RichText::new(text).size(size).color(color))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Wrap),
                    width,
                    egui::TextStyle::Body,
                )
                .size()
                .y
        };
        rect.top()
            + text_height(title, 13.0, TITLE)
            + if hint.is_empty() {
                0.0
            } else {
                1.0 + text_height(hint, 12.0, HINT)
            }
            + 4.0
    } else {
        rect.top()
    };
    let mut controls = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(rect.right() - control_width, top),
                egui::vec2(control_width, height),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true)),
    );
    controls.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    controls.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
    controls.spacing_mut().slider_width = (control_width - 72.0).clamp(48.0, 150.0);
    controls.spacing_mut().text_edit_width = control_width.min(240.0);
    controls.spacing_mut().combo_width = control_width.min(240.0);
    let inner = control(&mut controls);
    let label_width = if stacked {
        width
    } else {
        (controls.min_rect().left() - rect.left() - 16.0).max(0.0)
    };
    let mut labels = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                rect.min,
                egui::vec2(label_width, height),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    labels.set_width(label_width);
    labels.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if hint.is_empty() && !stacked {
            ui.add_space(((height - 16.0) * 0.5).max(0.0));
        }
        ui.add(egui::Label::new(RichText::new(title).size(13.0).color(TITLE)).wrap());
        if !hint.is_empty() {
            ui.add(egui::Label::new(RichText::new(hint).size(12.0).color(HINT)).wrap());
        }
    });
    let label_rect = labels.min_rect();
    ui.advance_cursor_after_rect(label_rect | controls.min_rect());
    ui.add_space(4.0);
    (inner, label_rect)
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
    row_body(ui, title, hint, control).0
}

/// Row with a switch on the right; drop-in for `ui.checkbox(value, title)`.
pub fn toggle_row(
    ui: &mut egui::Ui,
    title: impl AsRef<str>,
    hint: &str,
    value: &mut bool,
) -> egui::Response {
    divider_if_needed(ui);
    let (mut response, label_rect) = row_body(ui, title.as_ref(), hint, |ui| toggle(ui, value));
    let label = ui.interact(label_rect, response.id.with("label"), egui::Sense::click());
    label.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            *value,
            title.as_ref(),
        )
    });
    if label.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.union(label)
}

/// Row with a slider on the right; pass the slider without `.text(...)`.
pub fn slider_row(
    ui: &mut egui::Ui,
    title: impl AsRef<str>,
    hint: &str,
    slider: impl egui::Widget,
) -> egui::Response {
    row(ui, title.as_ref(), hint, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 72.0).clamp(48.0, 150.0);
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
                .desired_width(ui.available_width().min(240.0)),
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
                .desired_width(ui.available_width().min(240.0)),
        )
    })
}

/// Muted explanatory line; use instead of paragraphs of body text.
pub fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(12.0).color(HINT)).wrap());
}

/// Collapsible card with a title, open by default; holds rows.
pub fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) {
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::CollapsingHeader::new(RichText::new(title).size(13.0).color(TITLE))
                .default_open(true)
                .show_unindented(ui, add);
        });
}

/// A collapsed Advanced section. Use a unique page id; existing capture and
/// `("open-advanced", id)` requests can still reveal and scroll to its controls.
pub fn advanced<R>(
    ui: &mut egui::Ui,
    id: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    ui.add_space(8.0);
    let capture = ui.ctx().data(|data| {
        data.get_temp::<bool>(egui::Id::new("capture-advanced"))
            .unwrap_or(false)
    });
    let requested = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<bool>(egui::Id::new(("open-advanced", id))))
        .unwrap_or(false);
    let response = egui::CollapsingHeader::new(RichText::new("Advanced").size(13.0))
        .id_salt((id, "advanced"))
        .default_open(capture)
        .open(requested.then_some(true))
        .show_unindented(ui, add);
    if requested {
        response
            .header_response
            .scroll_to_me(Some(egui::Align::TOP));
    }
    let scroll_id = egui::Id::new(("capture-advanced-scrolled", id));
    if capture
        && !ui
            .ctx()
            .data(|data| data.get_temp::<bool>(scroll_id).unwrap_or(false))
    {
        response
            .header_response
            .scroll_to_me(Some(egui::Align::TOP));
        ui.ctx().data_mut(|data| data.insert_temp(scroll_id, true));
    }
    response
}

/// Compact switch. Returns a response whose `changed()` reports a flip.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(34.0, 20.0);
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
            Color32::from_rgb(80, 80, 80)
        } else {
            Color32::from_rgb(64, 64, 64)
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

/// Centers a bounded settings column, shrinking to the available width.
pub fn column<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let width = ui.available_width().min(COLUMN);
    let margin = ((ui.available_width() - width) / 2.0).max(0.0);
    let item_spacing = ui.spacing().item_spacing.x;
    ui.horizontal(|ui| {
        // add_space already accounts for the inset; horizontal item spacing would
        // otherwise push the column past its available right edge.
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.x = item_spacing;
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        width: f32,
        events: Vec<egui::Event>,
        add: impl FnMut(&mut egui::Ui),
    ) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 1000.0),
                )),
                events,
                ..Default::default()
            },
            add,
        );
        output.textures_delta.clear();
    }

    #[test]
    fn labels_controls_and_cards_fit_wide_and_narrow_columns() {
        for width in [320.0, 420.0, 520.0, 800.0, 1280.0] {
            let ctx = egui::Context::default();
            ctx.style_mut_of(egui::Theme::Dark, |style| {
                style.spacing.item_spacing = egui::vec2(8.0, 6.0);
                style.spacing.interact_size.y = 26.0;
            });
            frame(&ctx, width, vec![], |ui| {
                let available = ui.max_rect();
                column(ui, |ui| {
                    let column = ui.max_rect();
                    assert_eq!(ui.spacing().item_spacing.x, 8.0);
                    section(ui, "Connections");
                    card(ui, |card| {
                        let (control, label) = row_body(
                            card.ui,
                            "Transcription service",
                            "Keys are managed in Connections.",
                            |ui| {
                                egui::ComboBox::from_id_salt("service")
                                    .width(ui.available_width().min(240.0))
                                    .selected_text(
                                        "A long provider name that must not expand the row",
                                    )
                                    .truncate()
                                    .show_ui(ui, |_| {})
                                    .response
                            },
                        );
                        assert!(!control.rect.intersects(label), "overlap at {width}");
                        assert!(control.rect.right() <= column.right() + 1.0);
                        assert!(control.rect.left() >= column.left() - 1.0);
                        let (_, button_label) = row_body(card.ui, "Two actions", "", |ui| {
                            let first = ui.button("Stop");
                            let second = ui.button("Preview");
                            assert!(second.rect.right() <= first.rect.left());
                            first.union(second)
                        });
                        assert!(button_label.right() <= column.right() + 1.0);
                        card.row("Service status", "", |ui| {
                            ui.label("Ready");
                        });
                        let mut value = String::new();
                        text_row(card.ui, "Endpoint", "", &mut value, "URL");
                        secret_row(card.ui, "Session key", "", &mut value);
                        let mut speed = 1.0;
                        slider_row(
                            card.ui,
                            "Speed",
                            "",
                            egui::Slider::new(&mut speed, 0.5..=2.0),
                        );
                        card.block(|ui| note(ui, "Long technical notes wrap within their card rather than widening the settings column."));
                    });
                    group(ui, "Another group", |ui| note(ui, "Compact content"));
                    assert!(
                        ui.min_rect().right() <= column.right() + 1.0,
                        "column overflow at {width}"
                    );
                });
                assert!(
                    ui.min_rect().right() <= available.right() + 1.0,
                    "page overflow at {width}"
                );
            });
        }
    }

    #[test]
    fn resizing_keeps_widget_ids_and_switch_labels_work_when_stacked() {
        let ctx = egui::Context::default();
        let mut enabled = false;
        let mut first_id = None;
        let mut label_pos = egui::Pos2::ZERO;
        for width in [800.0, 320.0] {
            frame(&ctx, width, vec![], |ui| {
                let response = toggle_row(ui, "Enable feature", "Optional feature", &mut enabled);
                if let Some(id) = first_id {
                    assert_eq!(response.id, id);
                } else {
                    first_id = Some(response.id);
                }
                label_pos = response.rect.left_top() + egui::vec2(10.0, 10.0);
            });
        }
        let click = vec![
            egui::Event::PointerMoved(label_pos),
            egui::Event::PointerButton {
                pos: label_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: label_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        frame(&ctx, 320.0, click.clone(), |ui| {
            assert!(toggle_row(ui, "Enable feature", "Optional feature", &mut enabled).changed());
        });
        assert!(enabled);
        frame(&ctx, 320.0, click, |ui| {
            ui.disable();
            assert!(!toggle_row(ui, "Enable feature", "Optional feature", &mut enabled).changed());
        });
        assert!(enabled, "disabled labels must not toggle settings");
    }

    #[test]
    fn advanced_is_closed_and_existing_open_requests_still_work() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
        let mut rendered = false;
        let mut header_id = None;
        frame(&ctx, 800.0, vec![], |ui| {
            header_id = Some(
                advanced(ui, "fixture", |_| rendered = true)
                    .header_response
                    .id,
            );
        });
        assert!(!rendered);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(("open-advanced", "fixture")), true));
        frame(&ctx, 800.0, vec![], |ui| {
            assert_eq!(
                advanced(ui, "fixture", |_| rendered = true)
                    .header_response
                    .id,
                header_id.unwrap()
            );
        });
        assert!(rendered);
        assert!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new(("open-advanced", "fixture"))))
                .is_none()
        );
        let capture = egui::Context::default();
        capture.data_mut(|data| data.insert_temp(egui::Id::new("capture-advanced"), true));
        rendered = false;
        frame(&capture, 800.0, vec![], |ui| {
            advanced(ui, "fixture", |_| rendered = true);
        });
        assert!(rendered, "capture mode still reveals the complete page");
    }
}
