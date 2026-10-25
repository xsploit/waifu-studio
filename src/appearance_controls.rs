use bevy_egui::egui;
use waifu_studio::appearance::AppearanceState;
use waifu_studio::settings_kit as kit;

/// Returns true when persistent character settings changed.
pub fn ui(ui: &mut egui::Ui, state: &mut AppearanceState) -> bool {
    let before = state.config.clone();
    kit::section(ui, "Avatar parts");
    ui.label(&state.status);
    ui.small("Show or hide this model’s mesh parts.")
        .on_hover_text("Only whole mesh parts can be hidden. Clothing baked into the body mesh cannot be separated here.");
    if state.parts.is_empty() {
        if !state.config.hidden_parts.is_empty() {
            ui.label(format!(
                "{} saved hidden parts apply when a matching model loads.",
                state.config.hidden_parts.len()
            ));
        }
        if !state.config.hidden_parts.is_empty() && ui.button("Reset saved choices").clicked() {
            state.config.reset();
        }
        return state.config != before;
    }
    kit::text_row(
        ui,
        "Search parts",
        "Filter model meshes and materials",
        &mut state.filter,
        "Part name…",
    );
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                state.hidden_count() > 0,
                egui::Button::new("Show all parts"),
            )
            .clicked()
        {
            let keys: Vec<_> = state.parts.iter().map(|p| p.key.clone()).collect();
            for key in keys {
                state.config.set_hidden(&key, false);
            }
        }
        let missing = state.missing_saved();
        if missing > 0
            && ui
                .button(format!("Forget missing ({missing})"))
                .on_hover_text("Remove saved visibility choices for parts absent from this model.")
                .clicked()
        {
            state.forget_missing();
        }
    });
    let needle = state.filter.trim().to_lowercase();
    kit::group(ui, "Model parts", |ui| {
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .id_salt("appearance_parts")
            .show(ui, |ui| {
                let mut shown = 0;
                for part in &state.parts {
                    let text = match &part.material {
                        Some(material) => format!("{} · {material}", part.label),
                        None => part.label.clone(),
                    };
                    if !needle.is_empty()
                        && !text.to_lowercase().contains(&needle)
                        && !part.key.to_lowercase().contains(&needle)
                    {
                        continue;
                    }
                    shown += 1;
                    if part.hidden_by_model {
                        ui.add_enabled_ui(false, |ui| {
                            kit::toggle_row(ui, text, "Hidden by the model itself", &mut false)
                        })
                        .inner
                        .on_disabled_hover_text("Hidden by the model itself");
                        continue;
                    }
                    let mut visible = !state.config.is_hidden(&part.key);
                    if kit::toggle_row(ui, text, "", &mut visible)
                        .on_hover_text(&part.key)
                        .changed()
                    {
                        state.config.set_hidden(&part.key, !visible);
                    }
                }
                if shown == 0 {
                    ui.label("No parts match.");
                }
            });
    });
    state.config != before
}
