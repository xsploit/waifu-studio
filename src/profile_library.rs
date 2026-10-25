//! Shared profile cards and bounded background extraction of embedded VRM portraits.
use crate::studio::Studio;
use bevy_egui::egui::{self, Color32, Pos2, Rect, Vec2};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

struct Portraits {
    images: HashMap<PathBuf, Option<egui::TextureHandle>>,
    pending: HashSet<PathBuf>,
    send: mpsc::SyncSender<PathBuf>,
    receive: mpsc::Receiver<(PathBuf, Option<egui::ColorImage>)>,
}
impl Portraits {
    fn new() -> Self {
        let (send, jobs) = mpsc::sync_channel::<PathBuf>(16);
        let (results, receive) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            while let Ok(path) = jobs.recv() {
                let image = portrait(&path).ok();
                if results.send((path, image)).is_err() {
                    break;
                }
            }
        });
        Self {
            images: HashMap::new(),
            pending: HashSet::new(),
            send,
            receive,
        }
    }
    fn get(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        while let Ok((path, image)) = self.receive.try_recv() {
            self.pending.remove(&path);
            let texture = image.map(|image| {
                ctx.load_texture("profile-portrait", image, egui::TextureOptions::LINEAR)
            });
            self.images.insert(path, texture);
        }
        if !self.images.contains_key(path)
            && !self.pending.contains(path)
            && self.images.len() + self.pending.len() < 96
            && self.send.try_send(path.to_path_buf()).is_ok()
        {
            self.pending.insert(path.to_path_buf());
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.images.get(path).and_then(Clone::clone)
    }
}
fn portrait(path: &Path) -> Result<egui::ColorImage, String> {
    use std::io::Read;
    const LIMIT: u64 = 128 * 1024 * 1024;
    let metadata = std::fs::metadata(path).map_err(|_| "No model")?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err("Model outside portrait limits".into());
    }
    let file = std::fs::File::open(path).map_err(|_| "No model")?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Unreadable model")?;
    if bytes.len() as u64 > LIMIT {
        return Err("Model too large for portrait extraction".into());
    }
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|_| "Invalid model")?;
    let doc = &gltf.document;
    let index = if let Some(index) = doc
        .extension_value("VRMC_vrm")
        .and_then(|v| v.get("meta"))
        .and_then(|v| v.get("thumbnailImage"))
        .and_then(|v| v.as_u64())
    {
        usize::try_from(index).map_err(|_| "Invalid image index")?
    } else {
        let texture = doc
            .extension_value("VRM")
            .and_then(|v| v.get("meta"))
            .and_then(|v| v.get("texture"))
            .and_then(|v| v.as_u64())
            .ok_or("No portrait")?;
        doc.textures()
            .nth(usize::try_from(texture).map_err(|_| "Invalid texture index")?)
            .ok_or("Missing texture")?
            .source()
            .index()
    };
    let image = doc.images().nth(index).ok_or("Missing image")?;
    let gltf::image::Source::View { view, .. } = image.source() else {
        return Err("Only embedded portraits are read".into());
    };
    if view.buffer().index() != 0 || !matches!(view.buffer().source(), gltf::buffer::Source::Bin) {
        return Err("Only embedded model buffers are read".into());
    }
    let blob = gltf.blob.as_ref().ok_or("No embedded buffer")?;
    if view.length() > 8 * 1024 * 1024 {
        return Err("Portrait too large".into());
    }
    let end = view
        .offset()
        .checked_add(view.length())
        .ok_or("Invalid view")?;
    let encoded = blob.get(view.offset()..end).ok_or("Invalid image bounds")?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|_| "Invalid image")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "Portrait could not be decoded")?;
    let image = image
        .thumbnail(image.width().min(192), image.height().min(192))
        .to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}
