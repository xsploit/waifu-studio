use bevy_egui::egui;
use waifu_studio::facts_memory::{
    CANDIDATE_TTL_SECS, FactsMemoryState, FactsRecallOutcome, MAX_CANDIDATES, RecordKind,
    reflection::{MAX_CITED, MAX_SOURCE_CHARS, ReflectionStatus},
};

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
        pending_retention(ui, state);
        reflection_drafts(ui, state);
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
                if candidate.kind == RecordKind::Reflection {
                    reflection_editor(ui, state, &candidate);
                    ui.label(format!("Cited accepted records: {}", candidate.evidence));
                } else {
                    ui.label(format!("Quoted evidence: {}", candidate.evidence));
                }
                ui.weak(format!(
                    "Turn: {:?} · pending expiry: {:?} · not used in chat until accepted",
                    candidate.turn_id, candidate.expires_at
                ));
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
                            .on_hover_text("Hides this record from recall now. The next ledger save permanently removes fully expired correction chains; use Clear expiry before that save to keep it.")
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
    recall_inspector(ui, state);
}

/// Separate opt-in for keeping pending candidates in candidates.jsonl across restarts.
fn pending_retention(ui: &mut egui::Ui, state: &mut FactsMemoryState) {
    let mut retain = state.retain_pending;
    if ui
        .checkbox(
            &mut retain,
            "Keep pending candidates across restarts (opt in)",
        )
        .changed()
        && let Err(e) = state.set_retain_pending(retain)
    {
        state.error = Some(e);
    }
    ui.weak(format!(
        "Stores only each pending candidate's quoted user turn (up to 1000 characters), turn ID \
         and expiry ({} days) in candidates.jsonl beside this character's ledger — never full \
         transcripts. At most {} candidates. Pending candidates never reach chat until accepted. \
         Turning this off deletes that file; current suggestions remain until restart.",
        CANDIDATE_TTL_SECS / 86_400,
        MAX_CANDIDATES
    ));
    if state.pending_saving() {
        ui.weak("Saving pending candidates…");
    }
    let elsewhere = state.failed_pending_elsewhere();
    if elsewhere > 0 {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!("Saving pending candidates failed for {elsewhere} other character(s); select them to retry."),
        );
    }
    if let Some(e) = state.pending_error.clone() {
        ui.colored_label(
            egui::Color32::RED,
            format!("Pending candidate file unavailable; nothing is written to it: {e}"),
        );
        if state.pending_write_failed() {
            ui.weak("The suggestions shown were not saved. Retry once, or stop retaining them.");
            if ui.button("Retry saving pending candidates").clicked()
                && let Err(e) = state.retry_pending()
            {
                state.error = Some(e);
            }
        }
        if ui
            .button("Remove pending candidate file")
            .on_hover_text("Deletes candidates.jsonl only; the accepted ledger is untouched.")
            .clicked()
            && let Err(e) = state.set_retain_pending(false)
        {
            state.error = Some(e);
        }
    }
}

/// Read-only view of the latest evidence recall; collapsed by default.
fn recall_inspector(ui: &mut egui::Ui, state: &FactsMemoryState) {
    egui::CollapsingHeader::new("Latest evidence recall trace")
        .id_salt("facts-recall-trace")
        .default_open(false)
        .show(ui, |ui| {
            let Some(trace) = state.latest_trace() else {
                ui.weak("No evidence recall yet for this character in this session.");
                return;
            };
            ui.weak(format!(
                "{} query terms · {} bytes inserted · {} over budget · {} over 6-record limit",
                trace.query_terms, trace.bytes_inserted, trace.over_budget, trace.over_limit
            ));
            ui.weak(format!(
                "Excluded: {} superseded by corrections · {} expired · {} unmatched",
                trace.superseded, trace.expired, trace.unmatched
            ));
            if trace.hits.is_empty() {
                ui.weak("No matching records.");
            }
            for hit in &trace.hits {
                let outcome = match hit.outcome {
                    FactsRecallOutcome::Inserted => "sent",
                    FactsRecallOutcome::OverBudget => "over budget",
                    FactsRecallOutcome::OverLimit => "over limit",
                };
                ui.label(format!(
                    "{:?} {} · score {} · matched: {} · {outcome}",
                    hit.kind,
                    &hit.record.to_string()[..8],
                    hit.score,
                    hit.matched_terms.join(", ")
                ))
                .on_hover_text(hit.record.to_string());
            }
        });
}

