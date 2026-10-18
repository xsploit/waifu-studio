//! Explicitly enabled, runtime-only Hyprland desktop geometry bridge.
//!
//! Talks to this user's Hyprland instance socket directly; `hyprctl` and any
//! other process are never executed. `start` authenticates the socket before
//! any request. A background thread polls geometry at 20 Hz so the render
//! frame only reads the latest snapshot. Window titles are used solely to find
//! this process's window and are never retained.
//!
//! IPC reference: <https://wiki.hypr.land/IPC/> (request socket `.socket.sock`,
//! `j/` JSON prefix, `dispatch movewindowpixel exact X Y,address:0x…`).

use super::{DesktopSnapshot, surface_id};
use super::{MAX_COORDINATE, MAX_PAYLOAD, MAX_RECORDS, SNAPSHOT_TTL, valid_point};
use crate::desktop_interaction::{DesktopSurface, ScreenRect, SurfaceId};
use bevy::prelude::Vec2;
use serde::Deserialize;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const IO_TIMEOUT: Duration = Duration::from_millis(250);
/// Keep at least this many logical pixels reachable on the nearest monitor.
const REACHABLE: f32 = 24.;

#[derive(Deserialize)]
struct WireWorkspace {
    id: i64,
}

#[derive(Deserialize)]
struct WireMonitor {
    id: i64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
    #[serde(default)]
    transform: u8,
    /// Logical left, top, right, bottom space reserved by bars and layers.
    #[serde(default)]
    reserved: [f32; 4],
    #[serde(rename = "activeWorkspace")]
    active_workspace: WireWorkspace,
    #[serde(rename = "specialWorkspace")]
    special_workspace: Option<WireWorkspace>,
    #[serde(default)]
    disabled: bool,
}

/// Hyprland reported `fullscreen` as a bool before 0.42 and a mode int after.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireFullscreen {
    Flag(bool),
    Mode(i64),
}

impl WireFullscreen {
    fn active(&self) -> bool {
        match self {
            Self::Flag(flag) => *flag,
            Self::Mode(mode) => *mode != 0,
        }
    }
}

#[derive(Deserialize)]
struct WireClient {
    address: String,
    mapped: bool,
    hidden: bool,
    at: [f32; 2],
    size: [f32; 2],
    workspace: WireWorkspace,
    floating: bool,
    pid: i64,
    title: String,
    fullscreen: WireFullscreen,
    #[serde(rename = "focusHistoryID", default = "least_recent")]
    focus_history: i64,
}

fn least_recent() -> i64 {
    i64::MAX
}

#[derive(Deserialize)]
struct WireCursor {
    x: f32,
    y: f32,
}

/// The own window as needed to apply a move; never exposed outside the worker.
#[derive(Clone, Debug, PartialEq)]
struct OwnWindow {
    address: String,
    floating: bool,
    fullscreen: bool,
}

#[derive(Debug)]
struct Parsed {
    snapshot: DesktopSnapshot,
    own: Option<OwnWindow>,
}

fn checked_rect(origin: Vec2, size: Vec2) -> Option<ScreenRect> {
    (valid_point(origin)
        && valid_point(size)
        && size.min_element() > 0.
        && valid_point(origin + size))
    .then_some(ScreenRect { origin, size })
}

