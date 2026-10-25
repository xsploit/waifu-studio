use bevy_egui::egui;
use waifu_studio::gaze::{GazeConfig, GazeState, GazeTarget};
use waifu_studio::settings_kit as kit;

/// Returns true only when the saved gaze configuration changes.
pub fn ui(ui: &mut egui::Ui, state: &mut GazeState) -> bool {
    let before = state.config.clone();
    kit::group(ui, "Procedural gaze", |ui| {
        kit::toggle_row(
            ui,
            "Head & eye movement",
            "Gentle motion over the current animation.",
            &mut state.config.enabled,
        );
        ui.add_enabled_ui(state.config.enabled,|ui| {
        kit::row(ui, "Gaze target", "", |ui| {
egui::ComboBox::from_id_salt("gaze-target")
            .selected_text(match state.config.target { GazeTarget::Ambient => "Ambient", GazeTarget::Pointer => "Follow pointer" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.config.target, GazeTarget::Ambient, "Ambient");
                ui.selectable_value(&mut state.config.target, GazeTarget::Pointer, "Follow pointer");
            });
});
        kit::slider_row(ui, "Overall strength", "", egui::Slider::new(&mut state.config.intensity,0.0..=1.0));
        egui::CollapsingHeader::new("Advanced · gaze")
            .id_salt("gaze_advanced")
            .default_open(false)
            .show(ui, |ui| {
        kit::slider_row(ui, "Head follow", "", egui::Slider::new(&mut state.config.head_follow,0.0..=1.0));
        kit::slider_row(ui, "Head drift", "", egui::Slider::new(&mut state.config.head_drift,0.0..=1.0));
        kit::slider_row(ui, "Eye motion", "", egui::Slider::new(&mut state.config.eye_motion,0.0..=1.0));
        kit::slider_row(ui, "Target smoothing", "", egui::Slider::new(&mut state.config.smoothing_seconds,0.05..=2.5).suffix(" s"));
        kit::note(ui, "Slow head sway and occasional focus shifts; eyes make smaller independent movements. The current animation remains the base pose.");
        });
    });
        if !state.status.is_empty() {
            ui.small(&state.status);
        }
        ui.small("Pointer following applies inside the avatar view.")
            .on_hover_text("Outside the avatar view, gaze returns to ambient movement. Uses the model’s VRM look-at range maps or expressions; manual look controls take priority. Models without look-at metadata need eye bones. Desktop detection can provide pointer coordinates during click passthrough. Applies to the selected character only.");
        if ui.button("Reset gaze settings").clicked() {
            state.config = GazeConfig::default();
        }
    });
    state.config = state.config.sanitized();
    state.config != before
}
