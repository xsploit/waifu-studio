//! Read-only Fish voice library picker. Returns a reference ID only on explicit selection.
use bevy_egui::egui;
use waifu_studio::{
    voice::VoiceConfig,
    voice_library::{MAX_PAGE_NUMBER, MAX_PAGE_SIZE, SortBy, VoiceLibraryState},
};

/// Returns the newly chosen reference ID; root writes it into `VoiceConfig::reference_id`.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut VoiceLibraryState,
    config: &VoiceConfig,
    session_key: &str,
) -> Option<String> {
    state.poll();
    ui.label(egui::RichText::new("Fish voice library").strong());
    ui.small("Read-only search. Uses the session key or the configured key environment variable; nothing is saved.");
    let busy = state.is_busy();
    let mut search = false;
    ui.add_enabled_ui(!busy, |ui| {
        egui::ComboBox::from_id_salt("voice-library-scope")
            .selected_text(if state.query.own_only {
                "My voices"
            } else {
                "Public voices"
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.query.own_only, false, "Public voices");
                ui.selectable_value(&mut state.query.own_only, true, "My voices");
            });
        ui.horizontal(|ui| {
            ui.label("Title");
            search |= ui.text_edit_singleline(&mut state.query.title).lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
        });
        ui.horizontal(|ui| {
            ui.label("Tag");
            ui.add(egui::TextEdit::singleline(&mut state.query.tag).desired_width(80.0));
            ui.label("Language");
            ui.add(egui::TextEdit::singleline(&mut state.query.language).desired_width(48.0));
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("voice-library-sort")
                .selected_text(state.query.sort_by.label())
                .show_ui(ui, |ui| {
                    for sort in SortBy::ALL {
                        ui.selectable_value(&mut state.query.sort_by, sort, sort.label());
                    }
                });
            ui.add(
                egui::Slider::new(&mut state.query.page_size, 1..=MAX_PAGE_SIZE).text("Per page"),
            );
        });
        ui.horizontal(|ui| {
            if ui.button("Search").clicked() {
                state.query.page_number = 1;
                search = true;
            }
            let page = state.query.page_number;
            if ui.add_enabled(page > 1, egui::Button::new("◀")).clicked() {
                state.query.page_number = page - 1;
                search = true;
            }
            ui.label(format!("Page {page}"));
            let more = u64::from(page) * u64::from(state.query.page_size) < state.page.total;
            if ui
                .add_enabled(more && page < MAX_PAGE_NUMBER, egui::Button::new("▶"))
                .clicked()
            {
                state.query.page_number = page + 1;
                search = true;
            }
        });
    });
    if search && let Err(error) = state.fetch(config, Some(session_key)) {
        state.status = error;
    }
    if busy {
        if ui.button("Cancel search").clicked() {
            state.cancel();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.label(&state.status);
    let current = config.reference_id.trim();
    let mut chosen = None;
    let selected_title = state
        .page
        .voices
        .iter()
        .find(|voice| voice.id == current)
        .map(|voice| voice.title.as_str())
        .unwrap_or(if current.is_empty() {
            "Choose a voice…"
        } else {
            current
        });
    egui::ComboBox::from_id_salt("voice-library-selection")
        .selected_text(selected_title)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("voice-library-results")
                .max_height(220.0)
                .show(ui, |ui| {
                    for voice in &state.page.voices {
                        ui.horizontal(|ui| {
                            let label = if voice.author.is_empty() {
                                voice.title.clone()
                            } else {
                                format!("{} · {}", voice.title, voice.author)
                            };
                            let response = ui.selectable_label(voice.id == current, label);
                            let details = [
                                voice.description.clone(),
                                voice.languages.join(", "),
                                voice.tags.join(", "),
                                format!("{} · {}", voice.visibility, voice.id),
                            ]
                            .into_iter()
                            .filter(|line| !line.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n");
                            if response.on_hover_text(details).clicked() {
                                chosen = Some(voice.id.clone());
                                ui.close();
                            }
                        });
                    }
                });
        });
    if let Some(id) = chosen {
        state.select(&id);
    }
    state.take_selected()
}
