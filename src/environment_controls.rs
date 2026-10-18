use bevy_egui::egui;
use waifu_studio::environments::{EnvironmentKind, EnvironmentSettings};

/// Returns true when the selection changed so the caller can persist it.
pub fn ui(ui: &mut egui::Ui, settings: &mut EnvironmentSettings) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new("ENVIRONMENT")
            .size(12.0)
            .strong()
            .weak(),
    );
    egui::ComboBox::from_id_salt("environment")
        .selected_text(settings.kind.label())
        .width(ui.available_width().min(520.0))
        .show_ui(ui, |ui| {
            for kind in EnvironmentKind::ALL {
                changed |= ui
                    .selectable_value(&mut settings.kind, kind, kind.label())
                    .changed();
            }
        });
    ui.label("Furnished 3D rooms and outdoor scenery. Hidden in the transparent desktop overlay.");
    changed
}
