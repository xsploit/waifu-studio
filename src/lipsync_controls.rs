use bevy_egui::egui;
use waifu_studio::viseme_audio::VisemeState;

pub fn ui(ui: &mut egui::Ui, state: &mut VisemeState) -> bool {
    let before = state.config.clone();
    state.poll_dialog();
    ui.heading("Native wLipSync / uLipSync vowels");
    ui.label("A calibrated MFCC profile classifies A I U E O on a background worker. Without a compatible profile, speech uses the existing RMS mouth envelope.");
    ui.checkbox(
        &mut state.config.enabled,
        "Enable calibrated vowel lip sync",
    );
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.profile_draft);
        if ui
            .add_enabled(!state.loading(), egui::Button::new("Load profile"))
            .clicked()
        {
            let path = std::path::PathBuf::from(state.profile_draft.trim());
            if let Err(error) = state.import_profile(path) {
                state.status = error;
            }
        }
        if ui.button("Browse…").clicked() {
            state.open_profile_dialog();
        }
        if ui.button("Clear").clicked() {
            state.clear_profile();
        }
    });
    ui.small("Import a uLipSync JSON calibration or wLipSync WLIP .bin file. Profiles should match the synthesized voice. No calibration is bundled.");
    ui.add(egui::Slider::new(&mut state.config.min_volume, -5.0..=0.0).text("Minimum log10 RMS"));
    ui.add(egui::Slider::new(&mut state.config.max_volume, -4.0..=0.5).text("Maximum log10 RMS"));
    ui.add(egui::Slider::new(&mut state.config.smoothness, 0.0..=0.3).text("Smoothing (seconds)"));
    egui::ComboBox::from_id_salt("native-mfcc-block")
        .selected_text(format!("{} samples at 48 kHz", state.config.block_size))
        .show_ui(ui, |ui| {
            for size in [128, 256, 512, 1024, 2048] {
                ui.selectable_value(&mut state.config.block_size, size, size.to_string());
            }
        });
    ui.checkbox(
        &mut state.config.use_phoneme_blend,
        "Blend calibrated phoneme scores",
    );
    ui.checkbox(
        &mut state.config.average_channels,
        "Average input channels (default: first channel)",
    );
    ui.collapsing("Vowel gains", |ui| {
        for (name, gain) in ["A", "I", "U", "E", "O"]
            .iter()
            .zip(&mut state.config.vowel_gains)
        {
            ui.add(egui::Slider::new(gain, 0.0..=4.0).text(*name));
        }
    });
    ui.label(&state.status);
    if let Some(coverage) = state.vowel_coverage() {
        ui.small(format!(
            "Calibrated vowels: {}",
            ["A", "I", "U", "E", "O"]
                .iter()
                .zip(coverage)
                .filter_map(|(name, present)| present.then_some(*name))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    if let Some(weights) = state.weights() {
        for (name, value) in ["A", "I", "U", "E", "O"].iter().zip(weights) {
            ui.add(egui::ProgressBar::new(value).text(format!("{name} {value:.2}")));
        }
    }
    if state.dropped_samples() > 0 {
        ui.small(format!(
            "{} analysis samples dropped; temporal analysis resets after overflow",
            state.dropped_samples()
        ));
    }
    state.config != before
}
