use bevy_egui::egui;
use waifu_studio::gaze::{GazeConfig, GazeState, GazeTarget};

/// Returns true only when the saved gaze configuration changes.
pub fn ui(ui: &mut egui::Ui, state: &mut GazeState) -> bool {
    let before = state.config.clone();
    ui.heading("Procedural gaze");
    ui.checkbox(
        &mut state.config.enabled,
        "Enable gentle head and eye movement",
    );
    ui.add_enabled_ui(state.config.enabled,|ui| {
        egui::ComboBox::from_id_salt("gaze-target")
            .selected_text(match state.config.target { GazeTarget::Ambient => "Ambient movement", GazeTarget::Pointer => "Follow pointer in avatar view" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.config.target, GazeTarget::Ambient, "Ambient movement");
                ui.selectable_value(&mut state.config.target, GazeTarget::Pointer, "Follow pointer in avatar view");
            });
        ui.add(egui::Slider::new(&mut state.config.intensity,0.0..=1.0).text("Overall strength"));
        ui.add(egui::Slider::new(&mut state.config.head_follow,0.0..=1.0).text("Head follow"));
        ui.add(egui::Slider::new(&mut state.config.head_drift,0.0..=1.0).text("Head drift"));
        ui.add(egui::Slider::new(&mut state.config.eye_motion,0.0..=1.0).text("Eye motion"));
        ui.add(egui::Slider::new(&mut state.config.smoothing_seconds,0.05..=2.5).text("Target smoothing (seconds)"));
        ui.small("Slow head sway and occasional focus shifts; eyes make smaller independent movements. The current animation remains the base pose.");
    });
    if !state.status.is_empty() {
        ui.small(&state.status);
    }
    ui.small("Applies to the selected character. Eye movement needs eye bones. Pointer following uses the avatar viewport and returns to ambient movement outside it. Desktop window detection can supply pointer coordinates during click passthrough. Expression-based eye tracking is unavailable.");
    if ui.button("Reset gaze settings").clicked() {
        state.config = GazeConfig::default();
    }
    state.config = state.config.sanitized();
    state.config != before
}