/// Hyprland addresses are `0x` followed by hex; anything else could alter the
/// dispatcher argument and is rejected.
fn valid_address(address: &str) -> bool {
    address.len() > 2
        && address.len() <= 18
        && address.starts_with("0x")
        && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_json<'a, T: Deserialize<'a>>(json: &'a str, what: &str) -> Result<T, String> {
    if json.len() > MAX_PAYLOAD {
        return Err(format!("Hyprland {what} reply is too large"));
    }
    serde_json::from_str(json).map_err(|_| format!("Invalid Hyprland {what} reply"))
}

fn parse_snapshot(
    monitors: &str,
    clients: &str,
    cursor: &str,
    own_pid: u32,
    own_title: &str,
) -> Result<Parsed, String> {
    let monitors: Vec<WireMonitor> = parse_json(monitors, "monitor")?;
    let clients: Vec<WireClient> = parse_json(clients, "window")?;
    let cursor: WireCursor = parse_json(cursor, "cursor")?;
    if monitors.len() > 64 || clients.len() > MAX_RECORDS {
        return Err("Too many Hyprland surfaces".into());
    }
    let cursor = Vec2::new(cursor.x, cursor.y);
    if !valid_point(cursor) {
        return Err("Invalid Hyprland cursor coordinates".into());
    }

    let mut visible_workspaces = Vec::new();
    let mut work_areas = Vec::new();
    for monitor in monitors.iter().filter(|monitor| !monitor.disabled) {
        if !(monitor.scale.is_finite() && monitor.scale > 0.) {
            continue;
        }
        // Mode size is in physical pixels; odd transforms rotate 90 degrees.
        let mut size = Vec2::new(monitor.width, monitor.height) / monitor.scale;
        if monitor.transform % 2 == 1 {
            size = Vec2::new(size.y, size.x);
        }
        let [left, top, right, bottom] = monitor.reserved;
        if !monitor
            .reserved
            .iter()
            .all(|edge| edge.is_finite() && *edge >= 0.)
        {
            continue;
        }
        let origin = Vec2::new(monitor.x + left, monitor.y + top);
        let area = size - Vec2::new(left + right, top + bottom);
        if let Some(rect) = checked_rect(origin, area) {
            visible_workspaces.push(monitor.active_workspace.id);
            // Hyprland reports no special workspace as id 0.
            if let Some(special) = &monitor.special_workspace
                && special.id != 0
            {
                visible_workspaces.push(special.id);
            }
            work_areas.push(DesktopSurface {
                id: SurfaceId::WorkArea(surface_id(&format!("monitor-{}", monitor.id))),
                rect,
                z_order: i32::MAX,
            });
        }
    }

    let mut own = None;
    let mut own_window_rect = None;
    let mut windows = Vec::new();
    for client in &clients {
        if !valid_address(&client.address) {
            continue;
        }
        let rect = checked_rect(Vec2::from_array(client.at), Vec2::from_array(client.size));
        if i64::from(own_pid) == client.pid && client.title == own_title {
            if own.is_some() {
                // A pid and title must identify exactly one window; never
                // move a sibling window of this process.
                return Err("Ambiguous companion window in Hyprland".into());
            }
            own = Some(OwnWindow {
                address: client.address.clone(),
                floating: client.floating,
                fullscreen: client.fullscreen.active(),
            });
            own_window_rect = rect;
        }
        if !client.mapped || client.hidden || !visible_workspaces.contains(&client.workspace.id) {
            continue;
        }
        if let Some(rect) = rect {
            windows.push((client, rect));
        }
    }
    // Hyprland exposes no stacking order. Approximate it: fullscreen above
    // floating above tiled (tiled windows do not overlap), then most recently
    // focused first. Smaller z_order is on top.
    windows.sort_by_key(|(client, _)| {
        let layer = if client.fullscreen.active() {
            0
        } else if client.floating {
            1
        } else {
            2
        };
        (layer, client.focus_history)
    });
    let mut surfaces: Vec<DesktopSurface> = windows
        .into_iter()
        .enumerate()
        .map(|(index, (client, rect))| DesktopSurface {
            id: SurfaceId::Window(surface_id(&client.address)),
            rect,
            z_order: index as i32,
        })
        .collect();
    surfaces.extend(work_areas);
    Ok(Parsed {
        snapshot: DesktopSnapshot {
            received_at: Instant::now(),
            surfaces,
            own_window_ids: own
                .as_ref()
                .map(|own| surface_id(&own.address))
                .into_iter()
                .collect(),
            own_window_rect,
            // Hyprland does not report whether a native move is in progress.
            own_interactive_move: false,
            cursor,
        },
        own,
    })
}

/// Dispatcher command for a validated move, clamped to stay reachable.
fn move_command(target: Vec2, own: &OwnWindow, snapshot: &DesktopSnapshot) -> Option<String> {
    if !own.floating || own.fullscreen || !valid_address(&own.address) || !valid_point(target) {
        return None;
    }
    let size = snapshot.own_window_rect?.size;
    let best = snapshot
        .surfaces
        .iter()
        .filter(|surface| matches!(surface.id, SurfaceId::WorkArea(_)))
        .map(|area| {
            let r = area.rect;
            let low = r.origin - size + REACHABLE;
            let high = r.origin + r.size - REACHABLE;
            target.max(low).min(high)
        })
        .min_by(|a, b| {
            a.distance_squared(target)
                .total_cmp(&b.distance_squared(target))
        })?
        .round();
    (best.abs().max_element() <= MAX_COORDINATE).then(|| {
        format!(
            "dispatch movewindowpixel exact {} {},address:{}",
            best.x as i64, best.y as i64, own.address
        )
    })
}

/// Only an exact instance-signature directory name is accepted as a path part.
fn valid_signature(signature: &str) -> bool {
    !signature.is_empty()
        && signature.len() <= 128
        && signature
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether this process was started inside a Hyprland session. Only `start`
/// authenticates the socket; this never performs I/O.
pub fn hyprland_session() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok_and(|value| valid_signature(&value))
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::UnixStream,
        },
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    #[derive(Default)]
    struct Shared {
        snapshot: Option<(DesktopSnapshot, Instant)>,
        own: Option<OwnWindow>,
        pending_move: Option<Vec2>,
        error: Option<String>,
    }

    /// One request per connection; Hyprland closes after replying.
    pub(super) fn request(socket: &Path, command: &str) -> Result<String, String> {
        let io = |error: std::io::Error| format!("Hyprland socket: {error}");
        let mut stream = UnixStream::connect(socket).map_err(io)?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(io)?;
        stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(io)?;
        stream.write_all(command.as_bytes()).map_err(io)?;
        let mut reply = Vec::new();
        stream
            .take(MAX_PAYLOAD as u64 + 1)
            .read_to_end(&mut reply)
            .map_err(io)?;
        if reply.len() > MAX_PAYLOAD {
            return Err("Hyprland reply is too large".into());
        }
        String::from_utf8(reply).map_err(|_| "Hyprland reply is not UTF-8".into())
    }

    /// The runtime directory and socket must belong to this user, so another
    /// account cannot impersonate the compositor.
    fn authenticated_socket() -> Result<PathBuf, String> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .ok()
            .filter(|value| valid_signature(value))
            .ok_or("Window surfaces on Hyprland require a Hyprland session")?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("XDG_RUNTIME_DIR is unavailable")?;
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        let runtime_meta = std::fs::metadata(&runtime).map_err(|error| error.to_string())?;
        if runtime_meta.uid() != uid || runtime_meta.permissions().mode() & 0o077 != 0 {
            return Err("XDG_RUNTIME_DIR is not private to this user".into());
        }
        let socket = runtime.join("hypr").join(signature).join(".socket.sock");
        let meta = std::fs::symlink_metadata(&socket)
            .map_err(|_| "No Hyprland socket for this session")?;
        if !meta.file_type().is_socket() || meta.uid() != uid {
            return Err("Hyprland socket is not owned by this user".into());
        }
        Ok(socket)
    }

    fn poll(socket: &Path, pid: u32, title: &str, shared: &Mutex<Shared>) -> Result<(), String> {
        let parsed = parse_snapshot(
            &request(socket, "j/monitors")?,
            &request(socket, "j/clients")?,
            &request(socket, "j/cursorpos")?,
            pid,
            title,
        )?;
        let command = {
            let mut state = shared.lock().map_err(|_| "Desktop bridge unavailable")?;
            let command = state
                .pending_move
                .take()
                .and_then(|target| move_command(target, parsed.own.as_ref()?, &parsed.snapshot));
            state.snapshot = Some((parsed.snapshot, Instant::now()));
            state.own = parsed.own;
            state.error = None;
            command
        };
        if let Some(command) = command {
            let reply = request(socket, &command)?;
            if reply.trim() != "ok" {
                return Err("Hyprland refused to move the companion window".into());
            }
        }
        Ok(())
    }

    /// Keep alive only while the user has enabled desktop window surfaces.
    /// Create it off the render thread; drop stops the poller within one tick.
    pub struct HyprlandDesktopBridge {
        shared: Arc<Mutex<Shared>>,
        stop: Arc<AtomicBool>,
    }

    impl HyprlandDesktopBridge {
        pub fn start(own_window_title: &str) -> Result<Self, String> {
            if own_window_title.is_empty() || own_window_title.len() > 512 {
                return Err("A bounded companion window title is required".into());
            }
            let socket = authenticated_socket()?;
            Self::start_at(socket, own_window_title)
                .map_err(|error| format!("Could not enable Hyprland window surfaces: {error}"))
        }

        pub(super) fn start_at(socket: PathBuf, own_window_title: &str) -> Result<Self, String> {
            let version = request(&socket, "j/version")?;
            let version: serde_json::Value = parse_json(&version, "version")?;
            if version.get("tag").and_then(|tag| tag.as_str()).is_none() {
                return Err("Socket did not answer as Hyprland".into());
            }
            let shared = Arc::new(Mutex::new(Shared::default()));
            let stop = Arc::new(AtomicBool::new(false));
            let title = own_window_title.to_owned();
            let pid = std::process::id();
            let (worker_shared, worker_stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("hyprland-geometry".into())
                .spawn(move || {
                    while !worker_stop.load(Ordering::Relaxed) {
                        let started = Instant::now();
                        if let Err(error) = poll(&socket, pid, &title, &worker_shared)
                            && let Ok(mut state) = worker_shared.lock()
                        {
                            state.snapshot = None;
                            state.own = None;
                            state.pending_move = None;
                            state.error = Some(error);
                        }
                        std::thread::sleep(POLL_INTERVAL.saturating_sub(started.elapsed()));
                    }
                })
                .map_err(|error| error.to_string())?;
            Ok(Self { shared, stop })
        }

        pub fn snapshot(&self) -> Result<DesktopSnapshot, String> {
            let state = self
                .shared
                .lock()
                .map_err(|_| "Desktop bridge unavailable")?;
            if let Some(error) = &state.error {
                return Err(error.clone());
            }
            match &state.snapshot {
                Some((snapshot, at)) if at.elapsed() <= SNAPSHOT_TTL => Ok(snapshot.clone()),
                Some(_) => {
                    Err("Hyprland geometry snapshot expired; window sitting is paused".into())
                }
                None => Err("Waiting for the first Hyprland geometry snapshot".into()),
            }
        }

        /// Latest request wins; applied on the next poll to this process's
        /// exact floating window, keeping a reachable portion on a monitor.
        pub fn request_move(&self, client_origin: Vec2) -> Result<(), String> {
            if !valid_point(client_origin) {
                return Err("Invalid companion window origin".into());
            }
            let mut state = self
                .shared
                .lock()
                .map_err(|_| "Desktop bridge unavailable")?;
            let fresh = state.snapshot.as_ref().is_some_and(|(snapshot, at)| {
                at.elapsed() <= SNAPSHOT_TTL && snapshot.own_window_rect.is_some()
            });
            if !fresh {
                state.pending_move = None;
                return Err("Current companion window geometry is required to move it".into());
            }
            if !state
                .own
                .as_ref()
                .is_some_and(|own| own.floating && !own.fullscreen)
            {
                state.pending_move = None;
                return Err("Hyprland can only move the companion while it floats".into());
            }
            state.pending_move = Some(client_origin);
            Ok(())
        }
    }

    impl Drop for HyprlandDesktopBridge {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::HyprlandDesktopBridge;

#[cfg(not(target_os = "linux"))]
pub struct HyprlandDesktopBridge;

#[cfg(not(target_os = "linux"))]
impl HyprlandDesktopBridge {
    pub fn start(_: &str) -> Result<Self, String> {
        Err("Window surfaces on Hyprland require Linux".into())
    }
    pub fn snapshot(&self) -> Result<DesktopSnapshot, String> {
        Err("Hyprland desktop bridge is unavailable on this platform".into())
    }
    pub fn request_move(&self, _: Vec2) -> Result<(), String> {
        Err("Hyprland desktop bridge is unavailable on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = r#"[
        {"id":0,"name":"DP-1","x":0,"y":0,"width":3840,"height":2160,"scale":2.0,
         "transform":0,"reserved":[0,30,0,0],"activeWorkspace":{"id":1,"name":"1"},
         "specialWorkspace":{"id":0,"name":""},"disabled":false},
        {"id":1,"name":"HDMI-A-1","x":1920,"y":0,"width":1920,"height":1080,"scale":1.0,
         "transform":1,"reserved":[0,0,0,0],"activeWorkspace":{"id":2,"name":"2"},
         "specialWorkspace":{"id":-98,"name":"special:magic"}}
    ]"#;

    fn client(address: &str, at: [i32; 2], size: [i32; 2], workspace: i64, extra: &str) -> String {
        let mut value = serde_json::json!({
            "address": address, "mapped": true, "hidden": false, "at": at, "size": size,
            "workspace": {"id": workspace, "name": "w"}, "floating": false, "pid": 1,
            "title": "Other", "fullscreen": 0, "focusHistoryID": 5
        });
        let overrides: serde_json::Value = serde_json::from_str(&format!("{{{extra}}}")).unwrap();
        for (key, field) in overrides.as_object().unwrap() {
            value[key] = field.clone();
        }
        value.to_string()
    }

    fn own(floating: bool) -> String {
        format!(
            r#"{{"address":"0xabc","mapped":true,"hidden":false,"at":[100,200],
            "size":[300,500],"workspace":{{"id":1,"name":"1"}},"floating":{floating},
            "pid":4242,"title":"Waifu Studio","fullscreen":false,"focusHistoryID":0}}"#
        )
    }

    fn parse(clients: &str) -> Result<Parsed, String> {
        parse_snapshot(
            MONITORS,
            clients,
            r#"{"x":-5,"y":12.5}"#,
            4242,
            "Waifu Studio",
        )
    }

    #[test]
    fn logical_work_areas_respect_scale_transform_and_reserved_space() {
        let parsed = parse("[]").unwrap();
        let areas: Vec<_> = parsed.snapshot.surfaces.iter().map(|s| s.rect).collect();
        assert_eq!(areas.len(), 2);
        assert_eq!(areas[0].origin, Vec2::new(0., 30.));
        assert_eq!(areas[0].size, Vec2::new(1920., 1050.));
        assert_eq!(areas[1].origin, Vec2::new(1920., 0.));
        assert_eq!(areas[1].size, Vec2::new(1080., 1920.));
        assert_eq!(parsed.snapshot.cursor, Vec2::new(-5., 12.5));
        assert!(parsed.own.is_none() && parsed.snapshot.own_window_rect.is_none());
    }

    #[test]
    fn only_visible_workspace_windows_are_stacked_fullscreen_floating_tiled() {
        let clients = format!(
            "[{},{},{},{},{},{}]",
            client("0x1", [0, 30], [960, 1050], 1, ""),
            client(
                "0x2",
                [10, 40],
                [400, 300],
                1,
                r#""floating":true,"focusHistoryID":3"#
            ),
            client("0x3", [1920, 0], [1080, 1920], 2, r#""fullscreen":2"#),
            client(
                "0x4",
                [1950, 50],
                [200, 200],
                -98,
                r#""floating":true,"focusHistoryID":1"#
            ),
            client("0x5", [0, 0], [100, 100], 7, ""),
            client("0x6", [0, 0], [100, 100], 1, r#""hidden":true"#),
        );
        let parsed = parse(&clients).unwrap();
        let ids: Vec<_> = parsed.snapshot.surfaces.iter().map(|s| s.id).collect();
        let window = |address: &str| SurfaceId::Window(surface_id(address));
        assert_eq!(
            ids[..4],
            [window("0x3"), window("0x4"), window("0x2"), window("0x1")]
        );
        assert!(matches!(ids[4], SurfaceId::WorkArea(_)));
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn own_window_requires_exact_pid_and_title_and_fails_closed_when_ambiguous() {
        let parsed = parse(&format!("[{}]", own(true))).unwrap();
        assert_eq!(parsed.snapshot.own_window_ids, vec![surface_id("0xabc")]);
        assert_eq!(
            parsed.snapshot.own_window_rect.unwrap().origin,
            Vec2::new(100., 200.)
        );
        let impostor = own(true).replace("4242", "4243");
        assert!(parse(&format!("[{impostor}]")).unwrap().own.is_none());
        let twin = own(true).replace("0xabc", "0xdef");
        assert!(parse(&format!("[{},{twin}]", own(true))).is_err());
    }

    #[test]
    fn rejects_invalid_geometry_addresses_and_oversized_replies() {
        let bad = format!(
            "[{},{}]",
            client("0x1", [0, 0], [-1, 10], 1, ""),
            client("0x1 ,address:0x2", [0, 0], [10, 10], 1, "")
        );
        assert_eq!(parse(&bad).unwrap().snapshot.surfaces.len(), 2);
        assert!(parse_snapshot(MONITORS, "[]", r#"{"x":1e30,"y":0}"#, 1, "t").is_err());
        assert!(parse(&" ".repeat(MAX_PAYLOAD + 1)).is_err());
        assert!(parse("{}").is_err());
        assert!(!valid_signature("../../etc") && valid_signature("abc_123-x"));
    }

    #[test]
    fn moves_only_floating_own_window_and_keeps_it_reachable() {
        let parsed = parse(&format!("[{}]", own(true))).unwrap();
        let own_window = parsed.own.clone().unwrap();
        assert_eq!(
            move_command(Vec2::new(500.4, 600.6), &own_window, &parsed.snapshot).unwrap(),
            "dispatch movewindowpixel exact 500 601,address:0xabc"
        );
        // Far off-screen requests clamp to the nearest monitor edge.
        assert_eq!(
            move_command(Vec2::new(-9000., 300.), &own_window, &parsed.snapshot).unwrap(),
            "dispatch movewindowpixel exact -276 300,address:0xabc"
        );
        let tiled = OwnWindow {
            floating: false,
            ..own_window.clone()
        };
        assert!(move_command(Vec2::ZERO, &tiled, &parsed.snapshot).is_none());
        let fullscreen = OwnWindow {
            fullscreen: true,
            ..own_window
        };
        assert!(move_command(Vec2::ZERO, &fullscreen, &parsed.snapshot).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fake_socket_round_trip_polls_and_dispatches_queued_move() {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixListener,
            sync::{Arc, Mutex},
        };
        let directory = std::env::temp_dir().join(format!("hypr-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let socket = directory.join(".socket.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let commands = Arc::new(Mutex::new(Vec::<String>::new()));
        let own_client = own(true).replace("4242", &std::process::id().to_string());
        let log = Arc::clone(&commands);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0; 512];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let command = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let reply = match command.as_str() {
                    "j/version" => r#"{"tag":"v0.50.0"}"#.to_string(),
                    "j/monitors" => MONITORS.to_string(),
                    "j/clients" => format!("[{own_client}]"),
                    "j/cursorpos" => r#"{"x":1,"y":2}"#.to_string(),
                    _ => "ok".to_string(),
                };
                log.lock().unwrap().push(command);
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        let bridge = linux::HyprlandDesktopBridge::start_at(socket, "Waifu Studio").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            match bridge.snapshot() {
                Ok(snapshot) => break snapshot,
                Err(_) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
                Err(error) => panic!("{error}"),
            }
        };
        assert_eq!(snapshot.cursor, Vec2::new(1., 2.));
        assert!(snapshot.own_window_rect.is_some());
        bridge.request_move(Vec2::new(40., 50.)).unwrap();
        let expected = "dispatch movewindowpixel exact 40 50,address:0xabc";
        while !commands.lock().unwrap().iter().any(|c| c == expected) {
            assert!(Instant::now() < deadline, "move was not dispatched");
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(bridge.request_move(Vec2::new(f32::NAN, 0.)).is_err());
        drop(bridge);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
