//! App-local desktop preferences. Never changes compositor or system settings.
use bevy::prelude::{IVec2, Resource, Vec2};
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use waifu_studio::settings_kit as kit;

#[derive(Clone, Copy)]
pub struct DesktopMoveRequest {
    pub logical_origin: Vec2,
    pub physical_origin: IVec2,
    pub native_position_supported: bool,
}

#[derive(Resource, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub focused_fps: u32,
    pub background_fps: u32,
    pub overlay_fps: u32,
    pub always_on_top: bool,
    pub light_intensity: f32,
    pub avatar_zoom: f32,
    pub avatar_x: f32,
    pub avatar_y: f32,
    /// Actual session state, confirmed by a live desktop surface.
    #[serde(skip)]
    pub sitting: bool,
    #[serde(skip)]
    pub sit_requested: Option<bool>,
    #[serde(skip)]
    pub sitting_status: String,
    #[serde(skip)]
    pub move_requested: Option<DesktopMoveRequest>,
    #[serde(skip)]
    pub placement_status: String,
    /// Auxiliary passthrough recovery is created only by a menu action.
    #[serde(skip)]
    pub controls_requested: bool,
    pub automatic_motions: bool,
    pub window_occlusion: bool,
    pub kwin_window_surfaces: bool,
    #[serde(default)]
    surface_detection_version: u8,
    #[serde(skip)]
    pub surface_status: String,
    pub perch: waifu_studio::desktop_perch::PerchSettings,
    #[serde(skip)]
    pub perch_requested: bool,
    #[serde(skip)]
    pub resize_requested: Option<(u32, u32)>,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    error: Option<String>,
}
impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            focused_fps: 60,
            background_fps: 15,
            overlay_fps: 30,
            always_on_top: true,
            light_intensity: 1.0,
            avatar_zoom: 1.0,
            avatar_x: 0.0,
            avatar_y: 0.0,
            sitting: false,
            sit_requested: None,
            sitting_status: String::new(),
            move_requested: None,
            placement_status: String::new(),
            controls_requested: false,
            automatic_motions: true,
            window_occlusion: true,
            kwin_window_surfaces: waifu_studio::desktop_platform::window_surfaces_session(),
            surface_detection_version: 1,
            surface_status: String::new(),
            perch: Default::default(),
            perch_requested: false,
            resize_requested: None,
            path: None,
            dirty: false,
            error: None,
        }
    }
}
impl DesktopSettings {
    pub fn request_sitting(&mut self, sitting: bool) {
        self.sit_requested = Some(sitting);
        self.sitting_status.clear();
    }
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
    pub fn load(path: PathBuf) -> Self {
        let read = || -> std::io::Result<Vec<u8>> {
            use std::io::Read;
            let file = std::fs::File::open(&path)?;
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::other("Settings must be a regular file"));
            }
            let mut bytes = Vec::new();
            file.take(65537).read_to_end(&mut bytes)?;
            if bytes.len() > 65536 {
                return Err(std::io::Error::other("Desktop settings exceed 64 KiB"));
            }
            Ok(bytes)
        };
        let mut result = match read() {
            Ok(bytes) => serde_json::from_slice::<Self>(&bytes).unwrap_or_else(|err| Self {
                error: Some(format!("Desktop settings: {err}")),
                ..Self::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => Self {
                error: Some(format!("Desktop settings: {err}")),
                ..Self::default()
            },
        };
        result.path = Some(path);
        // Earlier builds disabled the experimental bridge by default. Migrate
        // that legacy default once; an explicit Disabled choice in this build
        // is retained on subsequent loads.
        if result.surface_detection_version == 0 {
            result.kwin_window_surfaces = waifu_studio::desktop_platform::window_surfaces_session();
            result.surface_detection_version = 1;
            result.dirty = true;
        }
        result.perch = result.perch.sanitized();
        result.focused_fps = result.focused_fps.clamp(15, 144);
        result.background_fps = result.background_fps.clamp(5, 60);
        result.overlay_fps = result.overlay_fps.clamp(5, 60);
        if !result.light_intensity.is_finite() {
            result.light_intensity = 1.0;
        }
        if !result.avatar_zoom.is_finite() {
            result.avatar_zoom = 1.0;
        }
        if !result.avatar_x.is_finite() {
            result.avatar_x = 0.0;
        }
        if !result.avatar_y.is_finite() {
            result.avatar_y = 0.0;
        }
        result.light_intensity = result.light_intensity.clamp(0.2, 2.0);
        result.avatar_zoom = result.avatar_zoom.clamp(0.5, 4.0);
        result.avatar_x = result.avatar_x.clamp(-1.0, 1.0);
        result.avatar_y = result.avatar_y.clamp(-1.0, 1.0);
        result
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        kit::group(ui, "Desktop companion", |ui| {
            changed |= kit::toggle_row(
                ui,
                "Keep on top",
                "Keep the companion above other windows.",
                &mut self.always_on_top,
            )
            .changed();
            changed |= kit::slider_row(
                ui,
                "Avatar size",
                "Scroll over the companion to resize her.",
                egui::Slider::new(&mut self.avatar_zoom, 0.5..=4.0),
            )
            .changed();
            changed |= kit::toggle_row(
                ui,
                "Automatic animations",
                "Idle, grabbed and seated motions.",
                &mut self.automatic_motions,
            )
            .changed();
            let mut sitting = self.sit_requested.unwrap_or(self.sitting);
            if kit::toggle_row(
                ui,
                "Sit on an edge",
                "Drag onto a detected window or taskbar.",
                &mut sitting,
            )
            .changed()
            {
                self.request_sitting(sitting);
            }
            if !self.sitting_status.is_empty() {
                kit::note(ui, &self.sitting_status);
            }
            let before = self.kwin_window_surfaces;
            kit::row(
                ui,
                "Desktop integration",
                "Window edges and taskbar detection.",
                |ui| {
                    egui::ComboBox::from_id_salt("Window & taskbar detection")
                    .selected_text(if self.kwin_window_surfaces { "Automatic" } else { "Off" })
                    .width(160.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.kwin_window_surfaces, true, "Automatic · Windows / KDE / Hyprland");
                        ui.selectable_value(&mut self.kwin_window_surfaces, false, "Off");
                    }).response.on_hover_text("Detect visible window tops and taskbar edges. Windows and KDE support topmost placement; Hyprland cannot place the companion above panels.");
                },
            );
            changed |= before != self.kwin_window_surfaces;
            if !self.surface_status.is_empty() {
                kit::note(ui, &self.surface_status);
            }
        });
        egui::CollapsingHeader::new("Advanced desktop settings")
            .default_open(false)
            .show(ui, |ui| {
                kit::group(ui, "Placement", |ui| {
                    changed |= kit::toggle_row(
                        ui,
                        "Window occlusion",
                        "Hide body parts behind a seated window ledge.",
                        &mut self.window_occlusion,
                    )
                    .changed();
                    use waifu_studio::desktop_perch::PerchMode;
                    let label = |mode| match mode {
                        PerchMode::Disabled => "Free placement",
                        PerchMode::ScreenBottom => "Bottom edge",
                        PerchMode::ScreenLeft => "Bottom left",
                        PerchMode::ScreenRight => "Bottom right",
                        PerchMode::ManualSurface => "Manual surface",
                    };
                    let before = self.perch.mode;
                    kit::row(ui, "Snap position", "", |ui| {
                        egui::ComboBox::from_id_salt("Snap position")
                            .selected_text(label(self.perch.mode))
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for mode in [
                                    PerchMode::Disabled,
                                    PerchMode::ScreenBottom,
                                    PerchMode::ScreenLeft,
                                    PerchMode::ScreenRight,
                                    PerchMode::ManualSurface,
                                ] {
                                    ui.selectable_value(&mut self.perch.mode, mode, label(mode));
                                }
                            });
                    });
                    changed |= before != self.perch.mode;
                    changed |= kit::slider_row(
                        ui,
                        "Horizontal position",
                        "",
                        egui::Slider::new(&mut self.perch.horizontal_fraction, 0.0..=1.0),
                    )
                    .changed();
                    if self.perch.mode == PerchMode::ManualSurface {
                        changed |= kit::slider_row(
                            ui,
                            "Surface height",
                            "Measured from the top of the screen.",
                            egui::Slider::new(&mut self.perch.manual_surface.y_fraction, 0.0..=1.0),
                        )
                        .changed();
                    }
                    kit::row(
                        ui,
                        "Apply position",
                        "Uses the current monitor's available space.",
                        |ui| {
                            if ui.button("Snap companion").clicked() {
                                self.perch_requested = true;
                            }
                        },
                    );
                    if !self.placement_status.is_empty() {
                        kit::note(ui, &self.placement_status);
                    }
                });
                kit::group(ui, "Performance & window", |ui| {
                    changed |= kit::slider_row(
                        ui,
                        "Focused FPS",
                        "",
                        egui::Slider::new(&mut self.focused_fps, 15..=144),
                    )
                    .changed();
                    changed |= kit::slider_row(
                        ui,
                        "Background FPS",
                        "",
                        egui::Slider::new(&mut self.background_fps, 5..=60),
                    )
                    .changed();
                    changed |= kit::slider_row(
                        ui,
                        "Desktop FPS",
                        "",
                        egui::Slider::new(&mut self.overlay_fps, 5..=60),
                    )
                    .changed();
                    kit::row(
                        ui,
                        "Studio window size",
                        "Desktop mode follows the avatar size above.",
                        |ui| {
                            egui::ComboBox::from_id_salt("Window size")
                                .selected_text("Choose size…")
                                .width(160.0)
                                .show_ui(ui, |ui| {
                                    for (w, h) in [(640, 720), (960, 720), (1280, 900)] {
                                        if ui
                                            .selectable_label(false, format!("{w} × {h}"))
                                            .clicked()
                                        {
                                            self.resize_requested = Some((w, h));
                                        }
                                    }
                                });
                        },
                    );
                });
                kit::group(ui, "Alignment & light", |ui| {
                    changed |= kit::slider_row(
                        ui,
                        "Horizontal offset",
                        "",
                        egui::Slider::new(&mut self.avatar_x, -1.0..=1.0),
                    )
                    .changed();
                    changed |= kit::slider_row(
                        ui,
                        "Vertical offset",
                        "",
                        egui::Slider::new(&mut self.avatar_y, -1.0..=1.0),
                    )
                    .changed();
                    changed |= kit::slider_row(
                        ui,
                        "Key light",
                        "",
                        egui::Slider::new(&mut self.light_intensity, 0.2..=2.0),
                    )
                    .changed();
                    kit::row(
                        ui,
                        "Reset placement",
                        "Restore size, offsets and lighting.",
                        |ui| {
                            if ui.button("Reset").clicked() {
                                self.avatar_zoom = 1.0;
                                self.avatar_x = 0.0;
                                self.avatar_y = 0.0;
                                self.light_intensity = 1.0;
                                changed = true;
                            }
                        },
                    );
                });
            });
        self.dirty |= changed;
        if ui
            .add_enabled(self.dirty, egui::Button::new("Save window settings"))
            .clicked()
            && let Some(path) = &self.path
        {
            let save = || -> Result<(), Box<dyn std::error::Error>> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
                std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
                std::fs::rename(temporary, path)?;
                Ok(())
            };
            let result = save();
            self.dirty = result.is_err();
            self.error = result
                .err()
                .map(|err| format!("Could not save desktop settings: {err}"));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sitting_and_surface_requests_never_persist_across_sessions() {
        let legacy: DesktopSettings =
            serde_json::from_str(r#"{"sitting":true,"sit_requested":true}"#).unwrap();
        assert!(!legacy.sitting && legacy.sit_requested.is_none());
        assert!(!legacy.controls_requested);
        assert!(
            legacy.window_occlusion,
            "Existing profiles get occlusion by default"
        );
        let mut active = DesktopSettings {
            sitting: true,
            ..Default::default()
        };
        active.request_sitting(true);
        active.controls_requested = true;
        active.sitting_status = "Sitting on a detected surface".into();
        let saved = serde_json::to_value(&active).unwrap();
        assert!(saved.get("sitting").is_none());
        assert!(saved.get("sit_requested").is_none());
        assert!(saved.get("sitting_status").is_none());
        assert!(saved.get("controls_requested").is_none());
        let reopened: DesktopSettings = serde_json::from_value(saved).unwrap();
        assert!(!reopened.sitting && reopened.sit_requested.is_none());
        assert!(!reopened.controls_requested);
    }

    #[test]
    fn legacy_detection_default_migrates_but_current_disabled_choice_survives() {
        let directory =
            std::env::temp_dir().join(format!("desktop-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("desktop.json");
        std::fs::write(&path, br#"{"kwin_window_surfaces":false}"#).unwrap();
        let legacy = DesktopSettings::load(path.clone());
        assert_eq!(
            legacy.kwin_window_surfaces,
            waifu_studio::desktop_platform::window_surfaces_session()
        );
        assert_eq!(legacy.surface_detection_version, 1);
        std::fs::write(
            &path,
            br#"{"kwin_window_surfaces":false,"surface_detection_version":1}"#,
        )
        .unwrap();
        assert!(!DesktopSettings::load(path).kwin_window_surfaces);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
