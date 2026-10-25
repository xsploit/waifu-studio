use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::voice_design::VoiceDesignState;
pub fn ui(ui: &mut egui::Ui, state: &mut VoiceDesignState) {
    kit::group(ui, "Voice design", |ui| {
        ui.small("Paid previews · $0.01 per successful request.")
            .on_hover_text("Price from the reviewed Fish documentation. This uses a separate preview endpoint. Candidate IDs are not TTS reference IDs; generating does not save a persistent voice model.");
        let busy = state.is_busy();
        ui.add_enabled_ui(!busy, |ui| {
            egui::CollapsingHeader::new("Advanced · connection")
                .id_salt("voice_design_connection")
                .default_open(false)
                .show(ui, |ui| {
                    kit::text_row(
                        ui,
                        "Endpoint",
                        "HTTPS, or localhost for testing.",
                        &mut state.endpoint,
                        "",
                    );
                    kit::secret_row(ui, "Session API key", "Not saved.", &mut state.session_key);
                    kit::text_row(
                        ui,
                        "Key variable",
                        "Used when no session key is entered.",
                        &mut state.api_key_env,
                        "",
                    );
                });
            kit::row(ui, "Voice description", "1–2000 characters.", |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut state.instruction)
                        .desired_width(240.0)
                        .desired_rows(3),
                );
            });
            kit::row(ui, "Preview text", "Up to 150 characters.", |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut state.reference_text)
                        .desired_width(240.0)
                        .desired_rows(2),
                );
            });
            kit::slider_row(ui, "Candidates", "", egui::Slider::new(&mut state.n, 1..=4));
            egui::CollapsingHeader::new("Advanced · preview tuning")
                .id_salt("voice_design_tuning")
                .default_open(false)
                .show(ui, |ui| {
                    kit::slider_row(
                        ui,
                        "Speed",
                        "",
                        egui::Slider::new(&mut state.speed, 0.01..=3.0),
                    );
                    let mut seeded = state.seed.is_some();
                    if kit::toggle_row(ui, "Use seed", "", &mut seeded).changed() {
                        state.seed = seeded.then_some(0);
                    }
                    if let Some(seed) = &mut state.seed {
                        kit::row(ui, "Seed", "Reuse a seed for repeatable previews.", |ui| {
                            ui.add(egui::DragValue::new(seed))
                        });
                    }
                });
            if ui.button("Generate paid previews").clicked()
                && let Err(error) = state.generate()
            {
                state.status = error;
            }
        });
        if busy {
            if ui.button("Cancel previews").clicked() {
                state.cancel();
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.label(&state.status);
        if !state.candidates.is_empty() {
            kit::row(ui, "Preview", "Choose a generated candidate.", |ui| {
                egui::ComboBox::from_id_salt("voice_design_candidate")
                    .selected_text(state.candidates.get(state.selected).map_or_else(
                        || "Choose preview…".to_owned(),
                        |candidate| format!("Candidate {}", candidate.index + 1),
                    ))
                    .show_ui(ui, |ui| {
                        for (index, candidate) in state.candidates.iter().enumerate() {
                            ui.selectable_value(
                                &mut state.selected,
                                index,
                                format!(
                                    "Candidate {} · {} ms · {} Hz",
                                    candidate.index + 1,
                                    candidate.duration_ms,
                                    candidate.sample_rate
                                ),
                            );
                        }
                    });
            });
        }
        if ui
            .add_enabled(
                !busy && !state.candidates.is_empty(),
                egui::Button::new("Play selected preview"),
            )
            .clicked()
        {
            state.preview_selected();
        }
    });
}
