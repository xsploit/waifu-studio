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
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for name in &state.available {
                ui.selectable_value(&mut state.config.mouth_expression, name.clone(), name);
            }
        });
    if !state.config.lip_sync {
        ui.weak("Mouth movement is disabled here, including calibrated vowels.");
    } else if state.config.mouth_gain <= 0.0 {
        ui.weak("Mouth sensitivity is zero: audio-envelope mouth movement is disabled.");
    } else if !state.available.contains(&state.config.mouth_expression) {
        ui.weak("The selected mouth expression is unavailable on this avatar. Choose one from the dropdown for audio-envelope lip sync.");
    }
    if ui.button("Restore mouth defaults").clicked() {
        let defaults = waifu_studio::expressions::FacialConfig::default();
        state.config.lip_sync = defaults.lip_sync;
        state.config.mouth_gain = defaults.mouth_gain;
        state.config.mouth_expression = defaults.mouth_expression;
    }
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