pub(crate) fn thumbnail(ui: &egui::Ui, path: &Path, name: &str, rect: Rect) {
    let key = egui::Id::new("profile_portraits");
    let cache = ui.ctx().data_mut(|d| {
        d.get_temp::<Arc<Mutex<Portraits>>>(key).unwrap_or_else(|| {
            let cache = Arc::new(Mutex::new(Portraits::new()));
            d.insert_temp(key, cache.clone());
            cache
        })
    });
    let texture = cache.lock().ok().and_then(|mut p| p.get(ui.ctx(), path));
    ui.painter()
        .rect_filled(rect, 8, Color32::from_rgb(30, 31, 40));
    if let Some(texture) = texture {
        egui::Image::new((texture.id(), texture.size_vec2()))
            .fit_to_exact_size(rect.size())
            .maintain_aspect_ratio(true)
            .corner_radius(8)
            .paint_at(ui, rect);
    } else {
        let initials: String = name
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect();
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initials,
            egui::FontId::proportional(rect.height() * 0.26),
            Color32::from_rgb(177, 163, 217),
        );
    }
}
pub(crate) fn gallery(ui: &mut egui::Ui, studio: &mut Studio) {
    ui.horizontal_wrapped(|ui| {
        ui.heading("Your profiles");
        if ui.button("Save profile").clicked() {
            studio.save();
            if studio.error.is_none() {
                studio.notice = Some("Profile saved.".into());
            }
        }
        if ui.button("Save as new").clicked() {
            studio.duplicate_character();
        }
        if ui
            .add_enabled(
                studio.library.selected.is_some(),
                egui::Button::new("Use on desktop"),
            )
            .clicked()
        {
            studio.mode = crate::studio::ViewMode::Desktop;
        }
        ui.menu_button("Add…", |ui| {
            if ui.button("Import avatar…").clicked() {
                studio.open_models();
                ui.close();
            }
            if ui.button("Scan folder…").clicked() {
                studio.scan_folder();
                ui.close();
            }
        });
    });
    ui.label(
        egui::RichText::new(
            "Model, personality, voice and appearance—saved together. Pick a profile to load it.",
        )
        .weak(),
    );
    let search_id = egui::Id::new("profile_gallery_search");
    let mut search = ui
        .ctx()
        .data(|d| d.get_temp::<String>(search_id).unwrap_or_default());
    ui.add(
        egui::TextEdit::singleline(&mut search)
            .hint_text("Find a profile…")
            .desired_width(240.0),
    );
    ui.ctx()
        .data_mut(|d| d.insert_temp(search_id, search.clone()));
    let search = search.to_lowercase();
    let width = ui.available_width();
    let columns = ((width + 8.0) / 154.0).floor().max(1.0) as usize;
    let card_width = ((width - (columns - 1) as f32 * 8.0) / columns as f32).min(190.0);
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_salt("profile_gallery")
        .min_scrolled_height(182.0)
        .max_height(370.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("profile_card_grid")
                .spacing(Vec2::splat(8.0))
                .show(ui, |ui| {
                    let mut count = 0;
                    for character in studio
                        .library
                        .characters
                        .iter()
                        .filter(|c| c.name.to_lowercase().contains(&search))
                    {
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(card_width, 174.0),
                            egui::Sense::click(),
                        );
                        let selected = studio.library.selected == Some(character.id);
                        let stroke = if selected {
                            Color32::from_rgb(188, 168, 249)
                        } else if response.hovered() {
                            Color32::from_gray(110)
                        } else {
                            Color32::from_gray(55)
                        };
                        ui.painter().rect(
                            rect,
                            10,
                            Color32::from_rgb(25, 26, 33),
                            egui::Stroke::new(if selected { 1.5 } else { 1.0 }, stroke),
                            egui::StrokeKind::Inside,
                        );
                        let image_rect = Rect::from_min_size(
                            rect.min + Vec2::splat(7.0),
                            Vec2::new(card_width - 14.0, 122.0),
                        );
                        thumbnail(ui, &character.model, &character.name, image_rect);
                        text(
                            ui,
                            Pos2::new(rect.left() + 9.0, rect.top() + 135.0),
                            &character.name,
                            card_width - 18.0,
                            12.0,
                            Color32::from_gray(235),
                        );
                        let persona = crate::ui::personality_catalog::by_prompt(&character.persona)
                            .map_or(
                                if character.persona.trim().is_empty() {
                                    "Default personality"
                                } else {
                                    "Custom personality"
                                },
                                |p| p.name,
                            );
                        text(
                            ui,
                            Pos2::new(rect.left() + 9.0, rect.top() + 153.0),
                            persona,
                            card_width - 18.0,
                            10.0,
                            Color32::from_gray(160),
                        );
                        if response
                            .on_hover_text(format!(
                                "{}\n{}\n{}",
                                character.name,
                                character
                                    .model
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy(),
                                persona
                            ))
                            .clicked()
                        {
                            chosen = Some(character.id);
                        }
                        count += 1;
                        if count % columns == 0 {
                            ui.end_row();
                        }
                    }
                    if count == 0 {
                        ui.label("No profiles yet. Import an avatar to start.");
                    }
                });
        });
    if let Some(id) = chosen
        && studio.library.selected != Some(id)
    {
        studio.select_character(id);
    }
    ui.add_space(10.0);
}
pub(crate) fn text(ui: &egui::Ui, pos: Pos2, text: &str, width: f32, size: f32, color: Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.into(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn model(version_one: bool) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([180, 80, 210, 255]));
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let png = encoded.into_inner();
        let extension = if version_one {
            serde_json::json!({"VRMC_vrm":{"meta":{"thumbnailImage":0}}})
        } else {
            serde_json::json!({"VRM":{"meta":{"texture":0}}})
        };
        let doc = serde_json::json!({"asset":{"version":"2.0"},"extensions":extension,
            "buffers":[{"byteLength":png.len()}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png"}],"textures":[{"source":0}]});
        let mut json = serde_json::to_vec(&doc).unwrap();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut bin = png;
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend(json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend(bin);
        glb
    }
    #[test]
    fn embedded_vrm_zero_and_one_portraits_decode_without_external_fetches() {
        let path =
            std::env::temp_dir().join(format!("profile-thumbnail-{}.vrm", uuid::Uuid::new_v4()));
        for version in [false, true] {
            std::fs::write(&path, model(version)).unwrap();
            let result = portrait(&path).unwrap();
            assert_eq!(result.size, [2, 2]);
            assert_eq!(result.pixels[0].to_array(), [180, 80, 210, 255]);
        }
        std::fs::write(&path, b"not a model").unwrap();
        assert!(portrait(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn profile_cards_load_real_portraits_and_fit_compact_layouts() {
        use waifu_studio::data::{Character, Store};
        let directory =
            std::env::temp_dir().join(format!("profile-gallery-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let bundle: serde_json::Value =
            serde_json::from_str(include_str!("../assets/characters/bundle.json")).unwrap();
        for entry in bundle["characters"].as_array().unwrap().iter().take(3) {
            let mut character: Character = serde_json::from_value(entry.clone()).unwrap();
            character.model = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/characters")
                .join(character.model);
            library.characters.push(character);
        }
        library.selected = Some(library.characters[0].id);
        let mut studio = Studio::new(store, library);
        for width in [760.0, 420.0, 280.0] {
            let ctx = egui::Context::default();
            crate::ui::ensure_style(&ctx);
            let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 520.0));
            let mut textures = egui::TexturesDelta::default();
            for frame in 0..100 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(frame as f64 * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        let right = ui.max_rect().right();
                        gallery(ui, &mut studio);
                        assert!(
                            ui.min_rect().right() <= right + 1.0,
                            "gallery overflow at {width}"
                        );
                    },
                );
                textures.append(std::mem::take(&mut output.textures_delta));
                let cache = ctx
                    .data(|d| {
                        d.get_temp::<Arc<Mutex<Portraits>>>(egui::Id::new("profile_portraits"))
                    })
                    .unwrap();
                let ready = {
                    let cache = cache.lock().unwrap();
                    cache.pending.is_empty() && cache.images.len() == 3
                };
                if ready {
                    assert_eq!(
                        cache
                            .lock()
                            .unwrap()
                            .images
                            .values()
                            .filter(|image| image.is_some())
                            .count(),
                        3
                    );
                    if let Some(folder) = std::env::var_os("WAIFU_RADIAL_REVIEW_DIR") {
                        let folder = PathBuf::from(folder);
                        std::fs::create_dir_all(&folder).unwrap();
                        output.textures_delta = std::mem::take(&mut textures);
                        let shapes = ctx.tessellate(
                            std::mem::take(&mut output.shapes),
                            output.pixels_per_point,
                        );
                        write_review(
                            &output,
                            &shapes,
                            [width, 520.0],
                            &folder.join(format!("profiles-{width}.json")),
                        );
                    }
                    output.textures_delta.clear();
                    break;
                }
                output.textures_delta.clear();
                assert!(frame < 99, "portrait worker did not settle");
                std::thread::sleep(Duration::from_millis(20));
            }
            textures.clear();
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
pub(crate) fn write_review(
    output: &egui::FullOutput,
    primitives: &[egui::ClippedPrimitive],
    size: [f32; 2],
    path: &Path,
) {
    let textures: Vec<_> = output
        .textures_delta
        .set
        .iter()
        .flat_map(|(id, deltas)| {
            deltas.iter().map(move |delta| {
                let egui::ImageData::Color(image) = &delta.image;
                serde_json::json!({"id":format!("{id:?}"),"size":image.size,"pos":delta.pos,
            "pixels":image.pixels.iter().map(|c|c.to_array()).collect::<Vec<_>>()})
            })
        })
        .collect();
    let meshes:Vec<_>=primitives.iter().filter_map(|clipped| {
        let egui::epaint::Primitive::Mesh(mesh)=&clipped.primitive else {return None};
        Some(serde_json::json!({"texture":format!("{:?}",mesh.texture_id),"indices":mesh.indices,
            "clip":[clipped.clip_rect.min.x,clipped.clip_rect.min.y,clipped.clip_rect.max.x,clipped.clip_rect.max.y],
            "vertices":mesh.vertices.iter().map(|v|serde_json::json!({"p":[v.pos.x,v.pos.y],"uv":[v.uv.x,v.uv.y],"color":v.color.to_array()})).collect::<Vec<_>>()}))
    }).collect();
    std::fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({"size":size,"textures":textures,"meshes":meshes}))
            .unwrap(),
    )
    .unwrap();
}
