//! Explicitly enabled local automation mailbox. No socket or global input.
//! Requests are bounded, actions run on the app thread, and status omits chat/keys.
use crate::{
    desktop_settings::DesktopSettings,
    motion_services::MotionServices,
    studio::{Studio, Tab, ViewMode},
};
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use waifu_studio::{animation::AnimationState, loading::AvatarLoadState};

#[derive(Resource)]
pub(crate) struct LocalControl {
    directory: PathBuf,
    session: String,
    poll: Instant,
    report: Instant,
    frame: Instant,
    intervals: Vec<f64>,
    fps: f64,
    p99_ms: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: uuid::Uuid,
    session: String,
    command: Value,
}

impl LocalControl {
    pub fn new(directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(directory.join("captures")).map_err(|e| e.to_string())?;
        let session = uuid::Uuid::new_v4().to_string();
        // Old requests never carry into a new app session.
        let _ = std::fs::remove_file(directory.join("request.json"));
        write_json(
            &directory.join("session.json"),
            &json!({"session":session,"pid":std::process::id(),"protocol":1}),
        )?;
        let now = Instant::now();
        Ok(Self {
            directory,
            session,
            poll: now,
            report: now,
            frame: now,
            intervals: Vec::new(),
            fps: 0.,
            p99_ms: 0.,
        })
    }
}
fn write_json(path: &std::path::Path, value: &Value) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // Windows rename cannot replace an existing destination.
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}
fn safe_capture(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name.ends_with(".png")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.contains("..")
}
pub(crate) fn frame(world: &mut World) {
    world.resource_scope(|world, mut control: Mut<LocalControl>| {
        let now = Instant::now();
        let interval = now.duration_since(control.frame).as_secs_f64();
        control.intervals.push(interval);
        control.frame = now;
        if now.duration_since(control.report) >= Duration::from_secs(2) {
            let elapsed: f64 = control.intervals.iter().sum();
            control.fps = control.intervals.len() as f64 / elapsed.max(0.001);
            control.intervals.sort_unstable_by(f64::total_cmp);
            control.p99_ms = control.intervals[(control.intervals.len() - 1) * 99 / 100] * 1000.;
            control.intervals.clear();
            control.report = now;
            let status = status(world, &control);
            let _ = write_json(&control.directory.join("status.json"), &status);
        }
        if now.duration_since(control.poll) < Duration::from_millis(100) {
            return;
        }
        control.poll = now;
        let path = control.directory.join("request.json");
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return;
        };
        if !metadata.is_file() || metadata.len() > 65536 {
            let _ = std::fs::remove_file(path);
            return;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let _ = std::fs::remove_file(path);
        let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
            return;
        };
        let result = if request.session != control.session {
            Err("Wrong app session".to_string())
        } else {
            execute(world, &control, &request.command)
        };
        let response = match result {
            Ok(result) => json!({"id":request.id,"ok":true,"result":result}),
            Err(error) => json!({"id":request.id,"ok":false,"error":error}),
        };
        let _ = write_json(
            &control
                .directory
                .join(format!("response-{}.json", request.id)),
            &response,
        );
    });
}
fn status(world: &mut World, control: &LocalControl) -> Value {
    let studio = world.resource::<Studio>();
    let animation = world.resource::<AnimationState>();
    let desktop = world.resource::<DesktopSettings>();
    let character = studio.library.selected_character();
    let mut result = json!({
        "fps":control.fps,"frame_p99_ms":control.p99_ms,
        "view":format!("{:?}",studio.mode),"screen":format!("{:?}",studio.tab),
        "character":character.map(|c| &c.name),
        "motion":{"path":animation.path,"position":animation.position,"playing":animation.playing,"loading":animation.loading,"error":animation.error,"arm_clearance":animation.arm_clearance,"soft_arm_clearance":animation.soft_arm_clearance},
        "gaze":studio.library.scene.gaze,"facial":studio.library.scene.facial,
        "caps":{"focused":desktop.focused_fps,"background":desktop.background_fps,"desktop":desktop.overlay_fps},
        "sitting":desktop.sitting,"sitting_status":desktop.sitting_status,
        "surface_status":desktop.surface_status,
    });
    let avatar = world.resource::<AvatarLoadState>().active;
    result["avatar_ready"] = json!(avatar.is_some());
    let mut windows = world.query::<&Window>();
    result["windows"]=json!(windows.iter(world).map(|w|json!({"focused":w.focused,"width":w.physical_width(),"height":w.physical_height(),"scale":w.scale_factor()})).collect::<Vec<_>>());
    result
}
fn execute(world: &mut World, control: &LocalControl, command: &Value) -> Result<Value, String> {
    let action = command
        .get("action")
        .and_then(Value::as_str)
        .ok_or("Missing action")?;
    match action {
        "status" => return Ok(status(world, control)),
        "list_motions" => {
            let entries = world.resource::<MotionServices>().entries(false);
            return Ok(json!(
                entries
                    .iter()
                    .map(|e| json!({"path":e.path,"label":e.label,"format":e.format}))
                    .collect::<Vec<_>>()
            ));
        }
        "play_motion" => {
            let path = PathBuf::from(
                command
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("Missing path")?,
            );
            let entries = world.resource::<MotionServices>().entries(false);
            if !entries.iter().any(|e| e.path == path) {
                return Err("Motion must be in the app catalog".into());
            }
            let mut animation = world.resource_mut::<AnimationState>();
            animation.load_path(path)?;
            animation.take_manual_control();
        }
        "stop_motion" => {
            let mut animation = world.resource_mut::<AnimationState>();
            animation.take_manual_control();
            animation.stop();
        }
        "set_automatic_motion" => {
            let enabled = command
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("Missing enabled boolean")?;
            {
                let mut studio = world.resource_mut::<Studio>();
                studio.library.scene.companion_motion.enabled = enabled;
                studio.library.scene.reply_motion.mode = if enabled {
                    waifu_studio::data::ReplyMotionMode::AutoPlay
                } else {
                    waifu_studio::data::ReplyMotionMode::Off
                };
                studio.save();
            }
            if !enabled {
                let mut animation = world.resource_mut::<AnimationState>();
                animation.take_manual_control();
                animation.stop();
            } else {
                world
                    .resource_mut::<AnimationState>()
                    .resume_automatic_control();
                crate::motion_services::lock(&world.resource::<MotionServices>().companion)
                    .resume();
            }
        }
        "set_camera_view" => {
            crate::runtime::control_camera_view(
                world,
                command
                    .get("view")
                    .and_then(Value::as_str)
                    .ok_or("Missing view")?,
            )?;
        }
        "restore_window" => {
            let mut windows =
                world.query_filtered::<&mut Window, With<bevy::window::PrimaryWindow>>();
            for mut window in windows.iter_mut(world) {
                window.visible = true;
                window.set_minimized(false);
                window.resolution.set(1280., 800.);
            }
        }
        "set_frame_caps" => {
            let mut changes = Vec::new();
            for (key, min, max) in [
                ("focused", 15, 144),
                ("background", 5, 60),
                ("desktop", 5, 60),
            ] {
                if let Some(value) = command.get(key) {
                    let value = value
                        .as_u64()
                        .filter(|v| (*v >= min) && (*v <= max))
                        .ok_or("Invalid frame cap")? as u32;
                    changes.push((key, value));
                }
            }
            let mut caps = world.resource_mut::<DesktopSettings>();
            for (key, value) in changes {
                match key {
                    "focused" => caps.focused_fps = value,
                    "background" => caps.background_fps = value,
                    _ => caps.overlay_fps = value,
                }
            }
            caps.mark_dirty();
            caps.save()?;
        }
        "set_gaze" => {
            let config = serde_json::from_value::<waifu_studio::gaze::GazeConfig>(
                command.get("config").cloned().ok_or("Missing config")?,
            )
            .map_err(|e| e.to_string())?;
            config.validate()?;
            let mut studio = world.resource_mut::<Studio>();
            studio.library.scene.gaze = config;
            studio.save();
        }
        "set_view" => {
            let mode = match command.get("view").and_then(Value::as_str) {
                Some("studio") => ViewMode::Studio,
                Some("desktop") => ViewMode::Desktop,
                _ => return Err("view must be studio or desktop".into()),
            };
            world.resource_mut::<Studio>().mode = mode;
        }
        "set_screen" => {
            let tab = match command.get("screen").and_then(Value::as_str) {
                Some("chat") => Tab::Chat,
                Some("scene") => Tab::Scene,
                Some("connections") => Tab::Connections,
                Some("voice") => Tab::Voice,
                Some("tools") => Tab::Tools,
                Some("memory") => Tab::Memory,
                Some("characters") => Tab::Characters,
                Some("tasks") => Tab::Tasks,
                Some("extensions") => Tab::Extensions,
                _ => return Err("Unknown screen".into()),
            };
            let mut studio = world.resource_mut::<Studio>();
            studio.mode = ViewMode::Studio;
            studio.tab = tab;
        }
        "sit" => world.resource_mut::<DesktopSettings>().request_sitting(
            command
                .get("sitting")
                .and_then(Value::as_bool)
                .ok_or("Missing sitting boolean")?,
        ),
        "capture" => {
            let name = command
                .get("name")
                .and_then(Value::as_str)
                .ok_or("Missing name")?;
            if !safe_capture(name) {
                return Err("Capture needs a plain .png filename".into());
            }
            let screenshot = if let Some(target) = world.resource::<crate::Capture>().target.clone()
            {
                bevy::render::view::screenshot::Screenshot::image(target)
            } else {
                bevy::render::view::screenshot::Screenshot::primary_window()
            };
            world
                .spawn(screenshot)
                .observe(bevy::render::view::screenshot::save_to_disk(
                    control.directory.join("captures").join(name),
                ));
            return Ok(json!({"queued":true,"path":control.directory.join("captures").join(name)}));
        }
        _ => return Err("Unknown action".into()),
    }
    Ok(json!({"accepted":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_cannot_escape_mailbox_and_requests_reject_unrecognized_fields() {
        for name in [
            "../private.png",
            "C:\\tmp\\x.png",
            "a/b.png",
            "a.png.exe",
            "",
            "..png",
        ] {
            assert!(!safe_capture(name));
        }
        assert!(safe_capture("hikari-side-001.png"));
        assert!(
            serde_json::from_value::<Request>(
                json!({"id":uuid::Uuid::new_v4(),"session":"x","command":{},"exec":"no"})
            )
            .is_err()
        );
    }
}
