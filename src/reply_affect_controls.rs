//! Local inspector; caller persists changes using its existing store worker.
use crate::reply_affect::{AffectSource, CharacterAffect};
use crate::settings_kit as kit;
use bevy_egui::egui;

pub fn draw_affect_controls(ui: &mut egui::Ui, state: &mut CharacterAffect) -> bool {
    let mut changed = kit::toggle_row(
        ui,
        "Automatic character emotion",
        "Opt in for this character.",
        &mut state.config.enabled,
    )
    .changed();
    let previous = state.config.source;
    kit::row(ui, "Emotion source", "", |ui| {
        egui::ComboBox::from_id_salt("affect_source")
            .selected_text(match state.config.source {
                AffectSource::ReplyMetadata => "Reply metadata",
                AffectSource::UserMessageJev => "Incoming message (model)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut state.config.source,
                    AffectSource::ReplyMetadata,
                    "Reply metadata",
                );
                ui.selectable_value(
                    &mut state.config.source,
                    AffectSource::UserMessageJev,
                    "Incoming message (model)",
                );
            });
    });
    if previous != state.config.source {
        state.mood = Default::default();
        state.emotion = Default::default();
        changed = true;
    }
    match state.config.source {
        AffectSource::ReplyMetadata => {
            ui.small("Uses reply metadata; no extra call.")
                .on_hover_text("Requests a hidden emotion block from this character’s chat model.");
        }
        AffectSource::UserMessageJev => {
            ui.small("One extra model call per incoming message.")
                .on_hover_text("Appraises the character’s reaction alongside chat, not your mental state. Uses a 1.5-second deadline with no retries; late or failed results do nothing.");
        }
    }
    ui.add_enabled_ui(state.config.enabled, |ui| {
        egui::CollapsingHeader::new("Advanced · mood")
            .id_salt("affect_mood_advanced")
            .default_open(false)
            .show(ui, |ui| {
                changed |= kit::slider_row(
                    ui,
                    "Emotion influence",
                    "",
                    egui::Slider::new(&mut state.config.influence, 0.0..=1.0),
                )
                .changed();
                changed |= kit::slider_row(
                    ui,
                    "Mood half-life",
                    "Seconds until mood influence halves.",
                    egui::Slider::new(&mut state.config.half_life_seconds, 1.0..=3600.0)
                        .logarithmic(true)
                        .suffix(" s"),
                )
                .changed();
            });
    });
    ui.label(format!(
        "Mood: {:?} · V {:+.2} A {:+.2} D {:+.2}",
        state.emotion, state.mood.valence, state.mood.arousal, state.mood.dominance
    ));
    if ui.button("Reset mood to neutral").clicked() {
        state.mood = Default::default();
        state.emotion = Default::default();
        changed = true;
    }
    kit::note(
        ui,
        "Manual expression controls take priority. Mood belongs to this character and decays toward neutral.",
    );
    changed
}
