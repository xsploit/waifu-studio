//! Compact chest-physics presets with uncommon joint mapping folded away.
use bevy_egui::egui;
use waifu_studio::{
    settings_kit as kit,
    spring_physics::{JointSelection, SpringPhysicsConfig, SpringPhysicsState},
};

pub fn ui(ui: &mut egui::Ui, config: &mut SpringPhysicsConfig, state: &SpringPhysicsState) -> bool {
    let before = config.clone();
    kit::section(ui, "Physics");
    kit::card(ui, |c| {
        let label = if !config.enabled || config.strength == 0.0 {
            "Model settings".to_owned()
        } else if (config.strength - 0.65).abs() < 0.001 {
            "Expressive".to_owned()
        } else if config.strength == 1.0 {
            "Maximum jiggle".to_owned()
        } else {
            format!("Custom · {:.0}%", config.strength * 100.0)
        };
        c.row(
            "Chest motion",
            "Tunes chest springs while keeping the model’s gravity and collisions.",
            |ui| {
                egui::ComboBox::from_id_salt("chest_physics_preset")
                    .width(180.0)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (name, strength) in [
                            ("Model settings", 0.0),
                            ("Expressive", 0.65),
                            ("Maximum jiggle", 1.0),
                        ] {
                            if ui
                                .selectable_label(
                                    config.enabled && config.strength == strength,
                                    name,
                                )
                                .clicked()
                            {
                                config.enabled = true;
                                config.strength = strength;
                            }
                        }
                    });
            },
        );
        let selected = state
            .available_joints
            .iter()
            .filter(|joint| joint.selected)
            .count();
        let status = if state.available_joints.is_empty() {
            "Chest physics is unavailable or the model is still loading.".to_owned()
        } else if selected == 0 {
            "Choose this model’s chest joints in Advanced.".to_owned()
        } else if config.enabled && config.strength > 0.0 {
            format!("Uses {selected} chest joint(s); hair and clothing keep their own physics.")
        } else {
            "Using the model’s original physics.".to_owned()
        };
        kit::note(c.ui, &status);
        egui::CollapsingHeader::new("Advanced · physics")
            .id_salt("spring_physics_advanced")
            .default_open(false)
            .show(c.ui, |ui| {
                if kit::slider_row(
                    ui,
                    "Jiggle strength",
                    "Model settings at 0%; maximum tuning at 100%.",
                    egui::Slider::new(&mut config.strength, 0.0..=1.0)
                        .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                )
                .changed()
                {
                    config.enabled = true;
                }
                kit::note(ui, &state.status);
                kit::row(
                    ui,
                    "Chest joints",
                    "Choose manually if the model uses unusual names.",
                    |ui| {
                        egui::ComboBox::from_id_salt("chest_joint_selection")
                            .width(180.0)
                            .selected_text(match config.selection {
                                JointSelection::Detected => "Detected joints",
                                JointSelection::Manual => "Choose joints",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut config.selection,
                                    JointSelection::Detected,
                                    "Detected joints",
                                );
                                ui.selectable_value(
                                    &mut config.selection,
                                    JointSelection::Manual,
                                    "Choose joints",
                                );
                            });
                    },
                );
                if config.selection == JointSelection::Manual {
                    kit::row(ui, "Add joint", "Eligible joints from this model.", |ui| {
                        egui::ComboBox::from_id_salt("add_chest_joint")
                            .width(220.0)
                            .selected_text("Add a joint…")
                            .height(200.0)
                            .show_ui(ui, |ui| {
                                let mut offered = false;
                                for joint in state.available_joints.iter().filter(|joint| {
                                    joint.blocked_reason.is_none()
                                        && !config.manual_joints.contains(&joint.key)
                                }) {
                                    offered = true;
                                    if ui
                                        .selectable_label(
                                            false,
                                            format!(
                                                "{} · {}:{}",
                                                joint.name,
                                                joint.chain_index + 1,
                                                joint.joint_index + 1
                                            ),
                                        )
                                        .clicked()
                                    {
                                        config.manual_joints.insert(joint.key.clone());
                                        break;
                                    }
                                }
                                if !offered {
                                    ui.label("No additional eligible joints");
                                }
                            });
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("selected_chest_joints")
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for key in config.manual_joints.clone() {
                                let name = state
                                    .available_joints
                                    .iter()
                                    .find(|joint| joint.key == key)
                                    .map_or_else(
                                        || format!("Missing joint · {key}"),
                                        |joint| joint.name.clone(),
                                    );
                                kit::row(ui, &name, "", |ui| {
                                    if ui.small_button("Remove").clicked() {
                                        config.manual_joints.remove(&key);
                                    }
                                });
                            }
                        });
                }
                if ui.button("Use model physics").clicked() {
                    config.enabled = true;
                    config.strength = 0.0;
                }
            });
    });
    config.sanitize();
    *config != before
}
