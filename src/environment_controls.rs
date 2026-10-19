use bevy_egui::egui;
use waifu_studio::environments::{EnvironmentKind, EnvironmentSettings};
use waifu_studio::settings_kit as kit;

/// Returns true when the selection changed so the caller can persist it.
pub fn ui(ui: &mut egui::Ui, settings: &mut EnvironmentSettings) -> bool {
    let mut changed = false;
    kit::group(ui, "Environment", |ui| {
        kit::row(ui, "Environment", "", |ui| {
            egui::ComboBox::from_id_salt("environment")
                .selected_text(settings.kind.label())
                .width(220.0)
                .show_ui(ui, |ui| {
                    for kind in EnvironmentKind::ALL {
                        changed |= ui
                            .selectable_value(&mut settings.kind, kind, kind.label())
                            .changed();
                    }
                });
        });
        kit::note(
            ui,
            "Furnished 3D rooms and outdoor scenery. Hidden in the transparent desktop overlay.",
        );
    });
    changed
}
