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
    kit::group(ui, "Microphone transcription", |ui| {
        let mut enabled = state.config.enabled;
        if kit::toggle_row(
            ui,
            "Enable microphone transcription",
            "Start recording manually below.",
            &mut enabled,
        )
        .changed()
        {
            state.set_enabled(enabled);
        }
        kit::note(
            ui,
            "Manual Start opens your microphone. Stop, speech followed by silence, or the 30-second limit uploads a WAV automatically; the default is to review the transcript before sending. Disable or Cancel stops recording and discards the result; an upload already sent cannot be recalled.",
        );
        kit::toggle_row(
            ui,
            "Send transcript automatically (optional)",
            "",
            &mut state.config.auto_send,
        );
        kit::note(
            ui,
            "Applies only to recordings started with an empty draft. Sends to the same selected character if chat and memory are ready; otherwise keeps the text for review. This may trigger paid chat and speech requests. Start is still manual; the microphone never restarts itself.",
        );
        let openrouter = state.config.backend == SttBackend::OpenAiCompatible
            && state.config.endpoint.contains("openrouter.ai/");
        let mut selected = if openrouter {
            3
        } else {
            match state.config.backend {
                SttBackend::OpenAiCompatible => 0,
                SttBackend::FishAsr => 1,
                SttBackend::VercelGateway => 2,
            }
        };
        let previous = selected;
        let labels = [
            "OpenAI / local compatible",
            "Fish ASR",
            "Vercel AI Gateway",
            "OpenRouter",
        ];
        kit::row(
            ui,
            "Transcription service",
            "Choose the microphone transcription backend.",
            |ui| {
                egui::ComboBox::from_id_salt("stt_backend")
                    .selected_text(labels[selected])
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (index, label) in labels.iter().enumerate() {
                            ui.selectable_value(&mut selected, index, *label);
                        }
                    });
            },
        );
        if previous != selected {
            state.cancel();
            match selected {
                1 => state.config.preset(SttBackend::FishAsr),
                2 => state.config.preset(SttBackend::VercelGateway),
                3 => state.config.openrouter_preset(),
                _ => state.config.preset(SttBackend::OpenAiCompatible),
            }
        }
        let backend = state.config.backend;
        if backend == SttBackend::VercelGateway {
            kit::note(
                ui,
                "Vercel transcription is a beta service; availability depends on your team. Uses the v4 transcription protocol.",
            );
        }
        kit::text_row(
            ui,
            "Transcription endpoint",
            "Localhost and LAN endpoints are supported.",
            &mut state.config.endpoint,
            "Endpoint URL",
        );
        catalog.sync(
            &state.config.endpoint,
            &state.config.api_key_env,
            &state.session_key,
            waifu_studio::model_catalog::CatalogKind::Transcription,
        );
        crate::ui::speech_model_picker(ui, "stt_models", catalog, &mut state.config.model);
        if ui.small_button("Refresh transcription models").clicked() {
            catalog.retry(
                &state.config.endpoint,
                &state.config.api_key_env,
                &state.session_key,
                waifu_studio::model_catalog::CatalogKind::Transcription,
            );
        }
        kit::text_row(
            ui,
            "Model",
            "Transcription model ID.",
            &mut state.config.model,
            "",
        );
        kit::text_row(
            ui,
            "API key env var",
            "Environment variable containing the key.",
            &mut state.config.api_key_env,
            "",
        );
        if state.config.backend != SttBackend::VercelGateway {
            kit::text_row(
                ui,
                "Language hint (optional, e.g. en or ja)",
                "Optional language code, such as en or ja.",
                &mut state.config.language,
                "",
            );
        }
        kit::secret_row(ui, "Session API key", "Not saved.", &mut state.session_key);
        kit::toggle_row(
            ui,
            "Use Silero neural VAD (optional local ONNX)",
            "",
            &mut state.config.silero_enabled,
        );
        if state.config.silero_enabled {
            kit::text_row(
                ui,
                "Silero ONNX model path (current 512-frame model)",
                "Current 512-frame ONNX model.",
                &mut state.config.silero_model,
                "File path",
            );
            kit::text_row(
                ui,
                "ONNX Runtime shared library path (trusted native library)",
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
        kit::note(
            ui,
            "Shorter silence starts transcription sooner, but may stop during a pause in your sentence.",
        );
        kit::slider_row(
            ui,
            "Trailing silence ms",
            "",
            egui::Slider::new(&mut state.config.trailing_silence_ms, 200..=5000),
        );
        kit::toggle_row(
            ui,
            "Trim detected silence before upload",
            "",
            &mut state.config.trim_silence,
        );
        kit::note(
            ui,
            "Optional: keep 200 ms before and after detected speech, preserving pauses inside it. VAD can miss quiet speech; leave this off to upload the entire recording. It does not shorten the silence needed to stop recording.",
        );
        ui.horizontal(|ui| {
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
