use bevy_egui::egui;
use waifu_studio::expressions::FacialState;

/// Returns true when persistent character settings changed.
pub fn ui(ui: &mut egui::Ui, state: &mut FacialState) -> bool {
    let before = state.config.clone();
    ui.heading("Face & lip sync");
    ui.label(&state.status);
    egui::ComboBox::from_id_salt("facial_expression")
        .selected_text(if state.config.selected_expression.is_empty() {
            "Neutral / manual"
        } else {
            &state.config.selected_expression
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut state.config.selected_expression,
                String::new(),
                "Neutral / manual",
            );
            for name in &state.available {
                ui.selectable_value(&mut state.config.selected_expression, name.clone(), name);
            }
        });
    ui.add(
        egui::Slider::new(&mut state.config.expression_weight, 0.0..=1.0)
            .text("Expression strength"),
    );
    ui.checkbox(&mut state.config.auto_blink, "Automatic blinking");
    ui.add(egui::Slider::new(&mut state.config.blink_strength, 0.0..=1.0).text("Blink strength"));
    ui.add(
        egui::Slider::new(&mut state.config.blink_interval_secs, 1.5..=10.0)
            .text("Blink interval (seconds)"),
    );
    ui.checkbox(&mut state.config.lip_sync, "Audio lip sync");
    ui.add(egui::Slider::new(&mut state.config.mouth_gain, 0.0..=10.0).text("Mouth sensitivity"));
    egui::ComboBox::from_id_salt("mouth_expression")
        .selected_text(&state.config.mouth_expression)
        .show_ui(ui, |ui| {
            for name in &state.available {
                ui.selectable_value(&mut state.config.mouth_expression, name.clone(), name);
            }
        });
    ui.collapsing("Manual expression weights", |ui| {
        for name in &state.available {
            let mut weight = state.config.weights.get(name).copied().unwrap_or_default();
            if ui
                .add(egui::Slider::new(&mut weight, 0.0..=1.0).text(name))
                .changed()
            {
                if weight > 0.0 {
                    state.config.weights.insert(name.clone(), weight);
                } else {
                    state.config.weights.remove(name);
                }
            }
        }
        if ui.button("Reset manual weights").clicked() {
            state.config.weights.clear();
        }
    });
    state.config != before
}
