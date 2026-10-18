//! Explicitly enabled, runtime-only KWin 6 desktop geometry bridge.
//!
//! Calling `start` is the only operation that connects to D-Bus or loads a
//! compositor script. There is no persistent KWin configuration, autostart,
//! title collection, screenshot, or generic command execution. Drop unloads
//! the script. A script watchdog unloads it when the app no longer answers.
//!
//! KWin API references (checked against official Plasma/6.5 source):
//! <https://develop.kde.org/docs/plasma/kwin/api/>
//! <https://invent.kde.org/plasma/kwin/-/blob/Plasma/6.5/src/scripting/scripting.cpp>
//! <https://invent.kde.org/plasma/kwin/-/blob/Plasma/6.5/src/scripting/workspace_wrapper.h>

use crate::desktop_interaction::{DesktopSurface, ScreenRect, SurfaceId};
use bevy::prelude::Vec2;
use serde::Deserialize;
use std::time::{Duration, Instant};

mod hyprland;
pub use hyprland::{HyprlandDesktopBridge, hyprland_session};

const MAX_PAYLOAD: usize = 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const MAX_COORDINATE: f32 = 16_777_216.;
const SNAPSHOT_TTL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct DesktopSnapshot {
    /// Time this distinct sample reached the bridge, preserved when cloned.
    /// Native drag completion must not reuse a cached sample from before drag.
    pub received_at: Instant,
    /// Topmost window first; work areas follow all windows.
    pub surfaces: Vec<DesktopSurface>,
    pub own_window_ids: Vec<u64>,
    /// The logical client rectangle, including on Wayland where winit cannot query it.
    pub own_window_rect: Option<ScreenRect>,
    /// KWin owns an interactive native drag and may consume pointer release.
    pub own_interactive_move: bool,
    pub cursor: Vec2,
}

#[derive(Deserialize)]
struct WireRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl WireRect {
    fn checked(&self) -> Option<ScreenRect> {
        let origin = Vec2::new(self.x, self.y);
        let size = Vec2::new(self.width, self.height);
        (valid_point(origin)
            && valid_point(size)
            && size.min_element() > 0.
            && valid_point(origin + size))
        .then_some(ScreenRect { origin, size })
    }
}

#[derive(Deserialize)]
struct WireSurface {
    id: String,
    rect: WireRect,
}

#[derive(Deserialize)]
struct WireSnapshot {
    windows: Vec<WireSurface>,
    work_areas: Vec<WireSurface>,
    own_id: Option<String>,
    own_rect: Option<WireRect>,
    #[serde(default)]
    own_interactive_move: bool,
    cursor: [f32; 2],
}

fn valid_point(point: Vec2) -> bool {
    point.is_finite() && point.abs().max_element() <= MAX_COORDINATE
}

