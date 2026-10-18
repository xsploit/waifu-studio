use bevy_egui::egui;
use waifu_studio::voice_design::VoiceDesignState;
pub fn ui(ui: &mut egui::Ui, state: &mut VoiceDesignState) {
    ui.heading("Fish Voice Design");
    ui.label("Separate paid preview endpoint: $0.01 per successful request in reviewed Fish documentation. Candidate IDs are not TTS reference IDs; generating does not save a persistent voice model.");
    let busy = state.is_busy();
    ui.add_enabled_ui(!busy, |ui| {
        ui.label("Endpoint (HTTPS, or localhost for testing)");
        ui.text_edit_singleline(&mut state.endpoint);
        ui.label("Session API key (not saved)");
        ui.add(egui::TextEdit::singleline(&mut state.session_key).password(true));
        ui.label("Or API key environment variable");
        ui.text_edit_singleline(&mut state.api_key_env);
        ui.label("Describe the voice (1–2000 characters)");
        ui.text_edit_multiline(&mut state.instruction);
        ui.label("Preview text (up to 150 characters)");
        ui.text_edit_multiline(&mut state.reference_text);
        ui.add(egui::Slider::new(&mut state.n, 1..=4).text("Candidates"));
        ui.add(egui::Slider::new(&mut state.speed, 0.01..=3.0).text("Speed"));
        let mut seeded = state.seed.is_some();
        if ui.checkbox(&mut seeded, "Use seed").changed() {
            state.seed = seeded.then_some(0);
        }
        if let Some(seed) = &mut state.seed {
            ui.add(egui::DragValue::new(seed));
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
}
