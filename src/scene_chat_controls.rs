use bevy_egui::egui;
use waifu_studio::{data::Library, scene_chat::SceneChat, scene_roster::SceneRoster};
pub fn ui(
    ui: &mut egui::Ui,
    chat: &mut SceneChat,
    library: &Library,
    roster: &SceneRoster,
    decision: &waifu_studio::recipient_decision::DecisionConfig,
) {
    ui.heading("Talk to the scene");
    ui.small("Each checked character replies in order. Scene conversations are saved separately per character; private chat histories and other characters' notes are never shared.");
    ui.small("Checkboxes override @names. With no checkboxes, exact @names route locally; a sole scene character is selected automatically. Jev runs only when you click its button.");
    let busy = chat.is_busy();
    ui.add_enabled_ui(!busy, |ui| {
        for c in &library.characters {
            if library.selected == Some(c.id) || roster.contains(c.id) {
                let mut checked = chat.recipients.contains(&c.id);
                let provider = if library.selected == Some(c.id) {
                    library.provider
                } else {
                    c.provider.unwrap_or(library.provider)
                };
                let model = library
                    .providers
                    .get(provider)
                    .map(|p| p.effective_model())
                    .unwrap_or_else(|| "Missing connection".into());
                if ui
                    .checkbox(&mut checked, format!("{} · {}", c.name, model))
                    .changed()
                {
                    if checked {
                        chat.recipients.push(c.id);
                    } else {
                        chat.recipients.retain(|id| *id != c.id);
                    }
                }
            }
        }
        ui.add(
            egui::TextEdit::multiline(&mut chat.draft)
                .desired_rows(3)
                .hint_text("Message the checked characters…"),
        );
        if chat.recipients.is_empty()
            && ui.button("Ask Jev to choose recipient").clicked()
            && let Err(error) = chat.route_draft(decision, library, roster)
        {
            chat.status = error;
        }
        if ui.button("Send to recipients").clicked()
            && let Err(error) = chat.submit(library, roster)
        {
            chat.status = error;
        }
    });
    if busy && ui.button("Cancel scene replies").clicked() {
        chat.cancel();
    }
    ui.label(&chat.status);
    egui::CollapsingHeader::new("Latest scene emotions").show(ui, |ui| {
        ui.small("Uses each recipient’s saved automatic-emotion opt-in. Local behavior is required for facial output. Reply metadata costs no extra call; Jev appraisal adds a separate bounded request. Neither guarantees speech-onset synchronization.");
        for receipt in chat.affect_receipts() {
            let name = library.characters.iter().find(|c| c.id == receipt.character).map(|c| c.name.as_str()).unwrap_or("Removed character");
            if let Some(error) = &receipt.error { ui.colored_label(egui::Color32::LIGHT_RED, format!("{name}: {error}")); }
            else if let Some(state) = chat.affect.entries.get(&receipt.character) { ui.label(format!("{name}: {:?} · V {:+.2} A {:+.2} D {:+.2}", state.emotion, state.mood.valence, state.mood.arousal, state.mood.dominance)); }
        }
    });
    if !chat.partial.is_empty() {
        ui.label(&chat.partial);
    }
    egui::CollapsingHeader::new("Latest scene recall")
        .id_salt("scene_recall")
        .show(ui, |ui| {
            ui.small("Memory prepared for the latest scene requests. This does not confirm provider delivery or a successful reply. Traces stay in this session; focused-chat inspectors are separate.");
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
                            ui.small(format!("{} terms · {} bytes · {} unmatched · {} corrected · {} expired · {} over budget · {} over limit",
                                trace.query_terms, trace.bytes_inserted, trace.unmatched, trace.superseded,
                                trace.expired, trace.over_budget, trace.over_limit));
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