fn surface_id(id: &str) -> u64 {
    // Stable within the desktop session, unlike enumeration indices. KWin UUID
    // strings exceed JavaScript's exact integer range, so never parse as numbers.
    id.as_bytes().iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn parse_snapshot(json: &str) -> Result<DesktopSnapshot, String> {
    if json.len() > MAX_PAYLOAD {
        return Err("KWin geometry snapshot is too large".into());
    }
    let wire: WireSnapshot =
        serde_json::from_str(json).map_err(|_| "Invalid KWin geometry snapshot".to_string())?;
    if wire.windows.len() > MAX_RECORDS || wire.work_areas.len() > 64 {
        return Err("Too many KWin surfaces".into());
    }
    let cursor = Vec2::from_array(wire.cursor);
    if !valid_point(cursor) {
        return Err("Invalid KWin cursor coordinates".into());
    }
    let mut surfaces = Vec::with_capacity(wire.windows.len() + wire.work_areas.len());
    for (index, record) in wire.windows.into_iter().enumerate() {
        if record.id.is_empty() || record.id.len() > 256 {
            continue;
        }
        if let Some(rect) = record.rect.checked() {
            surfaces.push(DesktopSurface {
                id: SurfaceId::Window(surface_id(&record.id)),
                rect,
                z_order: index as i32,
            });
        }
    }
    for record in wire.work_areas {
        if record.id.is_empty() || record.id.len() > 256 {
            continue;
        }
        if let Some(rect) = record.rect.checked() {
            surfaces.push(DesktopSurface {
                id: SurfaceId::WorkArea(surface_id(&record.id)),
                rect,
                z_order: i32::MAX,
            });
        }
    }
    let own_window_rect = wire.own_rect.and_then(|rect| rect.checked());
    let own_window_ids = wire.own_id.as_deref().map(surface_id).into_iter().collect();
    Ok(DesktopSnapshot {
        received_at: Instant::now(),
        surfaces,
        own_window_ids,
        own_window_rect,
        own_interactive_move: wire.own_interactive_move,
        cursor,
    })
}

fn session_hints_allow_kwin(session_type: Option<&str>, desktop: Option<&str>) -> bool {
    // App launches through terminals, IDEs and automation may omit XDG hints.
    // Missing metadata must not veto authenticated session-bus discovery.
    let compatible_type = session_type.is_none_or(|value| {
        value.trim().is_empty() || value.trim().eq_ignore_ascii_case("wayland")
    });
    let compatible_desktop = desktop.is_none_or(|value| {
        value.trim().is_empty()
            || value
                .split(':')
                .any(|part| part.trim().eq_ignore_ascii_case("KDE"))
    });
    compatible_type && compatible_desktop
}

/// Whether environment hints permit probing for KWin on Wayland. Missing hints
/// permit discovery; only `start` authenticates the actual KWin bus owner.
pub fn kwin_wayland_session() -> bool {
    cfg!(target_os = "linux")
        && session_hints_allow_kwin(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        )
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::{Arc, Mutex},
    };
    use zbus::blocking::{Connection, Proxy, connection::Builder};

    const SERVICE_PATH: &str = "/org/waifustudio/DesktopBridge";
    const SCRIPT: &str = include_str!("../assets/desktop/kwin-surfaces.js");

    #[derive(Default)]
    struct Shared {
        kwin_owner: Option<String>,
        snapshot: Option<(DesktopSnapshot, Instant)>,
        pending_move: Option<Vec2>,
        error: Option<String>,
    }

    struct GeometryReceiver {
        token: String,
        shared: Arc<Mutex<Shared>>,
    }

    #[zbus::interface(name = "org.waifustudio.DesktopBridge")]
    impl GeometryReceiver {
        fn exchange(
            &self,
            token: &str,
            payload: &str,
            #[zbus(header)] header: zbus::message::Header<'_>,
        ) -> zbus::fdo::Result<String> {
            if token != self.token {
                return Err(zbus::fdo::Error::AccessDenied(
                    "Wrong bridge instance".into(),
                ));
            }
            let mut state = self
                .shared
                .lock()
                .map_err(|_| zbus::fdo::Error::Failed("Desktop bridge unavailable".into()))?;
            if header.sender().map(|sender| sender.as_str()) != state.kwin_owner.as_deref()
                || state.kwin_owner.is_none()
            {
                return Err(zbus::fdo::Error::AccessDenied(
                    "Only this session's KWin may send geometry".into(),
                ));
            }
            let parsed = parse_snapshot(payload);
            match parsed {
                Ok(snapshot) => {
                    state.snapshot = Some((snapshot, Instant::now()));
                    state.error = None;
                }
                Err(error) => {
                    state.snapshot = None;
                    state.pending_move = None;
                    state.error = Some(error);
                    return Ok(json!({ "stop": true }).to_string());
                }
            }
            let movement = state.pending_move.take().map(|point| point.to_array());
            Ok(json!({ "move": movement }).to_string())
        }
    }

    /// Keep alive only while the user has enabled desktop window surfaces.
    /// Create it off the render thread: initial D-Bus methods may take 2 seconds.
    pub struct KwinDesktopBridge {
        connection: Connection,
        shared: Arc<Mutex<Shared>>,
        plugin: String,
        script_dir: PathBuf,
    }

    impl KwinDesktopBridge {
        pub fn start(own_window_title: &str) -> Result<Self, String> {
            if !kwin_wayland_session() {
                return Err("Window surfaces require a KDE Plasma Wayland session".into());
            }
            if own_window_title.is_empty() || own_window_title.len() > 512 {
                return Err("A bounded companion window title is required".into());
            }
            Self::start_inner(own_window_title).map_err(|error| {
                format!("Could not enable temporary KWin window surfaces: {error}")
            })
        }

        fn start_inner(own_window_title: &str) -> Result<Self, String> {
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let service = format!("org.waifustudio.DesktopBridge.i{nonce}");
            let plugin = format!("waifu-studio-temporary-{nonce}");
            let token = uuid::Uuid::new_v4().to_string();
            let shared = Arc::new(Mutex::new(Shared::default()));
            let receiver = GeometryReceiver {
                token: token.clone(),
                shared: Arc::clone(&shared),
            };
            let connection = Builder::session()
                .map_err(|error| error.to_string())?
                .method_timeout(Duration::from_secs(2))
                .name(service.as_str())
                .map_err(|error| error.to_string())?
                .serve_at(SERVICE_PATH, receiver)
                .map_err(|error| error.to_string())?
                .build()
                .map_err(|error| error.to_string())?;
            let bus = Proxy::new(
                &connection,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
            )
            .map_err(|error| error.to_string())?;
            let kwin_owner: String = bus
                .call("GetNameOwner", &("org.kde.KWin",))
                .map_err(|error| error.to_string())?;
            shared
                .lock()
                .map_err(|_| "Desktop bridge unavailable")?
                .kwin_owner = Some(kwin_owner);
            drop(bus);
            // Ephemeral files are private and never live under KWin's scripts
            // directory. An unused script is cleaned up on every error path.
            let script_dir = std::env::temp_dir().join(&plugin);
            fs::create_dir(&script_dir).map_err(|error| error.to_string())?;
            if let Err(error) = fs::set_permissions(&script_dir, fs::Permissions::from_mode(0o700))
            {
                let _ = fs::remove_dir(&script_dir);
                return Err(error.to_string());
            }
            let bridge = Self {
                connection,
                shared,
                plugin,
                script_dir,
            };
            let path = bridge.script_dir.join("main.js");
            let config = json!({
                "service": service, "token": token, "plugin": bridge.plugin,
                "pid": std::process::id(), "title": own_window_title
            });
            fs::write(&path, format!("var bridgeConfig = {config};\n{SCRIPT}"))
                .map_err(|error| error.to_string())?;
            let manager = bridge.manager().map_err(|error| error.to_string())?;
            let script_id: i32 = manager
                .call(
                    "loadScript",
                    &(path.to_string_lossy().as_ref(), bridge.plugin.as_str()),
                )
                .map_err(|error| error.to_string())?;
            if script_id < 0 {
                return Err("KWin refused the temporary script".into());
            }
            let script_path = format!("/Scripting/Script{script_id}");
            let script = Proxy::new(
                &bridge.connection,
                "org.kde.KWin",
                script_path.as_str(),
                "org.kde.kwin.Script",
            )
            .map_err(|error| error.to_string())?;
            let _: () = script.call("run", &()).map_err(|error| error.to_string())?;
            drop(script);
            drop(manager);
            Ok(bridge)
        }

        fn manager(&self) -> zbus::Result<Proxy<'_>> {
            Proxy::new(
                &self.connection,
                "org.kde.KWin",
                "/Scripting",
                "org.kde.kwin.Scripting",
            )
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
                Some(_) => Err("KWin geometry snapshot expired; window sitting is paused".into()),
                None => Err("Waiting for the first KWin geometry snapshot".into()),
            }
        }

        /// Latest request wins; KWin applies it only to this process's exact
        /// titled window, keeping a reachable portion on a connected output.
        /// A successful return queues a request; the next snapshot confirms it.
        pub fn request_move(&self, client_origin: Vec2) -> Result<(), String> {
            if !valid_point(client_origin) {
                return Err("Invalid companion window origin".into());
            }
            let mut state = self
                .shared
                .lock()
                .map_err(|_| "Desktop bridge unavailable")?;
            let Some((snapshot, at)) = &state.snapshot else {
                return Err("Companion window geometry is not available yet".into());
            };
            if at.elapsed() > SNAPSHOT_TTL || snapshot.own_window_rect.is_none() {
                state.pending_move = None;
                return Err("Current companion window geometry is required to move it".into());
            }
            state.pending_move = Some(client_origin);
            Ok(())
        }
    }

    impl Drop for KwinDesktopBridge {
        fn drop(&mut self) {
            if let Ok(manager) = self.manager() {
                let _: zbus::Result<bool> = manager.call("unloadScript", &(self.plugin.as_str(),));
            }
            let _ = fs::remove_file(self.script_dir.join("main.js"));
            let _ = fs::remove_dir(&self.script_dir);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::KwinDesktopBridge;

#[cfg(not(target_os = "linux"))]
pub struct KwinDesktopBridge;

#[cfg(not(target_os = "linux"))]
impl KwinDesktopBridge {
    pub fn start(_: &str) -> Result<Self, String> {
        Err("Window surfaces require KDE Plasma on Linux Wayland".into())
    }
    pub fn snapshot(&self) -> Result<DesktopSnapshot, String> {
        Err("KWin desktop bridge is unavailable on this platform".into())
    }
    pub fn request_move(&self, _: Vec2) -> Result<(), String> {
        Err("KWin desktop bridge is unavailable on this platform".into())
    }
}

/// Compositor-neutral bridge behind the same explicit window-surfaces opt-in.
/// Hyprland is chosen only when this process runs inside a Hyprland session
/// whose socket authenticates; otherwise the KWin bridge applies its own
/// checks. No backend is probed on a compositor it does not belong to.
pub enum DesktopBridge {
    Kwin(KwinDesktopBridge),
    Hyprland(HyprlandDesktopBridge),
}

impl DesktopBridge {
    /// Blocking: may wait on D-Bus or the compositor socket. Call off the
    /// render thread, like `KwinDesktopBridge::start`.
    pub fn start(own_window_title: &str) -> Result<Self, String> {
        if hyprland_session() {
            HyprlandDesktopBridge::start(own_window_title).map(Self::Hyprland)
        } else {
            KwinDesktopBridge::start(own_window_title).map(Self::Kwin)
        }
    }

    pub fn snapshot(&self) -> Result<DesktopSnapshot, String> {
        match self {
            Self::Kwin(bridge) => bridge.snapshot(),
            Self::Hyprland(bridge) => bridge.snapshot(),
        }
    }

    pub fn request_move(&self, client_origin: Vec2) -> Result<(), String> {
        match self {
            Self::Kwin(bridge) => bridge.request_move(client_origin),
            Self::Hyprland(bridge) => bridge.request_move(client_origin),
        }
    }
}

/// Whether environment hints permit probing any supported geometry backend.
pub fn window_surfaces_session() -> bool {
    hyprland_session() || kwin_wayland_session()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_session_hints_allow_discovery_but_explicit_x11_does_not() {
        assert!(session_hints_allow_kwin(None, None));
        assert!(session_hints_allow_kwin(Some(""), Some("")));
        assert!(session_hints_allow_kwin(Some("wayland"), Some("KDE")));
        assert!(session_hints_allow_kwin(
            Some("wayland"),
            Some("plasma:KDE")
        ));
        assert!(!session_hints_allow_kwin(Some("x11"), Some("KDE")));
        assert!(!session_hints_allow_kwin(Some("tty"), None));
        assert!(!session_hints_allow_kwin(Some("wayland"), Some("GNOME")));
    }

    #[test]
    fn translates_uuid_identity_stacking_and_distinct_work_areas() {
        let snapshot = parse_snapshot(
            r#"{
            "windows":[
                {"id":"{upper-uuid}","rect":{"x":-500,"y":100,"width":400,"height":200}},
                {"id":"{lower-uuid}","rect":{"x":-600,"y":120,"width":500,"height":300}}
            ],
            "work_areas":[
                {"id":"screen-a","rect":{"x":-1920,"y":0,"width":1920,"height":1040}},
                {"id":"screen-b","rect":{"x":0,"y":0,"width":2560,"height":1400}}
            ],
            "own_id":"{own-uuid}",
            "own_rect":{"x":50,"y":80,"width":300,"height":500},
            "cursor":[-300,100]
        }"#,
        )
        .unwrap();
        assert_eq!(snapshot.surfaces.len(), 4);
        assert!(snapshot.surfaces[0].z_order < snapshot.surfaces[1].z_order);
        assert!(matches!(snapshot.surfaces[2].id, SurfaceId::WorkArea(_)));
        assert_ne!(snapshot.surfaces[2].id, snapshot.surfaces[3].id);
        assert_eq!(snapshot.own_window_ids, vec![surface_id("{own-uuid}")]);
        assert_eq!(
            snapshot.own_window_rect.unwrap().origin,
            Vec2::new(50., 80.)
        );
        assert_eq!(snapshot.cursor, Vec2::new(-300., 100.));
    }

    #[test]
    fn rejects_invalid_geometry_and_oversized_payloads() {
        let snapshot = parse_snapshot(
            r#"{
            "windows":[{"id":"bad","rect":{"x":0,"y":0,"width":-1,"height":1}}],
            "work_areas":[],"own_id":null,"own_rect":null,"cursor":[0,0]
        }"#,
        )
        .unwrap();
        assert!(snapshot.surfaces.is_empty());
        assert!(parse_snapshot(&" ".repeat(MAX_PAYLOAD + 1)).is_err());
        assert!(
            parse_snapshot(
                r#"{"windows":[],"work_areas":[],"own_id":null,
            "own_rect":null,"cursor":[1e30,0]}"#
            )
            .is_err()
        );
    }
}