/// Manual reflection drafts: shows exactly which accepted records would be sent, needs a
/// per-character confirmation, and only ever produces a pending candidate.
fn reflection_drafts(ui: &mut egui::Ui, state: &mut FactsMemoryState) {
    egui::CollapsingHeader::new("Reflection draft (manual, opt in)")
        .id_salt("facts-reflection")
        .default_open(false)
        .show(ui, |ui| {
            let sources = state.reflection_sources();
            let config = &mut state.reflection.config;
            ui.weak(format!(
                "Sends up to {} current accepted facts, preferences and questions (first {MAX_SOURCE_CHARS} \
                 characters each, never transcripts, pending suggestions or earlier reflections) to the \
                 endpoint below. The reply must cite at most {MAX_CITED} offered records; anything else is \
                 rejected. The result is a pending reflection you review, edit, accept or dismiss.",
                sources.len()
            ));
            ui.horizontal(|ui| {
                ui.label("Chat completions URL");
                ui.text_edit_singleline(&mut config.endpoint);
            });
            ui.horizontal(|ui| {
                ui.label("Model");
                ui.text_edit_singleline(&mut config.model);
            });
            ui.horizontal(|ui| {
                ui.label("Key variable");
                ui.text_edit_singleline(&mut config.key_env);
            });
            ui.horizontal(|ui| {
                ui.label("Session key");
                ui.add(egui::TextEdit::singleline(&mut config.session_key).password(true))
                    .on_hover_text("Kept in memory for this session only.");
            });
            egui::CollapsingHeader::new(format!("Records that would be sent ({})", sources.len()))
                .id_salt("facts-reflection-sources")
                .show(ui, |ui| {
                    for source in &sources {
                        let text: String = source.text.chars().take(MAX_SOURCE_CHARS).collect();
                        ui.label(format!("{:?} {} · {text}", source.kind, &source.id.to_string()[..8]))
                            .on_hover_text(source.id.to_string());
                    }
                });
            let confirm_id = ui
                .id()
                .with(("reflection-consent", state.selected_character()));
            let mut confirm = ui
                .data_mut(|d| d.get_temp::<bool>(confirm_id))
                .unwrap_or(false);
            ui.checkbox(
                &mut confirm,
                "I confirm sending these records to this endpoint for one draft",
            );
            ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
            let running = state.reflection.running();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        confirm && !running && !state.reflection.shutting_down() && !sources.is_empty(),
                        egui::Button::new("Draft reflection"),
                    )
                    .clicked()
                {
                    match state.start_reflection() {
                        // One confirmation per request.
                        Ok(()) => ui.data_mut(|d| {
 d.insert_temp(confirm_id, false);
 }),
                        Err(e) => state.error = Some(e),
                    }
                }
                if ui.add_enabled(running, egui::Button::new("Cancel")).clicked() {
                    state.reflection.cancel();
                }
            });
            let status = match state.reflection.status() {
                ReflectionStatus::Idle => "No reflection requested yet.".into(),
                ReflectionStatus::Running => "Drafting…".into(),
                ReflectionStatus::Proposed(_) => "Draft added below as a pending reflection.".into(),
                ReflectionStatus::NoReflection => "The records did not support a reflection.".into(),
                ReflectionStatus::Cancelled => "Cancelled; any late reply is discarded.".into(),
                ReflectionStatus::Stale(e) => format!("Discarded: {e}"),
                ReflectionStatus::Failed(e) => format!("Failed: {e}"),
            };
            ui.weak(status);
        });
}

/// Rewording keeps the code-attached citations; the draft stays pending until accepted.
fn reflection_editor(
    ui: &mut egui::Ui,
    state: &mut FactsMemoryState,
    candidate: &waifu_studio::facts_memory::MemoryRecord,
) {
    let text_id = ui.id().with(("reflection-edit", candidate.id));
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(text_id))
        .unwrap_or(candidate.text.clone());
    ui.add(egui::TextEdit::multiline(&mut text));
    ui.data_mut(|d| d.insert_temp(text_id, text.clone()));
    if ui
        .add_enabled(
            text != candidate.text,
            egui::Button::new("Save reflection edit"),
        )
        .clicked()
        && let Err(e) = state.edit_reflection_candidate(candidate.id, &text)
    {
        state.error = Some(e);
    }
}
