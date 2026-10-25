use bevy_egui::egui;
use waifu_studio::memory::{MemoryState, RecallOutcome};
use waifu_studio::settings_kit as kit;

pub fn ui(ui: &mut egui::Ui, state: &mut MemoryState) {
    ui.small("Local notes for this character.")
        .on_hover_text("Nothing is extracted automatically. Relevant and pinned notes are sent with chat only when enabled.");
    if !state.has_character() {
        ui.label("Select a character to manage memory.");
        return;
    }
    if state.busy {
        ui.label("Loading / saving local notes…");
    }
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::RED, error);
    }
    ui.add_enabled_ui(state.loaded && !state.busy, |ui| {
        ui.label("New note");
        ui.add(
            egui::TextEdit::multiline(&mut state.draft)
                .hint_text("New note (maximum 1000 characters)"),
        );
        if ui.button("Add note").clicked() {
            match state.add_note(state.draft.clone()) {
                Ok(()) => state.draft.clear(),
                Err(e) => state.error = Some(e),
            }
        }
        let mut linked_recall = state.linked_recall();
        if ui.add_enabled_ui(!state.busy, |ui| kit::toggle_row(ui, "Recall explicitly linked notes", "One hop from directly matched notes.", &mut linked_recall)).inner.changed()
            && let Err(error) = state.set_linked_recall(linked_recall) { state.error = Some(error); }
        ui.small("Related notes follow links you choose.")
            .on_hover_text("When enabled, up to two linked notes follow directly matched notes within the existing recall limits. Lexical recall only; semantic lookup is unchanged. No automatic fact inference.");
        let links: std::collections::BTreeSet<_> = state.links().iter().map(|link| (link.a, link.b)).collect();
        let notes = state.notes().to_vec();
        for note in &notes {
            ui.push_id(note.id, |ui| {
                kit::card(ui, |card| {
                    let ui = &mut *card.ui;
                    let draft_id = ui.id().with("note-edit");
                    let mut text = ui
                        .data_mut(|data| data.get_temp::<String>(draft_id))
                        .unwrap_or(note.text.clone());
                    ui.add(egui::TextEdit::multiline(&mut text));
                    ui.data_mut(|data| data.insert_temp(draft_id, text.clone()));
                    let mut pinned = note.pinned;
                        if kit::toggle_row(ui, "Pinned", "", &mut pinned).changed()
                            && let Err(e) = state.set_pinned(note.id, pinned)
                        {
                            state.error = Some(e);
                        }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                !state.busy && text != note.text,
                                egui::Button::new("Save edit"),
                            )
                            .clicked()
                            && let Err(e) = state.update_note(note.id, text)
                        {
                            state.error = Some(e);
                        }
                        if ui
                            .add_enabled(!state.busy, egui::Button::new("Delete note"))
                            .clicked()
                            && let Err(e) = state.remove_note(note.id)
                        {
                            state.error = Some(e);
                        }
                    });
                    egui::CollapsingHeader::new("Related notes").id_salt("note-links").show(ui, |ui| {
                        for other in notes.iter().filter(|other| other.id != note.id) {
                            let key = if note.id < other.id { (note.id, other.id) } else { (other.id, note.id) };
                            let mut linked = links.contains(&key);
                            let label: String = other.text.chars().take(64).collect();
                            if ui.add_enabled_ui(!state.busy, |ui| kit::toggle_row(ui, label, "", &mut linked)).inner
                                .on_hover_text(&other.text).changed()
                                && let Err(error) = state.set_link(note.id, other.id, linked) { state.error = Some(error); }
                        }
                        if notes.len() < 2 { ui.weak("Add another note to create a link."); }
                    });
                });
            });
        }
    });
    recall_inspector(ui, state);
}

/// Read-only view of the latest lexical recall; collapsed by default.
fn recall_inspector(ui: &mut egui::Ui, state: &MemoryState) {
    egui::CollapsingHeader::new("Latest recall trace")
        .id_salt("memory-recall-trace")
        .default_open(false)
        .show(ui, |ui| {
            let Some(trace) = state.latest_trace() else {
                ui.weak("No local note recall yet for this character in this session.");
                return;
            };
            ui.weak(match trace.mode {
                waifu_studio::memory::RecallMode::Lexical => "Lexical recall · focused chat",
                waifu_studio::memory::RecallMode::PinsAlongsideSemantic =>
                    "Pins only · semantic retrieval supplied the other notes",
            });
            ui.weak(format!(
                "{} query terms · {} bytes inserted · {} over budget · {} over 6-note limit · {} unmatched",
                trace.query_terms,
                trace.bytes_inserted,
                trace.over_budget,
                trace.over_limit,
                trace.unmatched
            ));
            if trace.hits.is_empty() {
                ui.weak("No pinned or matching notes.");
            }
            for hit in &trace.hits {
                let outcome = match hit.outcome {
                    RecallOutcome::Inserted => "included",
                    RecallOutcome::OverBudget => "over budget",
                    RecallOutcome::OverLimit => "over limit",
                };
                let reason = if let Some(via) = hit.linked_via {
                    format!("linked via {}", &via.to_string()[..8])
                } else if hit.pinned {
                    "pinned".to_string()
                } else {
                    format!("matched: {}", hit.matched_terms.join(", "))
                };
                ui.label(format!(
                    "{} · score {} · {reason} · {outcome}",
                    &hit.note.to_string()[..8],
                    hit.score
                ))
                .on_hover_text(hit.note.to_string());
            }
        });
}
