//! Live WebSocket controls; character delivery and Stop speech are owned by Studio.
use bevy_egui::egui;
use waifu_studio::fish_ws::FishWsState;

pub fn ui(ui: &mut egui::Ui, state: &mut FishWsState) {
    ui.separator();
    ui.label(egui::RichText::new("Fish live WebSocket").strong());
    ui.small(if state.timestamps {
        "MessagePack text input and progressive WAV with word timings."
    } else {
        "MessagePack text input and progressive WAV audio."
    });
    ui.checkbox(&mut state.auto_flush, "Flush after sentence endings");
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
    ui.small("Use Stop speech to cancel. HTTP CONNECT proxies are supported; HTTPS/SOCKS proxy routes are rejected explicitly.");
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
                    ui.small(format!("{:.2}–{:.2}s  {}", word.start, word.end, word.text));
                }
                if state.words.len() > 100 {
                    ui.small("Showing the first 100 words.");
                }
            });
    }
}
