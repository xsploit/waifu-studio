use bevy_egui::egui;
use waifu_studio::gaze::{GazeConfig, GazeState, GazeTarget};
use waifu_studio::settings_kit as kit;

/// Returns true only when the saved gaze configuration changes.
pub fn ui(ui: &mut egui::Ui, state: &mut GazeState) -> bool {
    let before = state.config.clone();
    kit::group(ui, "Procedural gaze", |ui| {
        kit::toggle_row(
            ui,
            "Enable gentle head and eye movement",
            "",
            &mut state.config.enabled,
        );
        ui.add_enabled_ui(state.config.enabled,|ui| {
        kit::row(ui, "Gaze target", "", |ui| {
egui::ComboBox::from_id_salt("gaze-target")
            .selected_text(match state.config.target { GazeTarget::Ambient => "Ambient movement", GazeTarget::Pointer => "Follow pointer in avatar view" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.config.target, GazeTarget::Ambient, "Ambient movement");
                ui.selectable_value(&mut state.config.target, GazeTarget::Pointer, "Follow pointer in avatar view");
            });
});
        kit::slider_row(ui, "Overall strength", "", egui::Slider::new(&mut state.config.intensity,0.0..=1.0));
        kit::slider_row(ui, "Head follow", "", egui::Slider::new(&mut state.config.head_follow,0.0..=1.0));
        kit::slider_row(ui, "Head drift", "", egui::Slider::new(&mut state.config.head_drift,0.0..=1.0));
        kit::slider_row(ui, "Eye motion", "", egui::Slider::new(&mut state.config.eye_motion,0.0..=1.0));
        kit::slider_row(ui, "Target smoothing (seconds)", "", egui::Slider::new(&mut state.config.smoothing_seconds,0.05..=2.5));
        kit::note(ui, "Slow head sway and occasional focus shifts; eyes make smaller independent movements. The current animation remains the base pose.");
    });
        if !state.status.is_empty() {
            ui.small(&state.status);
        }
        kit::note(
            ui,
            "Applies to the selected character. Eyes follow the model's authored VRM look-at: bone rigs use its range maps, expression rigs drive lookLeft/Right/Up/Down (manual look sliders and overrideLookAt win). Models without look-at metadata need eye bones. Pointer following uses the avatar viewport and returns to ambient movement outside it. Desktop window detection can supply pointer coordinates during click passthrough.",
        );
        if ui.button("Reset gaze settings").clicked() {
            state.config = GazeConfig::default();
        }
    });
    state.config = state.config.sanitized();
    state.config != before
}
