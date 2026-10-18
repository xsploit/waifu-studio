use bevy_egui::egui;
use waifu_studio::listen::{ListeningState, SttBackend};
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut ListeningState,
    catalog: &mut waifu_studio::model_catalog::ModelCatalog,
) {
    let mut enabled = state.config.enabled;
    if ui
        .checkbox(&mut enabled, "Enable microphone transcription")
        .changed()
    {
        state.set_enabled(enabled);
    }
    ui.label("Manual Start opens your microphone. Stop, speech followed by silence, or the 30-second limit uploads a WAV automatically; review the transcript before sending. Disable or Cancel stops recording and discards the result; an upload already sent cannot be recalled.");
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
    egui::ComboBox::from_id_salt("stt_backend")
        .selected_text(labels[selected])
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (index, label) in labels.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *label);
            }
        });
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
        ui.label("Vercel transcription is a beta service; availability depends on your team. Uses the v4 transcription protocol.");
    }
    ui.label("Transcription endpoint (localhost / LAN supported)");
    ui.text_edit_singleline(&mut state.config.endpoint);
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
    ui.horizontal(|ui| {
        ui.label("Model");
        ui.text_edit_singleline(&mut state.config.model);
    });
    ui.horizontal(|ui| {
        ui.label("API key env var");
        ui.text_edit_singleline(&mut state.config.api_key_env);
    });
    if state.config.backend != SttBackend::VercelGateway {
        ui.horizontal(|ui| {
            ui.label("Language hint (optional, e.g. en or ja)");
            ui.text_edit_singleline(&mut state.config.language);
        });
    }
    ui.label("Session API key (not saved)");
    ui.add(egui::TextEdit::singleline(&mut state.session_key).password(true));
    ui.checkbox(
        &mut state.config.silero_enabled,
        "Use Silero neural VAD (optional local ONNX)",
    );
    if state.config.silero_enabled {
        ui.label("Silero ONNX model path (current 512-frame model)");
        ui.text_edit_singleline(&mut state.config.silero_model);
        ui.label("ONNX Runtime shared library path (trusted native library)");
        ui.text_edit_singleline(&mut state.config.silero_runtime);
        ui.label("No downloads. Missing/incompatible files fall back to energy VAD. Runtime library changes may require an app restart.");
        ui.add(
            egui::Slider::new(&mut state.config.silero_threshold, 0.01..=0.99)
                .text("Speech probability"),
        );
    }
    if let Ok(status) = state.vad_status.lock() {
        ui.label(status.as_str());
    }
    ui.add(
        egui::Slider::new(&mut state.config.vad_threshold, 0.001..=0.2)
            .logarithmic(true)
            .text("Energy VAD threshold"),
    );
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
                ui.selectable_value(&mut state.config.trailing_silence_ms, delay, label);
            }
        });
    ui.small("Shorter silence starts transcription sooner, but may stop during a pause in your sentence.");
    ui.add(
        egui::Slider::new(&mut state.config.trailing_silence_ms, 200..=5000)
            .text("Trailing silence ms"),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.config.enabled && !state.is_busy(),
                egui::Button::new("Start microphone"),
            )
            .clicked()
        {
            state.start();
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
}
