use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::{data::Library, scene_roster::SceneRoster};
pub fn ui(ui: &mut egui::Ui, roster: &mut SceneRoster, library: &Library) {
    kit::section(ui, "Scene companions");
    ui.small("Add up to three companions beside the focused character.")
        .on_hover_text("Scene membership and placements are saved locally. Use Talk to the scene to choose recipients for a scene conversation.");
    if let Some(error) = &roster.persistence_error {
        ui.colored_label(egui::Color32::YELLOW, format!("Scene save: {error}"));
    }
    let mut remove = None;
    for slot in &mut roster.slots {
        ui.push_id(slot.character, |ui| {
            kit::row(ui, &slot.name, &format!("{:?}", slot.load.status), |ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(slot.character);
                }
            });
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
    if let Some(id) = remove {
        roster.remove(id);
    }
    if roster.slots.len() < 3 {
        kit::row(
            ui,
            "Add companion",
            "Up to three additional scene characters.",
            |ui| {
                egui::ComboBox::from_id_salt("add_scene_companion")
                    .selected_text("Add character…")
                    .show_ui(ui, |ui| {
                        for character in &library.characters {
                            if Some(character.id) != library.selected
                                && !roster.contains(character.id)
                                && ui.selectable_label(false, &character.name).clicked()
                            {
                                let _ = roster.add(
                                    character.id,
                                    character.name.clone(),
                                    character.model.clone(),
                                );
                            }
                        }
                    });
            },
        );
    }
}
