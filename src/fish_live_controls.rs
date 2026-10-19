//! Controls for implemented hosted Fish timestamp streaming.
use bevy_egui::egui;
use waifu_studio::fish_live::{FishLiveState, LiveTransport};
use waifu_studio::settings_kit as kit;

pub fn ui(ui: &mut egui::Ui, state: &mut FishLiveState) {
    kit::group(ui, "Fish timestamp SSE", |ui| {
        kit::row(
            ui,
            "Transport",
            "Choose how timestamp audio is delivered.",
            |ui| {
                egui::ComboBox::from_id_salt("fish-live-transport")
                    .selected_text(state.transport.label())
                    .show_ui(ui, |ui| {
                        for transport in LiveTransport::ALL {
                            ui.selectable_value(&mut state.transport, transport, transport.label());
                        }
                    });
            },
        );
        ui.small(state.transport.capability());
        kit::note(
            ui,
            "Uses hosted Fish settings and the selected model. Word timestamps time captions; they do not provide visemes. Focused replies use sentence requests; the displayed timings restart at zero for each request.",
        );
        ui.label(&state.status);
        ui.small("Use Stop speech above to cancel playback and the network stream.");
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
    });
}
