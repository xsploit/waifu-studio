use bevy_egui::egui;
use uuid::Uuid;
use waifu_studio::settings_kit as kit;
use waifu_studio::{memory::MemoryState, semantic_memory::SemanticMemory};

pub fn ui(
    ui: &mut egui::Ui,
    state: &mut SemanticMemory,
    character: Option<Uuid>,
    memory: &MemoryState,
    facts: &waifu_studio::facts_memory::FactsMemoryState,
) {
    ui.small("Vercel embeddings recall related notes and accepted evidence when you send.")
        .on_hover_text("Selected sources are indexed in the background. Automatic lookup sends your message as a query and attaches matching memories to that request. Pending suggestions still need review before recall.");
    if !state.settings_loaded() {
        ui.label("Loading settings…");
        return;
    }
    if (state.settings.scope == waifu_studio::semantic_memory::SemanticScope::Notes
        && !memory.enabled)
        || (state.settings.scope == waifu_studio::semantic_memory::SemanticScope::Evidence
            && !facts.enabled)
    {
        ui.label("Turn on the selected source above to use semantic lookup.");
    }
    let s = &mut state.settings;
    kit::toggle_row(ui, "Use semantic lookup", "", &mut s.enabled);
    kit::toggle_row(
        ui,
        "Look up memories when sending",
        "Wait briefly for matching memories before the reply starts.",
        &mut s.automatic_lookup,
    );
    kit::row(
        ui,
        "Memory source",
        "Save settings to apply a source change.",
        |ui| {
            egui::ComboBox::from_id_salt("semantic-source")
                .selected_text(match s.scope {
                    waifu_studio::semantic_memory::SemanticScope::Notes => "Explicit notes",
                    waifu_studio::semantic_memory::SemanticScope::Evidence => "Accepted evidence",
                    waifu_studio::semantic_memory::SemanticScope::All => {
                        "Notes + accepted evidence"
                    }
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut s.scope,
                        waifu_studio::semantic_memory::SemanticScope::All,
                        "Notes + accepted evidence",
                    );
                    ui.selectable_value(
                        &mut s.scope,
                        waifu_studio::semantic_memory::SemanticScope::Notes,
                        "Explicit notes",
                    );
                    ui.selectable_value(
                        &mut s.scope,
                        waifu_studio::semantic_memory::SemanticScope::Evidence,
                        "Accepted evidence",
                    );
                });
        },
    );
    ui.small("Evidence lookup uses accepted records only.")
        .on_hover_text("Sends current accepted record text, type, and quoted evidence to the embedding endpoint. Pending candidates and stale, expired, or corrected records are excluded.");
    kit::text_row(
        ui,
        "API base URL",
        "OpenAI-compatible embeddings endpoint.",
        &mut s.base_url,
        "",
    );
    kit::text_row(ui, "Embedding model", "", &mut s.model, "");
    kit::text_row(
        ui,
        "Key variable",
        "Optional environment variable name.",
        &mut s.key_env,
        "e.g. EMBEDDING_API_KEY",
    );
    egui::CollapsingHeader::new("Advanced · matching")
        .id_salt("semantic_matching_advanced")
        .default_open(false)
        .show(ui, |ui| {
            kit::slider_row(
                ui,
                "Results",
                "Maximum matching entries.",
                egui::Slider::new(&mut s.top_k, 1..=8),
            );
            kit::slider_row(
                ui,
                "Minimum similarity",
                "",
                egui::Slider::new(&mut s.min_score, -1.0..=1.0),
            );
        });
    kit::toggle_row(ui, "Allow non-local endpoint", "", &mut s.allow_remote);
    if s.allow_remote {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Selected memory entries and looked-up drafts will be sent to this server, which may charge per request.",
        );
    }
    ui.horizontal_wrapped(|ui| {
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
    let source = waifu_studio::semantic_memory::MemorySources {
        scope: state.scope(),
        memory,
        facts,
    };
    let ready = state.ready(character, &source);
    ui.label(if ready {
        format!("✔ {}", state.status)
    } else {
        state.status.clone()
    });
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::RED, error);
    }
}
