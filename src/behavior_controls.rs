use bevy_egui::egui;
use uuid::Uuid;
use waifu_studio::behavior::{BehaviorConfig, BehaviorEvent, BehaviorState, reserved_expression};
use waifu_studio::settings_kit as kit;

/// Returns true only when persistent per-character configuration changes.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut BehaviorState,
    config: &mut BehaviorConfig,
    character: Option<Uuid>,
    available: &[String],
) -> bool {
    let before = config.clone();
    kit::group(ui, "Local behavior", |ui| {
        kit::note(
            ui,
            "Optional local Idle / Thinking / Speaking controller. Emotion previews use an explicitly selected expression and decay over time. Automatic character emotion can separately use reply metadata or an opted-in Jev request; configure its source in the emotion controls.",
        );
        kit::toggle_row(
            ui,
            "Enable local behavior for this character",
            "",
            &mut config.enabled,
        );
        ui.label(format!(
            "Current state: {}",
            state.controller.phase().label()
        ));
        if character.is_none() {
            ui.label("Select a character to use behavior.");
        }
        ui.add_enabled_ui(config.enabled && character.is_some(), |ui| {
        kit::group(ui, "Lifecycle expression mappings", |ui| {
            expression_picker(ui, "behavior_idle", "Idle", &mut config.idle_expression, available);
            expression_picker(ui, "behavior_thinking", "Thinking", &mut config.thinking_expression, available);
            expression_picker(ui, "behavior_speaking", "Speaking", &mut config.speaking_expression, available);
            kit::slider_row(ui, "Lifecycle strength", "", egui::Slider::new(&mut config.lifecycle_strength, 0.0..=0.35));
            kit::note(ui, "Empty mappings only track state. Manual expression controls take priority; blink, look and mouth remain with their existing controllers.");
        });
        kit::slider_row(ui, "Emotion strength limit", "", egui::Slider::new(&mut config.emotion_max, 0.0..=0.65));
        kit::slider_row(ui, "Smoothing seconds", "", egui::Slider::new(&mut config.smoothing_seconds, 0.02..=5.0));
        kit::slider_row(ui, "Emotion decay seconds", "", egui::Slider::new(&mut config.decay_seconds, 0.1..=60.0));
        ui.separator();
        expression_picker(ui, "behavior_preview", "Preview expression", &mut state.preview_expression, available);
        kit::slider_row(ui, "Preview strength", "", egui::Slider::new(&mut state.preview_strength, 0.0..=1.0));
        if ui.add_enabled(!state.preview_expression.is_empty(), egui::Button::new("Preview explicit emotion")).clicked() {
            let result = state.controller.select(character, config, available)
                .and_then(|()| state.controller.token().ok_or_else(|| "Behavior is disabled".to_string()))
                .and_then(|token| state.controller.emotion(token, &state.preview_expression, state.preview_strength));
            state.error = result.err();
        }
        if ui.button("Interrupt behavior activity").clicked() && let Some(token) = state.controller.token() {
            state.error = state.controller.event(token, BehaviorEvent::Interrupted).err();
        }
    });
        if ui.button("Reset behavior output").clicked() {
            state.controller.reset();
            state.error = None;
        }
    });
    if config != &before {
        state.error = state.controller.select(character, config, available).err();
    }
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::RED, error);
    }
    config != &before
}
fn expression_picker(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    selected: &mut String,
    available: &[String],
) {
    kit::row(ui, label, "", |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(if selected.is_empty() {
                "None"
            } else {
                selected.as_str()
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(selected, String::new(), "None");
                for name in available.iter().filter(|name| !reserved_expression(name)) {
                    ui.selectable_value(selected, name.clone(), name);
                }
            });
    });
}
