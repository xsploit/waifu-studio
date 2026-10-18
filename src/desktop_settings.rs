//! App-local desktop preferences. Never changes compositor or system settings.
use bevy::prelude::{IVec2, Resource, Vec2};
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub automatic_motions: bool,
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
            automatic_motions: true,
            kwin_window_surfaces: waifu_studio::desktop_platform::kwin_wayland_session(),
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
            result.kwin_window_surfaces = waifu_studio::desktop_platform::kwin_wayland_session();
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
        result.avatar_zoom = result.avatar_zoom.clamp(0.5, 2.0);
        result.avatar_x = result.avatar_x.clamp(-1.0, 1.0);
        result.avatar_y = result.avatar_y.clamp(-1.0, 1.0);
        result
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label("Window & performance");
        let mut changed = ui
            .checkbox(&mut self.always_on_top, "Keep desktop companion on top")
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.focused_fps, 15..=144).text("Focused FPS"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.background_fps, 5..=60).text("Background FPS"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.overlay_fps, 5..=60).text("Desktop FPS"))
            .changed();
        ui.small("Lower caps reduce CPU/GPU use. Display sync may limit higher caps.");
        egui::ComboBox::from_label("Window size")
            .selected_text("Choose window size…")
            .width(240.0)
            .show_ui(ui, |ui| {
                for (w, h) in [(640, 720), (960, 720), (1280, 900)] {
                    if ui.selectable_label(false, format!("{w} × {h}")).clicked() {
                        self.resize_requested = Some((w, h));
                    }
                }
            });
        ui.label("Desktop sitting & placement");
        let mut sitting = self.sit_requested.unwrap_or(self.sitting);
        if ui
            .checkbox(&mut sitting, "Sit on a detected surface")
            .changed()
        {
            self.request_sitting(sitting);
        }
        if !self.sitting_status.is_empty() {
            ui.small(&self.sitting_status);
        }
        changed |= ui
            .checkbox(
                &mut self.automatic_motions,
                "Homunculus idle / grabbed / seated animations",
            )
            .changed();
        ui.small("Motion clips take priority. Drag the companion to line up with a window edge.");
        use waifu_studio::desktop_perch::PerchMode;
        let old_mode = self.perch.mode;
        egui::ComboBox::from_label("Snap position")
            .selected_text(format!("{:?}", self.perch.mode))
            .show_ui(ui, |ui| {
                for (mode, label) in [
                    (PerchMode::Disabled, "Free placement"),
                    (PerchMode::ScreenBottom, "Screen bottom"),
                    (PerchMode::ScreenLeft, "Bottom left"),
                    (PerchMode::ScreenRight, "Bottom right"),
                    (PerchMode::ManualSurface, "Manual surface"),
                ] {
                    ui.selectable_value(&mut self.perch.mode, mode, label);
                }
            });
        changed |= self.perch.mode != old_mode;
        changed |= ui
            .add(
                egui::Slider::new(&mut self.perch.horizontal_fraction, 0.0..=1.0)
                    .text("Seat horizontal position"),
            )
            .changed();
        if self.perch.mode == PerchMode::ManualSurface {
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.perch.manual_surface.y_fraction, 0.0..=1.0)
                        .text("Surface height from top"),
                )
                .changed();
        }
        if ui.button("Snap companion on current monitor").clicked() {
            self.perch_requested = true;
        }
        if !self.placement_status.is_empty() {
            ui.small(&self.placement_status);
        }
        let previous_detection = self.kwin_window_surfaces;
        egui::ComboBox::from_label("Window & taskbar detection")
            .selected_text(if self.kwin_window_surfaces {
                "Automatic · KDE Wayland"
            } else {
                "Disabled"
            })
            .width(280.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.kwin_window_surfaces,
                    true,
                    "Automatic · KDE Wayland",
                );
                ui.selectable_value(&mut self.kwin_window_surfaces, false, "Disabled");
            });
        changed |= previous_detection != self.kwin_window_surfaces;
        ui.small("Enables a temporary KWin script while in Desktop mode. It tracks window geometry and moves only this companion; nothing is installed in desktop settings.");
        if !self.surface_status.is_empty() {
            ui.small(&self.surface_status);
        }
        ui.small(
            "Other compositors may ignore screen snapping; manual dragging remains available.",
        );
        ui.label("Avatar placement & light");
        changed |= ui
            .add(egui::Slider::new(&mut self.avatar_zoom, 0.5..=2.0).text("Zoom"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.avatar_x, -1.0..=1.0).text("Horizontal"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.avatar_y, -1.0..=1.0).text("Vertical"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.light_intensity, 0.2..=2.0).text("Key light"))
            .changed();
        if ui.button("Reset placement & light").clicked() {
            self.avatar_zoom = 1.0;
            self.avatar_x = 0.0;
            self.avatar_y = 0.0;
            self.light_intensity = 1.0;
            changed = true;
        }
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
        let mut active = DesktopSettings {
            sitting: true,
            ..Default::default()
        };
        active.request_sitting(true);
        active.sitting_status = "Sitting on a detected surface".into();
        let saved = serde_json::to_value(&active).unwrap();
        assert!(saved.get("sitting").is_none());
        assert!(saved.get("sit_requested").is_none());
        assert!(saved.get("sitting_status").is_none());
        let reopened: DesktopSettings = serde_json::from_value(saved).unwrap();
        assert!(!reopened.sitting && reopened.sit_requested.is_none());
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
            waifu_studio::desktop_platform::kwin_wayland_session()
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
