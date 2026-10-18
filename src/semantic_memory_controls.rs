use bevy_egui::egui;
use uuid::Uuid;
use waifu_studio::{memory::MemoryState, semantic_memory::SemanticMemory};

pub fn ui(
    ui: &mut egui::Ui,
    state: &mut SemanticMemory,
    character: Option<Uuid>,
    memory: &MemoryState,
) {
    ui.separator();
    ui.heading("Semantic lookup (optional)");
    ui.label("Finds related explicit notes with an OpenAI-compatible /embeddings endpoint. Saved notes are indexed in the background after opt-in. Use Find notes for current draft to send that draft as a lookup query, then Add retrieved notes to place matching results in the composer. Chat does not add new memories automatically.");
    if !state.settings_loaded() {
        ui.label("Loading settings…");
        return;
    }
    if !memory.enabled {
        ui.label("Turn on \"Include local notes in chat\" above to use semantic lookup.");
    }
    let s = &mut state.settings;
    ui.checkbox(&mut s.enabled, "Use semantic lookup");
    egui::Grid::new("semantic-memory-settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("API base URL");
            ui.text_edit_singleline(&mut s.base_url);
            ui.end_row();
            ui.label("Embedding model");
            ui.text_edit_singleline(&mut s.model);
            ui.end_row();
            ui.label("Key variable (optional)");
            ui.add(egui::TextEdit::singleline(&mut s.key_env).hint_text("e.g. EMBEDDING_API_KEY"));
            ui.end_row();
            ui.label("Results");
            ui.add(egui::Slider::new(&mut s.top_k, 1..=8));
            ui.end_row();
            ui.label("Minimum similarity");
            ui.add(egui::Slider::new(&mut s.min_score, -1.0..=1.0));
            ui.end_row();
        });
    ui.checkbox(&mut s.allow_remote, "Allow non-local endpoint");
    if s.allow_remote {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Notes and looked-up drafts will be sent to this server, which may charge per request.",
        );
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.settings_dirty(),
                egui::Button::new("Save lookup settings"),
            )
            .clicked()
            && let Err(e) = state.save_settings()
        {
            state.error = Some(e);
        }
        if ui.button("Retry now").clicked() {
            state.retry();
        }
        if let Some(id) = character
            && ui
                .add_enabled(!state.busy(), egui::Button::new("Clear vector cache"))
                .clicked()
        {
            state.forget(id);
        }
    });
    let ready = state.ready(character, memory);
    ui.label(if ready {
        format!("✔ {}", state.status)
    } else {
        state.status.clone()
    });
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::RED, error);
    }
}
