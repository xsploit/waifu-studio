use bevy_egui::egui;
use waifu_studio::{data::Library, scene_roster::SceneRoster};
pub fn ui(ui: &mut egui::Ui, roster: &mut SceneRoster, library: &Library) {
    ui.heading("Scene companions");
    ui.small("Up to four characters including the focused character. Additional companions are visual only; chat, expressions and speech still belong to the focused character. Scene membership and placements are saved locally.");
    if let Some(error) = &roster.persistence_error {
        ui.colored_label(egui::Color32::YELLOW, format!("Scene save: {error}"));
    }
    let mut remove = None;
    for slot in &mut roster.slots {
        ui.push_id(slot.character, |ui| {
            ui.horizontal(|ui| {
                ui.label(&slot.name);
                ui.label(format!("{:?}", slot.load.status));
                if ui.small_button("Remove").clicked() {
                    remove = Some(slot.character);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Position");
                ui.add(
                    egui::DragValue::new(&mut slot.position.x)
                        .speed(0.05)
                        .range(-10.0..=10.0),
                );
                ui.add(
                    egui::DragValue::new(&mut slot.position.y)
                        .speed(0.05)
                        .range(-10.0..=10.0),
                );
                ui.add(
                    egui::DragValue::new(&mut slot.position.z)
                        .speed(0.05)
                        .range(-10.0..=10.0),
                );
                ui.label("Rotation");
                ui.add(egui::DragValue::new(&mut slot.yaw).speed(0.03));
            });
        });
    }
    if let Some(id) = remove {
        roster.remove(id);
    }
    if roster.slots.len() < 3 {
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
    }
}
