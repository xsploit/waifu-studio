use bevy_egui::egui;
use waifu_studio::facts_memory::{FactsMemoryState, RecordKind};

pub fn ui(ui: &mut egui::Ui, state: &mut FactsMemoryState) {
    ui.label("Evidence ledger: facts, preferences, questions and reflections are local to this character. Candidate extraction uses only user turns and always requires acceptance.");
    if !state.has_character() {
        ui.label("Select a character to manage the ledger.");
        return;
    }
    if state.busy {
        ui.label("Loading / saving evidence ledger…");
    }
    if let Some(e) = &state.error {
        ui.colored_label(egui::Color32::RED, e);
    }
    ui.add_enabled_ui(state.loaded && !state.busy, |ui| {
        let mut enabled = state.enabled;
        if ui
            .checkbox(&mut enabled, "Include relevant evidence in chat (opt in)")
            .changed()
            && let Err(e) = state.set_enabled(enabled)
        {
            state.error = Some(e);
        }
        let mut candidates = state.candidates_enabled;
        if ui
            .checkbox(
                &mut candidates,
                "Suggest candidates from ‘remember’, ‘I like’, ‘I dislike’ user turns",
            )
            .changed()
            && let Err(e) = state.set_candidates_enabled(candidates)
        {
            state.error = Some(e);
        }
        ui.add(
            egui::TextEdit::multiline(&mut state.draft)
                .hint_text("Record text (up to 1000 characters)"),
        );
        ui.add(
            egui::TextEdit::multiline(&mut state.evidence_draft)
                .hint_text("Quoted evidence / source excerpt (required)"),
        );
        ui.horizontal(|ui| {
            for kind in [
                RecordKind::Fact,
                RecordKind::Preference,
                RecordKind::Question,
                RecordKind::Reflection,
            ] {
                if ui
                    .add_enabled(!state.busy, egui::Button::new(format!("Add {kind:?}")))
                    .clicked()
                {
                    match state.add_record(kind, state.draft.clone(), state.evidence_draft.clone())
                    {
                        Ok(()) => {
                            state.draft.clear();
                            state.evidence_draft.clear();
                        }
                        Err(e) => state.error = Some(e),
                    }
                }
            }
        });
        for candidate in state.candidates().to_vec() {
            ui.group(|ui| {
                ui.label(format!(
                    "Candidate {:?}: {}",
                    candidate.kind, candidate.text
                ));
                ui.label(format!("Quoted evidence: {}", candidate.evidence));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!state.busy, egui::Button::new("Accept candidate"))
                        .clicked()
                        && let Err(e) = state.accept_candidate(candidate.id)
                    {
                        state.error = Some(e);
                    }
                    if ui.button("Dismiss").clicked() {
                        state.dismiss_candidate(candidate.id);
                    }
                });
            });
        }
        let character_scope = ui.id().with("evidence-records");
        for record in state.records().to_vec() {
            ui.push_id((character_scope, record.id), |ui| {
                ui.group(|ui| {
                    ui.label(format!("{:?} · {}", record.kind, record.id));
                    ui.label(format!(
                        "Source: {} · turn: {:?}",
                        record.source, record.turn_id
                    ));
                    ui.label(format!(
                        "Created: {} · expires: {:?} · supersedes: {:?}",
                        record.created_at, record.expires_at, record.supersedes
                    ));
                    let text_id = ui.id().with("text");
                    let evidence_id = ui.id().with("evidence");
                    let mut text = ui
                        .data_mut(|d| d.get_temp::<String>(text_id))
                        .unwrap_or(record.text.clone());
                    let mut evidence = ui
                        .data_mut(|d| d.get_temp::<String>(evidence_id))
                        .unwrap_or(record.evidence.clone());
                    ui.add(egui::TextEdit::multiline(&mut text));
                    ui.label("Quoted evidence");
                    ui.add(egui::TextEdit::multiline(&mut evidence));
                    ui.data_mut(|d| {
                        d.insert_temp(text_id, text.clone());
                        d.insert_temp(evidence_id, evidence.clone());
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !state.busy && (text != record.text || evidence != record.evidence),
                                egui::Button::new("Save correction"),
                            )
                            .clicked()
                            && let Err(e) = state.correct_record(record.id, text, evidence)
                        {
                            state.error = Some(e);
                        }
                        if ui
                            .add_enabled(!state.busy, egui::Button::new("Expire now"))
                            .clicked()
                            && let Err(e) = state.set_expiry(record.id, Some(0))
                        {
                            state.error = Some(e);
                        }
                        if ui
                            .add_enabled(
                                !state.busy && record.expires_at.is_some(),
                                egui::Button::new("Clear expiry"),
                            )
                            .clicked()
                            && let Err(e) = state.set_expiry(record.id, None)
                        {
                            state.error = Some(e);
                        }
                        if ui
                            .add_enabled(!state.busy, egui::Button::new("Forget permanently"))
                            .clicked()
                            && let Err(e) = state.forget(record.id)
                        {
                            state.error = Some(e);
                        }
                    });
                });
            });
        }
        let confirm_id = ui
            .id()
            .with(("confirm-ledger-delete", state.selected_character()));
        let mut confirm = ui
            .data_mut(|d| d.get_temp::<bool>(confirm_id))
            .unwrap_or(false);
        ui.checkbox(
            &mut confirm,
            "Confirm deletion of this character’s entire evidence ledger",
        );
        ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
        if ui
            .add_enabled(confirm && !state.busy, egui::Button::new("Delete ledger"))
            .clicked()
        {
            match state.delete_character_data() {
                Ok(()) => {
                    ui.data_mut(|d| d.insert_temp(confirm_id, false));
                }
                Err(e) => state.error = Some(e),
            }
        }
    });
}
