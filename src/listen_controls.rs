use bevy_egui::egui;
use waifu_studio::listen::{ListeningState, SttBackend};
use waifu_studio::settings_kit as kit;
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut ListeningState,
    character: Option<uuid::Uuid>,
    composer_empty: bool,
    catalog: &mut waifu_studio::model_catalog::ModelCatalog,
) {
    kit::group(ui, "Microphone", |ui| {
        ui.small("Start manually. Stop or silence sends audio; Cancel discards it.")
            .on_hover_text("An upload already sent cannot be recalled.");
        let backend = state.config.backend;
        if backend == SttBackend::VercelGateway {
            ui.small("Vercel transcription is beta.").on_hover_text(
                "Availability depends on your team. Uses the v4 transcription protocol.",
            );
        }
        egui::CollapsingHeader::new("Advanced · transcription")
            .id_salt("microphone_transcription")
            .default_open(false)
            .show(ui, |ui| {
                kit::text_row(
                    ui,
                    "Endpoint",
                    "Localhost and LAN endpoints are supported.",
                    &mut state.config.endpoint,
                    "Endpoint URL",
                );
                if ui.small_button("Refresh models").clicked() {
                    catalog.retry(
                        &state.config.endpoint,
                        &state.config.api_key_env,
                        &state.session_key,
                        waifu_studio::model_catalog::CatalogKind::Transcription,
                    );
                }
                kit::text_row(
                    ui,
                    "Model ID",
                    "Transcription model served by this endpoint.",
                    &mut state.config.model,
                    "",
                );
                kit::text_row(
                    ui,
                    "Key variable",
                    "Environment variable containing the key.",
                    &mut state.config.api_key_env,
                    "",
                );
                if state.config.backend != SttBackend::VercelGateway {
                    kit::text_row(
                        ui,
                        "Language hint",
                        "Optional language code, such as en or ja.",
                        &mut state.config.language,
                        "",
                    );
                }
            });
        catalog.sync(
            &state.config.endpoint,
            &state.config.api_key_env,
            &state.session_key,
            waifu_studio::model_catalog::CatalogKind::Transcription,
        );

        egui::CollapsingHeader::new("Advanced · speech detection")
            .id_salt("microphone_detection")
            .default_open(false)
            .show(ui, |ui| {
        kit::toggle_row(
            ui,
            "Silero VAD",
            "Use a local neural speech detector.",
            &mut state.config.silero_enabled,
        );
        if state.config.silero_enabled {
            kit::text_row(
                ui,
                "Silero model",
                "Current 512-frame ONNX model.",
                &mut state.config.silero_model,
                "File path",
            );
            kit::text_row(
                ui,
                "Runtime library",
                "Trusted native ONNX Runtime library.",
                &mut state.config.silero_runtime,
                "File path",
            );
            kit::note(
                ui,
                "No downloads. Missing/incompatible files fall back to energy VAD. Runtime library changes may require an app restart.",
            );
            kit::slider_row(
                ui,
                "Speech probability",
                "",
                egui::Slider::new(&mut state.config.silero_threshold, 0.01..=0.99),
            );
        }
        if let Ok(status) = state.vad_status.lock() {
            ui.label(status.as_str());
        }
        kit::slider_row(
            ui,
            "Energy VAD threshold",
            "",
            egui::Slider::new(&mut state.config.vad_threshold, 0.001..=0.2).logarithmic(true),
        );
        });
        kit::row(
            ui,
            "Silence preset",
            "How long to wait after speech ends.",
            |ui| {
                egui::ComboBox::from_id_salt("microphone-endpoint-delay")
                    .selected_text(match state.config.trailing_silence_ms {
                        400 => "Fast · 400 ms silence",
                        800 => "Balanced · 800 ms silence",
                        1200 => "Patient · 1200 ms silence",
                        _ => "Custom silence delay",
                    })
                    .show_ui(ui, |ui| {
                        for (delay, label) in [
                            (400, "Fast · 400 ms"),
                            (800, "Balanced · 800 ms"),
                            (1200, "Patient · 1200 ms"),
                        ] {
                            ui.selectable_value(
                                &mut state.config.trailing_silence_ms,
                                delay,
                                label,
                            );
                        }
                    });
            },
        );
        egui::CollapsingHeader::new("Advanced · recording")
            .id_salt("microphone_recording")
            .default_open(false)
            .show(ui, |ui| {
        kit::slider_row(
            ui,
            "Silence delay",
            "Short delays can stop mid-sentence.",
            egui::Slider::new(&mut state.config.trailing_silence_ms, 200..=5000).suffix(" ms"),
        );
        kit::toggle_row(
            ui,
            "Trim silence",
            "Trim the clip before upload.",
            &mut state.config.trim_silence,
        );
        ui.small("Keeps 200 ms around detected speech.")
            .on_hover_text("Pauses inside speech are preserved. VAD can miss quiet speech; turn trimming off to upload the entire recording. Trimming does not shorten the silence needed to stop recording.");
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    state.config.enabled && !state.is_busy(),
                    egui::Button::new("Start microphone"),
                )
                .clicked()
            {
                state.start_for(character, composer_empty);
            }
            if ui
                .add_enabled(state.is_recording(), egui::Button::new("Stop + transcribe"))
                .clicked()
            {
                state.stop();
            }
            if ui
                .add_enabled(state.is_busy(), egui::Button::new("Cancel"))
                .clicked()
            {
                state.cancel();
            }
        });
        ui.label(&state.status);
        if state.is_busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    });
}
