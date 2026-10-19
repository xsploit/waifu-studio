//! Memory settings: a few clear switches, the review queue, and what the companion knows.
//! The full inspectors stay available under Advanced.
use super::{Studio, decision_model_compact, settings_kit as kit};
use bevy_egui::egui::{self, RichText};
use std::collections::HashSet;
use waifu_studio::{
    facts_memory::RecordKind, memory::MemoryState, semantic_memory::SemanticMemory,
};

fn kind_label(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Fact => "Fact",
        RecordKind::Preference => "Preference",
        RecordKind::Question => "Question",
        RecordKind::Reflection => "Reflection",
    }
}

pub(super) fn ui(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    memory: &mut MemoryState,
    semantic: &mut SemanticMemory,
) {
    memory.select_character(studio.library.selected);
    let name = studio
        .library
        .selected
        .and_then(|id| studio.library.characters.iter().find(|c| c.id == id))
        .map_or("your companion".to_owned(), |c| c.name.clone());
    if !studio.facts.has_character() {
        kit::card(ui, |c| {
            c.row(
                "No companion selected",
                "Choose one under Characters.",
                |_| {},
            )
        });
        return;
    }
    let ready = studio.facts.loaded && !studio.facts.busy && memory.loaded && !memory.busy;
    for error in [studio.facts.error.clone(), memory.error.clone()]
        .into_iter()
        .flatten()
    {
        ui.colored_label(egui::Color32::from_rgb(214, 104, 112), error);
    }

    kit::section(ui, "Learning");
    ui.add_enabled_ui(ready, |ui| {
        kit::card(ui, |c| {
            let mut in_chat = studio.facts.enabled && memory.enabled;
            if c.row(
                "Use memories in chat",
                &format!("{name} brings up what matters from memory when it's relevant."),
                |ui| kit::toggle(ui, &mut in_chat),
            )
            .changed()
            {
                let result = studio
                    .facts
                    .set_enabled(in_chat)
                    .and(memory.set_enabled(in_chat));
                if let Err(e) = result {
                    studio.error = Some(e);
                }
            }
            let mut learn =
                studio.facts.candidates_enabled && studio.library.scene.memory_triage.enabled;
            if c.row(
                "Learn from chat",
                "Suggests new memories from what you say. You approve each one.",
                |ui| kit::toggle(ui, &mut learn),
            )
            .changed()
            {
                studio.library.scene.memory_triage.enabled = learn;
                studio.save();
                let facts = &mut studio.facts;
                let mut result = facts.set_candidates_enabled(learn);
                if learn && !facts.enabled {
                    result = result.and(facts.set_enabled(true));
                }
                if learn != facts.retain_pending {
                    result = result.and(facts.set_retain_pending(learn));
                }
                if let Err(e) = result {
                    studio.error = Some(e);
                }
            }
            let connection = studio.library.decision.clone();
            let mut model = studio.library.scene.memory_triage.model.clone();
            c.row(
                "Suggestion model",
                "Decides what's worth remembering.",
                |ui| {
                    decision_model_compact(
                        ui,
                        "memory_triage_model",
                        &mut studio.decision_models,
                        &connection,
                        &mut model,
                    )
                },
            );
            if model != studio.library.scene.memory_triage.model {
                studio.library.scene.memory_triage.model = model;
                studio.save();
            }
            let mut reflect = studio.library.scene.automatic_reflection.enabled;
            if c.row(
                "Reflections",
                &format!("Now and then {name} writes down what stood out. You approve it."),
                |ui| kit::toggle(ui, &mut reflect),
            )
            .changed()
            {
                studio.library.scene.automatic_reflection.enabled = reflect;
                studio.save();
            }
        });
    });
    if !studio.facts.triage_status.is_empty() {
        ui.label(RichText::new(&studio.facts.triage_status).size(12.0).weak());
    }

    let candidates = studio.facts.candidates().to_vec();
    if !candidates.is_empty() {
        kit::section(ui, &format!("Waiting for your OK ({})", candidates.len()));
        kit::card(ui, |c| {
            for candidate in &candidates {
                let mut hint = kind_label(candidate.kind).to_owned();
                if let Some(old) = candidate.supersedes
                    && let Some(r) = studio.facts.records().iter().find(|r| r.id == old)
                {
                    hint.push_str(&format!(" · replaces “{}”", r.text));
                }
                let (accept, dismiss) = c.row(&candidate.text, &hint, |ui| {
                    let dismiss = ui.button("Dismiss").clicked();
                    let accept = ui
                        .add_enabled(
                            ready,
                            egui::Button::new(
                                RichText::new("Remember").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(184, 88, 96)),
                        )
                        .clicked();
                    (accept, dismiss)
                });
                if accept && let Err(e) = studio.facts.accept_candidate(candidate.id) {
                    studio.error = Some(e);
                }
                if dismiss {
                    studio.facts.dismiss_candidate(candidate.id);
                }
            }
        });
    }

    kit::section(ui, &format!("What {name} knows"));
    let superseded: HashSet<_> = studio
        .facts
        .records()
        .iter()
        .filter_map(|r| r.supersedes)
        .collect();
    let records: Vec<_> = studio
        .facts
        .records()
        .iter()
        .filter(|r| !superseded.contains(&r.id))
        .cloned()
        .collect();
    let notes = memory.notes().to_vec();
    let confirm_id = egui::Id::new(("memory_forget_confirm", studio.library.selected));
    let mut confirm: Option<uuid::Uuid> = ui.data(|d| d.get_temp(confirm_id)).flatten();
    kit::card(ui, |c| {
        if records.is_empty() && notes.is_empty() {
            c.row(
                "Nothing yet",
                &format!("Tell {name} about yourself, or add something below."),
                |_| {},
            );
        }
        for record in &records {
            let asking = confirm == Some(record.id);
            let clicked = c.row(&record.text, kind_label(record.kind), |ui| {
                ui.add_enabled(
                    ready,
                    egui::Button::new(if asking { "Confirm" } else { "Forget" }),
                )
                .clicked()
            });
            if clicked {
                if asking {
                    confirm = None;
                    if let Err(e) = studio.facts.forget(record.id) {
                        studio.error = Some(e);
                    }
                } else {
                    confirm = Some(record.id);
                }
            }
        }
        for note in &notes {
            let hint = if note.pinned {
                "Note · always included"
            } else {
                "Note"
            };
            if c.row(&note.text, hint, |ui| {
                ui.add_enabled(ready, egui::Button::new("Delete")).clicked()
            }) && let Err(e) = memory.remove_note(note.id)
            {
                studio.error = Some(e);
            }
        }
        c.block(|ui| {
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut memory.draft)
                        .hint_text("Add something to remember…")
                        .desired_width(ui.available_width() - 64.0),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let add = ui.add_enabled(
                    ready && !memory.draft.trim().is_empty(),
                    egui::Button::new("Add"),
                );
                if (add.clicked() || enter) && !memory.draft.trim().is_empty() {
                    match memory.add_note(memory.draft.trim().to_owned()) {
                        Ok(()) => memory.draft.clear(),
                        Err(e) => studio.error = Some(e),
                    }
                }
            });
        });
    });
    ui.data_mut(|d| d.insert_temp(confirm_id, confirm));

    super::settings_pages::advanced(ui, "memory", |ui| {
        super::memory_advanced(ui, studio, memory, semantic)
    });
}
