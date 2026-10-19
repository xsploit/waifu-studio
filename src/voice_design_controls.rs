use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::voice_design::VoiceDesignState;
pub fn ui(ui: &mut egui::Ui, state: &mut VoiceDesignState) {
    kit::group(ui, "Voice Design", |ui| {
        kit::note(
            ui,
            "Separate paid preview endpoint: $0.01 per successful request in reviewed Fish documentation. Candidate IDs are not TTS reference IDs; generating does not save a persistent voice model.",
        );
        let busy = state.is_busy();
        ui.add_enabled_ui(!busy, |ui| {
            kit::text_row(
                ui,
                "Endpoint (HTTPS, or localhost for testing)",
                "HTTPS, or localhost for testing.",
                &mut state.endpoint,
                "",
            );
            kit::secret_row(ui, "Session API key", "Not saved.", &mut state.session_key);
            kit::text_row(
                ui,
                "Or API key environment variable",
                "Used when no session key is entered.",
                &mut state.api_key_env,
                "",
            );
            kit::row(
                ui,
                "Describe the voice (1–2000 characters)",
                "1–2000 characters.",
                |ui| {
                    ui.add(egui::TextEdit::multiline(&mut state.instruction).desired_width(240.0));
                },
            );
            kit::row(
                ui,
                "Preview text (up to 150 characters)",
                "Up to 150 characters.",
                |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut state.reference_text).desired_width(240.0),
                    );
                },
            );
            kit::slider_row(ui, "Candidates", "", egui::Slider::new(&mut state.n, 1..=4));
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
            if ui.button("Generate paid previews").clicked()
                && let Err(error) = state.generate()
            {
                state.status = error;
            }
        });
        if busy {
            if ui.button("Cancel preview request").clicked() {
                state.cancel();
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.label(&state.status);
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
