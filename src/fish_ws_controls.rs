//! Live WebSocket controls; character delivery and Stop speech are owned by Studio.
use bevy_egui::egui;
use waifu_studio::fish_ws::FishWsState;
use waifu_studio::settings_kit as kit;

pub fn ui(ui: &mut egui::Ui, state: &mut FishWsState) {
    kit::group(ui, "Fish live WebSocket", |ui| {
        ui.small(if state.timestamps {
            "MessagePack text input and progressive WAV with word timings."
        } else {
            "MessagePack text input and progressive WAV audio."
        });
        kit::toggle_row(
            ui,
            "Flush each sentence",
            "Send buffered text at sentence endings.",
            &mut state.auto_flush,
        );
        if ui
            .add_enabled(
                state.active_id().is_some(),
                egui::Button::new("Flush text now"),
            )
            .clicked()
            && let Err(error) = state.flush()
        {
            state.status = error;
        }
        ui.label(&state.status);
        ui.small("Use Stop speech to cancel.")
            .on_hover_text("HTTP CONNECT proxies are supported. HTTPS and SOCKS proxy routes are rejected explicitly.");
        egui::CollapsingHeader::new("Advanced · stream details")
            .id_salt("fish_ws_details")
            .default_open(false)
            .show(ui, |ui| {
                if let Some(stats) = &state.last_stats {
                    ui.small(format!(
                        "{} audio frames · {} KiB received",
                        stats.frames,
                        stats.audio_bytes / 1024
                    ));
                }
                if !state.words.is_empty() {
                    ui.label(format!("{} aligned words", state.words.len()));
                    egui::ScrollArea::vertical()
                        .max_height(100.0)
                        .show(ui, |ui| {
                            for word in state.words.iter().take(100) {
                                ui.small(format!(
                                    "{:.2}–{:.2}s  {}",
                                    word.start, word.end, word.text
                                ));
                            }
                            if state.words.len() > 100 {
                                ui.small("Showing the first 100 words.");
                            }
                        });
                }
            });
    });
}
