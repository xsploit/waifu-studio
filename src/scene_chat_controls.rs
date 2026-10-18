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
    if !chat.partial.is_empty() {
        ui.label(&chat.partial);
    }
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
