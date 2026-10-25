use bevy_egui::egui;
use waifu_studio::routines::{
    DAYS, MAX_ANCHORS, MAX_LABEL_CHARS, RoutineAnchor, RoutineConfig, local_clock,
};
use waifu_studio::settings_kit as kit;

pub fn ui(ui: &mut egui::Ui, config: &mut RoutineConfig) -> bool {
    let before = config.clone();
    kit::toggle_row(
        ui,
        "Weekly routine",
        "Fictional character activities.",
        &mut config.enabled,
    );
    ui.small("Uses this computer’s local time.")
        .on_hover_text("Each imagined activity lasts until the next anchor. These are not bookings or measured presence.");
    ui.add_enabled_ui(config.enabled, |ui| {
        kit::toggle_row(
            ui,
            "Share activity with chat",
            "Include the current and next fictional activity.",
            &mut config.share_with_chat,
        );
    });
    ui.small("Uses ordinary replies; no extra model calls.")
        .on_hover_text("Sharing adds the current and next imagined activity to ordinary replies. No background messages are sent.");
    if let Some((day, minute)) = local_clock() {
        ui.label(format!(
            "Local clock: {} {:02}:{:02}",
            DAYS[usize::from(day)],
            minute / 60,
            minute % 60
        ));
        if let Some(view) = config.at(day, minute) {
            ui.label(format!("Imagined now: {}", view.current.activity));
            ui.label(format!(
                "Next: {} — {} {:02}:{:02} (in {} h {} min)",
                view.next.activity,
                DAYS[usize::from(view.next.day)],
                view.next.minute / 60,
                view.next.minute % 60,
                view.minutes_until_next / 60,
                view.minutes_until_next % 60
            ));
        }
    } else {
        ui.small("Local clock is unavailable on this platform.");
    }
    if let Err(error) = config.validate() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let mut remove = None;
    for (index, anchor) in config.anchors.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            kit::row(ui, "Day", "", |ui| {
                egui::ComboBox::from_id_salt("weekday")
                    .selected_text(
                        DAYS.get(usize::from(anchor.day))
                            .copied()
                            .unwrap_or("Invalid day"),
                    )
                    .show_ui(ui, |ui| {
                        for (day, label) in DAYS.iter().enumerate() {
                            ui.selectable_value(&mut anchor.day, day as u8, *label);
                        }
                    });
            });
            kit::row(ui, "Time", "Local 24-hour time.", |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut hour = anchor.minute / 60;
                    let mut minute = anchor.minute % 60;
                    let changed = ui
                        .add(egui::DragValue::new(&mut hour).range(0..=23).prefix("H "))
                        .changed()
                        | ui.add(egui::DragValue::new(&mut minute).range(0..=59).prefix("M "))
                            .changed();
                    if changed {
                        anchor.minute = hour * 60 + minute;
                    }
                });
            });
            kit::row(ui, "Activity", "Fictional activity at this time.", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut anchor.activity)
                            .char_limit(MAX_LABEL_CHARS)
                            .hint_text("Fictional activity")
                            .desired_width(150.0),
                    );
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
            });
        });
    }
    if let Some(index) = remove {
        config.anchors.remove(index);
    }
    if ui
        .add_enabled(
            config.anchors.len() < MAX_ANCHORS,
            egui::Button::new("Add activity"),
        )
        .clicked()
    {
        let free = (0..7 * 1440)
            .find(|slot| {
                !config
                    .anchors
                    .iter()
                    .any(|a| u32::from(a.day) * 1440 + u32::from(a.minute) == *slot)
            })
            .unwrap_or(0);
        config.anchors.push(RoutineAnchor {
            day: (free / 1440) as u8,
            minute: (free % 1440) as u16,
            activity: "Free time".into(),
        });
    }
    // The panel is only drawn on the Scene page. Repaint once per minute so its
    // local clock advances while idle; no worker or catch-up queue is needed.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(60));
    *config != before
}
