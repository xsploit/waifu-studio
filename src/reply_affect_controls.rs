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
                AffectSource::ReplyMetadata => "Reply metadata (no extra call)",
                AffectSource::UserMessageJev => "Incoming message via decision model (extra call)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut state.config.source,
                    AffectSource::ReplyMetadata,
                    "Reply metadata (no extra call)",
                );
                ui.selectable_value(
                    &mut state.config.source,
                    AffectSource::UserMessageJev,
                    "Incoming message via decision model (extra call)",
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
            kit::note(
                ui,
                "Requests a hidden emotion block from this character's chat model. No extra provider calls.",
            );
        }
        AffectSource::UserMessageJev => {
            kit::note(
                ui,
                "One extra decision-model call per incoming message, alongside chat. Appraises the character's reaction, not your mental state. 1.5-second deadline, no retries; late or failed results do nothing.",
            );
        }
    }
    ui.add_enabled_ui(state.config.enabled, |ui| {
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
            egui::Slider::new(&mut state.config.half_life_seconds, 1.0..=3600.0).logarithmic(true),
        )
        .changed();
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
