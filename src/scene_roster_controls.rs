use bevy_egui::egui;
use waifu_studio::scene_roster::SceneRoster;
use waifu_studio::settings_kit as kit;
pub fn ui(ui: &mut egui::Ui, roster: &mut SceneRoster) {
    kit::section(ui, "Companion placement");
    if let Some(error) = &roster.persistence_error {
        ui.colored_label(egui::Color32::YELLOW, format!("Scene save: {error}"));
    }
    for slot in &mut roster.slots {
        ui.push_id(slot.character, |ui| {
            ui.strong(&slot.name);
            egui::CollapsingHeader::new("Advanced · placement")
                .id_salt("scene_placement")
                .default_open(false)
                .show(ui, |ui| {
                    kit::row(ui, "Position", "Scene coordinates.", |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut slot.position.x)
                                    .speed(0.05)
                                    .range(-10.0..=10.0)
                                    .prefix("X "),
                            );
                            ui.add(
                                egui::DragValue::new(&mut slot.position.y)
                                    .speed(0.05)
                                    .range(-10.0..=10.0)
                                    .prefix("Y "),
                            );
                            ui.add(
                                egui::DragValue::new(&mut slot.position.z)
                                    .speed(0.05)
                                    .range(-10.0..=10.0)
                                    .prefix("Z "),
                            );
                        });
                    });
                    kit::row(ui, "Yaw", "", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut slot.yaw)
                                .speed(0.03)
                                .suffix(" rad"),
                        );
                    });
                });
        });
    }
}
