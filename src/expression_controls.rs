use bevy_egui::egui;
use waifu_studio::expressions::FacialState;
use waifu_studio::settings_kit as kit;

/// Returns true when persistent character settings changed.
pub fn ui(ui: &mut egui::Ui, state: &mut FacialState) -> bool {
    let before = state.config.clone();
    kit::group(ui, "Face & lip sync", |ui| {
        ui.label(&state.status);
        kit::row(ui, "Facial expression", "", |ui| {
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
                        ui.selectable_value(
                            &mut state.config.selected_expression,
                            name.clone(),
                            name,
                        );
                    }
                });
        });
        kit::slider_row(
            ui,
            "Expression strength",
            "",
            egui::Slider::new(&mut state.config.expression_weight, 0.0..=1.0),
        );
        kit::toggle_row(ui, "Automatic blinking", "", &mut state.config.auto_blink);
        kit::slider_row(
            ui,
            "Blink strength",
            "",
            egui::Slider::new(&mut state.config.blink_strength, 0.0..=1.0),
        );
        kit::slider_row(
            ui,
            "Blink interval (seconds)",
            "",
            egui::Slider::new(&mut state.config.blink_interval_secs, 1.5..=10.0),
        );
        kit::toggle_row(ui, "Audio lip sync", "", &mut state.config.lip_sync);
        kit::slider_row(
            ui,
            "Mouth sensitivity",
            "",
            egui::Slider::new(&mut state.config.mouth_gain, 0.0..=10.0),
        );
        kit::row(ui, "Mouth expression", "", |ui| {
            egui::ComboBox::from_id_salt("mouth_expression")
                .selected_text(&state.config.mouth_expression)
                .width(200.0)
                .show_ui(ui, |ui| {
                    for name in &state.available {
                        ui.selectable_value(&mut state.config.mouth_expression, name.clone(), name);
                    }
                });
        });
        if !state.config.lip_sync {
            kit::note(
                ui,
                "Audio-driven mouth movement is disabled, including calibrated vowels. Manual expressions still apply.",
            );
        } else if state.config.mouth_gain <= 0.0 {
            kit::note(
                ui,
                "Mouth sensitivity is zero: audio-envelope mouth movement is disabled.",
            );
        } else if !state.available.contains(&state.config.mouth_expression) {
            kit::note(
                ui,
                "The selected mouth expression is unavailable on this avatar. Choose one from the dropdown for audio-envelope lip sync.",
            );
        }
        kit::group(ui, "Live mouth diagnostics", |ui| {
            ui.add(
                egui::ProgressBar::new(state.speech_level)
                    .text(format!("Speech signal: {:.0}%", state.speech_level * 100.)),
            );
            ui.add(egui::ProgressBar::new(state.mouth_weight).text(format!(
                "Mouth expression weight: {:.0}%",
                state.mouth_weight * 100.
            )));
            kit::note(
                ui,
                "Selected character only. Signal comes from speech samples consumed for playback, before mouth sensitivity. Expression weight includes manual controls and expression overrides; it does not prove visible motion or speaker timing.",
            );
        });
        if ui.button("Restore mouth defaults").clicked() {
            let defaults = waifu_studio::expressions::FacialConfig::default();
            state.config.lip_sync = defaults.lip_sync;
            state.config.mouth_gain = defaults.mouth_gain;
            state.config.mouth_expression = defaults.mouth_expression;
        }
        kit::group(ui, "Manual expression weights", |ui| {
            for name in &state.available {
                let mut weight = state.config.weights.get(name).copied().unwrap_or_default();
                if kit::slider_row(ui, name, "", egui::Slider::new(&mut weight, 0.0..=1.0))
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
    });
    state.config != before
}
