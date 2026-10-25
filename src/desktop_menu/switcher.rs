//! Quick saved combinations, using existing character identity and persistence.
use crate::{
    studio::{Studio, Tab, ViewMode},
    ui::personality_catalog,
};
use bevy_egui::egui;
use std::path::{Path, PathBuf};
use waifu_studio::data::Library;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Page {
    #[default]
    Profiles,
    Avatar,
    Personality,
}
#[derive(Default)]
pub(super) struct Switcher {
    pub open: bool,
    page: Page,
    search: String,
}
struct ModelChoice {
    path: PathBuf,
    name: String,
}
struct PersonaChoice {
    prompt: String,
    name: String,
    description: String,
}
impl Switcher {
    pub fn show(&mut self, page: Page) {
        self.open = true;
        self.page = page;
        self.search.clear();
    }
    pub fn close(&mut self) {
        self.open = false;
    }
    pub fn ui(&mut self, ctx: &egui::Context, studio: &mut Studio) -> bool {
        if !self.open {
            return false;
        }
        let mut open = true;
        let mut chosen_profile = None;
        let mut chosen_model = None;
        let mut chosen_persona = None;
        let mut touched = false;
        let mut edit = false;
        let width = (ctx.content_rect().width() - 32.0).clamp(140.0, 352.0);
        egui::Window::new("Companion profiles")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .vscroll(true)
            .max_height((ctx.content_rect().height() - 56.0).max(80.0))
            .default_height(360.0)
            .default_width(width)
            .max_width(width)
            .constrain_to(ctx.content_rect())
            .show(ctx, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                if let Some(current) = studio.library.selected_character() {
                    ui.label(egui::RichText::new(&current.name).strong());
                    if ctx.content_rect().height() >= 400.0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                model_name(&current.model),
                                persona_name(&current.persona)
                            ))
                            .small()
                            .weak(),
                        );
                    }
                }
                if ctx.content_rect().width() < 300.0 {
                    egui::ComboBox::from_id_salt("desktop_profile_page")
                        .selected_text(match self.page {
                            Page::Profiles => "Profiles",
                            Page::Avatar => "Avatar",
                            Page::Personality => "Personality",
                        })
                        .width(width - 16.0)
                        .show_ui(ui, |ui| {
                            for (page, label) in [
                                (Page::Profiles, "Profiles"),
                                (Page::Avatar, "Avatar"),
                                (Page::Personality, "Personality"),
                            ] {
                                if ui.selectable_value(&mut self.page, page, label).clicked() {
                                    self.search.clear();
                                    ui.close();
                                }
                            }
                        });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for (page, label) in [
                            (Page::Profiles, "Profiles"),
                            (Page::Avatar, "Avatar"),
                            (Page::Personality, "Personality"),
                        ] {
                            if ui.selectable_value(&mut self.page, page, label).clicked() {
                                self.search.clear();
                            }
                        }
                    });
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search…")
                        .desired_width(width),
                );
                ui.add_space(4.0);
                let search = self.search.trim().to_lowercase();
                let mut count = 0;
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 200.0).clamp(70.0, 280.0))
                    .show(ui, |ui| {
                        match self.page {
                            Page::Profiles => {
                                for c in &studio.library.characters {
                                    let detail = format!(
                                        "{} · {}",
                                        model_name(&c.model),
                                        persona_name(&c.persona)
                                    );
                                    if !format!("{} {detail}", c.name)
                                        .to_lowercase()
                                        .contains(&search)
                                    {
                                        continue;
                                    }
                                    count += 1;
                                    if profile_row(
                                        ui,
                                        c,
                                        &detail,
                                        Some(c.id) == studio.library.selected,
                                    ) {
                                        chosen_profile = Some(c.id);
                                    }
                                }
                            }
                            Page::Avatar => {
                                for model in models(&studio.library) {
                                    if !model.name.to_lowercase().contains(&search) {
                                        continue;
                                    }
                                    count += 1;
                                    let selected = studio
                                        .library
                                        .selected_character()
                                        .is_some_and(|c| c.model == model.path);
                                    if row(ui, &model.name, "Keep this personality", selected) {
                                        chosen_model = Some(model);
                                    }
                                }
                            }
                            Page::Personality => {
                                for persona in personas(&studio.library) {
                                    if !format!("{} {}", persona.name, persona.description)
                                        .to_lowercase()
                                        .contains(&search)
                                    {
                                        continue;
                                    }
                                    count += 1;
                                    let selected = studio
                                        .library
                                        .selected_character()
                                        .is_some_and(|c| c.persona == persona.prompt);
                                    if row(ui, &persona.name, &persona.description, selected) {
                                        chosen_persona = Some(persona);
                                    }
                                }
                            }
                        }
                        if count == 0 {
                            ui.label("No matches. Try another search.");
                        }
                    });
                ui.separator();
                if self.page != Page::Profiles {
                    ui.label(
                        egui::RichText::new("New combinations are saved as separate profiles.")
                            .small()
                            .weak(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Import avatar…").clicked() {
                        studio.open_models();
                        touched = true;
                    }
                    if ui.small_button("Edit profile…").clicked() {
                        edit = true;
                    }
                });
            });
        if let Some(id) = chosen_profile {
            studio.select_character(id);
            self.open = false;
            touched = true;
        }
        if let Some(model) = chosen_model
            && let Some(current) = studio.library.selected_character()
        {
            let prompt = current.persona.clone();
            let name = format!("{} · {}", model.name, persona_name(&prompt));
            studio.use_character_combination(model.path, prompt, name);
            self.open = false;
            touched = true;
        }
        if let Some(persona) = chosen_persona
            && let Some(current) = studio.library.selected_character()
        {
            let model = current.model.clone();
            let name = format!("{} · {}", model_name(&model), persona.name);
            studio.use_character_combination(model, persona.prompt, name);
            self.open = false;
            touched = true;
        }
        if edit {
            studio.tab = Tab::Characters;
            studio.mode = ViewMode::Studio;
            self.open = false;
            touched = true;
        }
        self.open &= open;
        touched
    }
}
fn profile_row(
    ui: &mut egui::Ui,
    c: &waifu_studio::data::Character,
    detail: &str,
    selected: bool,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            6,
            if selected {
                egui::Color32::from_rgb(63, 52, 82)
            } else {
                egui::Color32::from_rgb(38, 39, 48)
            },
        );
    }
    crate::profile_library::thumbnail(
        ui,
        &c.model,
        &c.name,
        egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(44.0, 44.0)),
    );
    crate::profile_library::text(
        ui,
        rect.min + egui::vec2(56.0, 9.0),
        &c.name,
        rect.width() - 64.0,
        12.0,
        egui::Color32::from_gray(235),
    );
    crate::profile_library::text(
        ui,
        rect.min + egui::vec2(56.0, 28.0),
        detail,
        rect.width() - 64.0,
        10.0,
        egui::Color32::from_gray(158),
    );
    response.on_hover_text(&c.name).clicked()
}
fn row(ui: &mut egui::Ui, name: &str, detail: &str, selected: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            6,
            if selected {
                egui::Color32::from_rgb(63, 52, 82)
            } else {
                egui::Color32::from_rgb(38, 39, 48)
            },
        );
    }
    crate::profile_library::text(
        ui,
        rect.min + egui::vec2(8.0, 5.0),
        name,
        rect.width() - 16.0,
        12.0,
        egui::Color32::from_gray(235),
    );
    crate::profile_library::text(
        ui,
        rect.min + egui::vec2(8.0, 24.0),
        detail,
        rect.width() - 16.0,
        10.0,
        egui::Color32::from_gray(158),
    );
    response
        .on_hover_text(format!("{name}\n{detail}"))
        .clicked()
}
fn model_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Avatar");
    let stem = match stem.split_once('-') {
        Some((hash, name)) if hash.len() == 12 && hash.chars().all(|c| c.is_ascii_hexdigit()) => {
            name
        }
        _ => stem,
    };
    stem.replace('_', " ")
}
fn persona_name(prompt: &str) -> String {
    personality_catalog::by_prompt(prompt).map_or_else(
        || {
            if prompt.trim().is_empty() {
                "Default".into()
            } else {
                "Custom".into()
            }
        },
        |p| p.name.into(),
    )
}
fn models(library: &Library) -> Vec<ModelChoice> {
    let mut rows = Vec::new();
    for c in &library.characters {
        if !rows.iter().any(|m: &ModelChoice| m.path == c.model) {
            rows.push(ModelChoice {
                path: c.model.clone(),
                name: model_name(&c.model),
            });
        }
    }
    rows.sort_by_key(|m| m.name.to_lowercase());
    rows
}
fn personas(library: &Library) -> Vec<PersonaChoice> {
    let mut rows: Vec<_> = personality_catalog::ORIGINALS
        .iter()
        .map(|p| PersonaChoice {
            prompt: p.system_prompt.into(),
            name: p.name.into(),
            description: p.description.into(),
        })
        .collect();
    for c in &library.characters {
        if !rows.iter().any(|p| p.prompt == c.persona) {
            rows.push(PersonaChoice {
                prompt: c.persona.clone(),
                name: format!("{} · {}", c.name, persona_name(&c.persona)),
                description: "Saved in your library".into(),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use waifu_studio::data::{Character, Store};
    #[test]
    fn switcher_pages_fit_desktop_and_narrow_windows() {
        let directory =
            std::env::temp_dir().join(format!("profile-switcher-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let bundle: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/characters/bundle.json")).unwrap();
        for item in bundle["characters"].as_array().unwrap().iter().take(3) {
            let mut c: Character = serde_json::from_value(item.clone()).unwrap();
            c.model = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/characters")
                .join(c.model);
            library.characters.push(c);
        }
        library.selected = Some(library.characters[0].id);
        let mut studio = Studio::new(store, library);
        let source = studio.library.selected_character().unwrap().persona.clone();
        for (name, page, width, height) in [
            ("switcher", Page::Profiles, 420.0, 640.0),
            ("switcher-small", Page::Profiles, 210.0, 320.0),
            ("personalities", Page::Personality, 420.0, 640.0),
        ] {
            let ctx = egui::Context::default();
            crate::ui::ensure_style(&ctx);
            let mut switcher = Switcher::default();
            switcher.show(page);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            let mut textures = egui::TexturesDelta::default();
            let frames = if page == Page::Profiles { 160 } else { 4 };
            for frame in 0..frames {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(frame as f64 * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        assert!(!switcher.ui(ui.ctx(), &mut studio));
                    },
                );
                textures.append(std::mem::take(&mut output.textures_delta));
                if frame == frames - 1 {
                    output.textures_delta = std::mem::take(&mut textures);
                    let meshes =
                        ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
                    let mut overflow = None;
                    for primitive in &meshes {
                        if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                            for vertex in &mesh.vertices {
                                // Windows keep a small drop shadow outside their frame.
                                if primitive.clip_rect.contains(vertex.pos)
                                    && !screen.expand(14.0).contains(vertex.pos)
                                {
                                    overflow = Some(vertex.pos);
                                }
                            }
                        }
                    }
                    if let Some(folder) = std::env::var_os("WAIFU_RADIAL_REVIEW_DIR") {
                        let folder = std::path::PathBuf::from(folder);
                        std::fs::create_dir_all(&folder).unwrap();
                        crate::profile_library::write_review(
                            &output,
                            &meshes,
                            [width, height],
                            &folder.join(format!("{name}.json")),
                        );
                    }
                    output.textures_delta.clear();
                    assert!(overflow.is_none(), "{name}: {overflow:?}");
                }
                output.textures_delta.clear();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            textures.clear();
        }
        assert_eq!(studio.library.selected_character().unwrap().persona, source);
        assert_eq!(models(&studio.library).len(), 3);
        assert_eq!(personas(&studio.library).len(), 3);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
