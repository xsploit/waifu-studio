//! Controls for implemented hosted Fish timestamp streaming.
use bevy_egui::egui;
use waifu_studio::fish_live::{FishLiveState, LiveTransport};

pub fn ui(ui: &mut egui::Ui, state: &mut FishLiveState) {
    ui.separator();
    ui.label(egui::RichText::new("Fish live speech").strong());
    egui::ComboBox::from_id_salt("fish-live-transport")
        .selected_text(state.transport.label())
        .show_ui(ui, |ui| {
            for transport in LiveTransport::ALL {
                ui.selectable_value(&mut state.transport, transport, transport.label());
            }
        });
    ui.small(state.transport.capability());
    ui.small("Uses hosted Fish settings and the selected model. Word timestamps time captions; they do not provide visemes.");
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
}
