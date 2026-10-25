use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::viseme_audio::VisemeState;

pub fn ui(ui: &mut egui::Ui, state: &mut VisemeState) -> bool {
    let before = state.config.clone();
    state.poll_dialog();
    kit::group(ui, "Vowel lip sync", |ui| {
        ui.small("Use a voice calibration for A I U E O.")
            .on_hover_text("A wLipSync or uLipSync MFCC profile classifies vowels on a background worker. Without a compatible profile, speech falls back to the RMS audio envelope.");
        kit::row(
            ui,
            "Analysis method",
            "Choose an envelope or calibrated vowel profile.",
            |ui| {
                egui::ComboBox::from_id_salt("lip-sync-method")
                    .selected_text(if state.config.enabled {
                        "Calibrated vowels"
                    } else {
                        "Audio envelope"
                    })
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.config.enabled, false, "Audio envelope");
                        ui.selectable_value(&mut state.config.enabled, true, "Calibrated vowels");
                    });
            },
        );
        if state.config.enabled && state.vowel_coverage().is_none() {
            kit::note(
                ui,
                "No calibration loaded: this method falls back to the audio envelope when Audio lip sync is enabled above. Load a matching profile to use vowels.",
            );
        }
        kit::text_row(
            ui,
            "Calibration profile",
            "uLipSync JSON or wLipSync WLIP file.",
            &mut state.profile_draft,
            "Profile path",
        );
        kit::row(
            ui,
            "Profile actions",
            "Load, browse, or remove the calibration.",
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!state.loading(), egui::Button::new("Load"))
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
            },
        );
        kit::note(
            ui,
            "Import a uLipSync JSON calibration or wLipSync WLIP .bin file. Profiles should match the synthesized voice. No calibration is bundled.",
        );
        egui::CollapsingHeader::new("Advanced · analysis")
            .id_salt("lipsync_analysis")
            .default_open(false)
            .show(ui, |ui| {
        if ui.button("Reset analysis tuning").clicked() {
            let defaults = waifu_studio::viseme_audio::VisemeConfig::default();
            state.config.min_volume = defaults.min_volume;
            state.config.max_volume = defaults.max_volume;
            state.config.smoothness = defaults.smoothness;
            state.config.block_size = defaults.block_size;
            state.config.use_phoneme_blend = defaults.use_phoneme_blend;
            state.config.vowel_gains = defaults.vowel_gains;
            state.config.average_channels = defaults.average_channels;
        }
        kit::note(
            ui,
            "Reset keeps your selected method and calibration file. Mouth strength and the master lip-sync switch are in Face controls above.",
        );
        kit::slider_row(
            ui,
            "Minimum volume",
            "Log10 RMS.",
            egui::Slider::new(&mut state.config.min_volume, -5.0..=0.0),
        );
        kit::slider_row(
            ui,
            "Maximum volume",
            "Log10 RMS.",
            egui::Slider::new(&mut state.config.max_volume, -4.0..=0.5),
        );
        kit::slider_row(
            ui,
            "Smoothing",
            "",
            egui::Slider::new(&mut state.config.smoothness, 0.0..=0.3).suffix(" s"),
        );
        kit::row(
            ui,
            "Analysis block",
            "Samples per analysis window at 48 kHz.",
            |ui| {
                egui::ComboBox::from_id_salt("native-mfcc-block")
                    .selected_text(format!("{} samples at 48 kHz", state.config.block_size))
                    .show_ui(ui, |ui| {
                        for size in [128, 256, 512, 1024, 2048] {
                            ui.selectable_value(
                                &mut state.config.block_size,
                                size,
                                size.to_string(),
                            );
                        }
                    });
            },
        );
        kit::toggle_row(
            ui,
            "Blend phoneme scores",
            "",
            &mut state.config.use_phoneme_blend,
        );
        kit::toggle_row(
            ui,
            "Average channels",
            "Off uses the first input channel.",
            &mut state.config.average_channels,
        );
        kit::group(ui, "Vowel gains", |ui| {
            for (name, gain) in ["A", "I", "U", "E", "O"]
                .iter()
                .zip(&mut state.config.vowel_gains)
            {
                kit::slider_row(ui, *name, "", egui::Slider::new(gain, 0.0..=4.0));
            }
        });
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
        egui::CollapsingHeader::new("Advanced · live vowels")
            .id_salt("lipsync_live_vowels")
            .default_open(false)
            .show(ui, |ui| {
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
            });
    });
    state.config != before
}
