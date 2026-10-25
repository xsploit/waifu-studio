use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::{data::Library, scene_chat::SceneChat};
pub fn ui(ui: &mut egui::Ui, chat: &mut SceneChat, library: &Library) {
    egui::CollapsingHeader::new("Advanced · scene emotions").id_salt("scene_emotions").default_open(false).show(ui, |ui| {
        kit::note(ui, "Uses each recipient’s saved automatic-emotion opt-in. Local behavior is required for facial output. Reply metadata costs no extra call; Jev appraisal adds a separate bounded request. Neither guarantees speech-onset synchronization.");
        for receipt in chat.affect_receipts() {
            let name = library.characters.iter().find(|c| c.id == receipt.character).map(|c| c.name.as_str()).unwrap_or("Removed character");
            if let Some(error) = &receipt.error { ui.colored_label(egui::Color32::LIGHT_RED, format!("{name}: {error}")); }
            else if let Some(state) = chat.affect.entries.get(&receipt.character) { ui.label(format!("{name}: {:?} · V {:+.2} A {:+.2} D {:+.2}", state.emotion, state.mood.valence, state.mood.arousal, state.mood.dominance)); }
        }
    });
    egui::CollapsingHeader::new("Advanced · scene recall")
        .id_salt("scene_recall")
        .default_open(false)
        .show(ui, |ui| {
            kit::note(ui, "Memory prepared for the latest scene requests. This does not confirm provider delivery or a successful reply. Traces stay in this session; focused-chat inspectors are separate.");
            if chat.recalls().is_empty() {
                ui.weak("No scene recall prepared yet.");
            }
            for recall in chat.recalls() {
                ui.push_id(recall.character, |ui| {
                    egui::CollapsingHeader::new(&recall.name).show(ui, |ui| {
                        ui.strong("Local notes");
                        if let Some(trace) = &recall.notes {
                            ui.small(format!("{} terms · {} bytes · {} unmatched · {} over budget · {} over limit",
                                trace.query_terms, trace.bytes_inserted, trace.unmatched, trace.over_budget, trace.over_limit));
                            for hit in &trace.hits {
                                ui.label(format!("{} · score {} · {} · {:?}",
                                    &hit.note.to_string()[..8], hit.score,
                                    if let Some(via) = hit.linked_via { format!("linked via {}", &via.to_string()[..8]) } else if hit.pinned { "pinned".into() } else { hit.matched_terms.join(", ") },
                                    hit.outcome)).on_hover_text(hit.note.to_string());
                            }
                        } else { ui.weak("Notes disabled or unavailable."); }
                        ui.strong("Evidence ledger");
                        if let Some(trace) = &recall.facts {
                            ui.small(format!("{} terms · {} bytes · {} unmatched · {} corrected · {} expired · {} stale reflections · {} over budget · {} over limit",
                                trace.query_terms, trace.bytes_inserted, trace.unmatched, trace.superseded,
                                trace.expired, trace.stale_reflections, trace.over_budget, trace.over_limit));
                            for hit in &trace.hits {
                                ui.label(format!("{} · {:?} · score {} · {} · {:?}",
                                    &hit.record.to_string()[..8], hit.kind, hit.score,
                                    hit.matched_terms.join(", "), hit.outcome)).on_hover_text(hit.record.to_string());
                            }
                        } else { ui.weak("Evidence recall disabled or unavailable."); }
                    });
                });
            }
        });
}

pub fn transcript(ui: &mut egui::Ui, chat: &SceneChat) {
    if chat.transcript.is_empty() {
        return;
    }
    kit::section(ui, "Conversation");
    egui::ScrollArea::vertical()
        .id_salt("scene_transcript")
        .max_height(260.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &chat.transcript {
                ui.strong(&line.name);
                ui.label(&line.text);
                ui.add_space(6.0);
            }
        });
}
