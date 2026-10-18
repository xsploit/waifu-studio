use bevy_egui::egui;
use waifu_studio::memory::MemoryState;

pub fn ui(ui: &mut egui::Ui, state: &mut MemoryState) {
    ui.label("Explicit local notes, stored separately for this character. Nothing is extracted automatically. Relevant and pinned notes are sent with chat only when enabled.");
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
        let mut enabled = state.enabled;
        if ui
            .checkbox(&mut enabled, "Include local notes in chat")
            .changed()
            && let Err(e) = state.set_enabled(enabled)
        {
            state.error = Some(e);
        }
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
        let notes = state.notes().to_vec();
        for note in notes {
            ui.push_id(note.id, |ui| {
                ui.group(|ui| {
                    let draft_id = ui.id().with("note-edit");
                    let mut text = ui
                        .data_mut(|data| data.get_temp::<String>(draft_id))
                        .unwrap_or(note.text.clone());
                    ui.add(egui::TextEdit::multiline(&mut text));
                    ui.data_mut(|data| data.insert_temp(draft_id, text.clone()));
                    ui.horizontal(|ui| {
                        let mut pinned = note.pinned;
                        if ui.checkbox(&mut pinned, "Pinned").changed()
                            && let Err(e) = state.set_pinned(note.id, pinned)
                        {
                            state.error = Some(e);
                        }
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
                });
            });
        }
    });
}
