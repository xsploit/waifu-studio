//! Local inspector; caller persists changes using its existing store worker.
use crate::reply_affect::{AffectSource, CharacterAffect};
use bevy_egui::egui;

pub fn draw_affect_controls(ui: &mut egui::Ui, state: &mut CharacterAffect) -> bool {
    let mut changed = ui
        .checkbox(
            &mut state.config.enabled,
            "Automatic character emotion (opt in)",
        )
        .changed();
    let previous = state.config.source;
    egui::ComboBox::from_id_salt("affect_source")
        .selected_text(match state.config.source {
            AffectSource::ReplyMetadata => "Reply metadata (no extra call)",
            AffectSource::UserMessageJev => "Incoming message via Jev (extra call)",
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
                "Incoming message via Jev (extra call)",
            );
        });
    if previous != state.config.source {
        state.mood = Default::default();
        state.emotion = Default::default();
        changed = true;
    }
    match state.config.source {
        AffectSource::ReplyMetadata => {
            ui.label("Requests a hidden emotion block from this character's chat model. No extra provider calls.");
        }
        AffectSource::UserMessageJev => {
            ui.label("One optional Jev call per incoming message, alongside chat. Uses the Scene chat decision connection. Appraises the character's reaction, not the user's mental state. Discrete VAD levels, 1.5-second deadline, no retries; unavailable or stale results do nothing.");
        }
    }
    ui.add_enabled_ui(state.config.enabled, |ui| {
        changed |= ui
            .add(
                egui::Slider::new(&mut state.config.influence, 0.0..=1.0).text("Emotion influence"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut state.config.half_life_seconds, 1.0..=3600.0)
                    .logarithmic(true)
                    .text("Mood half-life (seconds)"),
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
    ui.small("Manual expression controls take priority. Mood belongs to this character and decays toward neutral.");
    changed
}
