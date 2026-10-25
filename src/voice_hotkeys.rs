//! Voice actions shared by keyboard shortcuts, desktop menus and Voice settings.
use crate::studio::{Studio, Tab, ViewMode};
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, mpsc},
};
use waifu_studio::{listen::ListeningState, settings_kit as kit};

#[cfg(target_os = "linux")]
mod portal;
#[cfg(windows)]
#[path = "voice_hotkeys/windows.rs"]
mod portal;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    Microphone,
    Voice,
    Stop,
    Settings,
}
impl Action {
    pub const ALL: [Self; 4] = [Self::Microphone, Self::Voice, Self::Stop, Self::Settings];
    pub fn id(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Voice => "voice",
            Self::Stop => "stop",
            Self::Settings => "settings",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Microphone => "Start / stop microphone",
            Self::Voice => "Toggle voice replies",
            Self::Stop => "Stop voice and cancel recording",
            Self::Settings => "Open voice settings",
        }
    }
    pub fn preferred_trigger(self) -> &'static str {
        match self {
            Self::Microphone => "CTRL+SHIFT+space",
            Self::Voice => "CTRL+SHIFT+m",
            Self::Stop => "CTRL+SHIFT+period",
            Self::Settings => "CTRL+SHIFT+comma",
        }
    }
    pub fn shortcut(self) -> &'static str {
        match self {
            Self::Microphone => "Ctrl + Shift + Space",
            Self::Voice => "Ctrl + Shift + M",
            Self::Stop => "Ctrl + Shift + .",
            Self::Settings => "Ctrl + Shift + ,",
        }
    }
    fn key(self) -> KeyCode {
        match self {
            Self::Microphone => KeyCode::Space,
            Self::Voice => KeyCode::KeyM,
            Self::Stop => KeyCode::Period,
            Self::Settings => KeyCode::Comma,
        }
    }
}

pub(super) enum PortalEvent {
    Ready(Vec<(Action, String)>),
    Trigger(Action),
    Closed(String),
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    global: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { global: true }
    }
}

#[derive(Resource)]
pub(crate) struct VoiceHotkeys {
    preferences: Preferences,
    path: Option<PathBuf>,
    allow_global: bool,
    attempted: bool,
    generation: u64,
    tx: mpsc::SyncSender<(u64, PortalEvent)>,
    rx: Mutex<mpsc::Receiver<(u64, PortalEvent)>>,
    #[cfg(any(target_os = "linux", windows))]
    worker: Option<portal::PortalWorker>,
    bindings: HashMap<Action, String>,
    pending: Vec<Action>,
    pub status: String,
    setting_up: bool,
    reveal: bool,
}
impl Default for VoiceHotkeys {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel(64);
        Self {
            preferences: Preferences::default(),
            path: None,
            allow_global: true,
            attempted: false,
            generation: 0,
            tx,
            rx: Mutex::new(rx),
            #[cfg(any(target_os = "linux", windows))]
            worker: None,
            bindings: HashMap::new(),
            pending: Vec::new(),
            status: "Shortcuts work while Waifu Studio is focused.".into(),
            setting_up: false,
            reveal: false,
        }
    }
}
impl VoiceHotkeys {
    pub fn load(path: PathBuf, allow_global: bool) -> Self {
        use std::io::Read;
        let read = || -> std::io::Result<Vec<u8>> {
            let file = std::fs::File::open(&path)?;
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::other("Shortcut settings must be a file"));
            }
            let mut bytes = Vec::new();
            file.take(4097).read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        let preferences = read()
            .ok()
            .filter(|bytes| bytes.len() <= 4096)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            preferences,
            path: Some(path),
            allow_global,
            ..Default::default()
        }
    }
    pub fn shortcut(&self, action: Action) -> &str {
        self.bindings
            .get(&action)
            .map_or(action.shortcut(), String::as_str)
    }
    pub fn queue(&mut self, action: Action) {
        if self.pending.len() < Action::ALL.len() && !self.pending.contains(&action) {
            self.pending.push(action);
        }
    }
    fn save(&mut self) {
        if let Some(path) = &self.path
            && let Err(error) = serde_json::to_vec_pretty(&self.preferences)
                .map_err(std::io::Error::other)
                .and_then(|bytes| std::fs::write(path, bytes))
        {
            self.status = format!("Could not save shortcut preferences: {error}");
        }
    }
    fn start_global(&mut self) {
        if !self.allow_global {
            return;
        }
        self.stop_global();
        self.attempted = true;
        self.preferences.global = true;
        self.save();
        #[cfg(any(target_os = "linux", windows))]
        match portal::start(self.generation, self.tx.clone()) {
            Ok(worker) => {
                self.worker = Some(worker);
                self.setting_up = true;
                self.status = if cfg!(windows) {
                    "Registering Windows shortcuts."
                } else {
                    "Choose shortcuts in your desktop's dialog."
                }
                .into();
            }
            Err(error) => self.status = error,
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            self.status = "System-wide shortcuts are not available on this platform yet. Focused shortcuts still work.".into();
        }
    }
    fn stop_global(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        #[cfg(any(target_os = "linux", windows))]
        {
            self.worker = None;
        }
        self.bindings.clear();
        self.pending.clear();
        self.setting_up = false;
    }
    fn disable_global(&mut self) {
        self.stop_global();
        self.preferences.global = false;
        self.attempted = true;
        self.status = "Shortcuts work while Waifu Studio is focused.".into();
        self.save();
    }
    fn poll(&mut self) {
        if self.allow_global && self.preferences.global && !self.attempted {
            self.start_global();
        }
        let messages: Vec<_> = self
            .rx
            .lock()
            .map(|rx| rx.try_iter().take(64).collect())
            .unwrap_or_default();
        for (generation, event) in messages {
            if generation != self.generation {
                continue;
            }
            match event {
                PortalEvent::Ready(bindings) => {
                    self.bindings = bindings
                        .into_iter()
                        .filter(|(_, text)| !text.trim().is_empty())
                        .collect();
                    self.setting_up = false;
                    self.status = if self.bindings.is_empty() {
                        "No global shortcuts were assigned. Focused shortcuts still work.".into()
                    } else if self.bindings.len() < Action::ALL.len() {
                        format!(
                            "{} of {} global shortcuts are active. Unavailable bindings may belong to another app; they still work when Waifu Studio is focused.",
                            self.bindings.len(),
                            Action::ALL.len()
                        )
                    } else {
                        "Global shortcuts are active, including while another app is focused."
                            .into()
                    };
                }
                PortalEvent::Trigger(action) if self.bindings.contains_key(&action) => {
                    self.queue(action)
                }
                PortalEvent::Trigger(_) => {}
                PortalEvent::Closed(message) => {
                    self.stop_global();
                    self.status = format!("{message} Focused shortcuts still work.");
                }
            }
        }
    }
}

pub struct VoiceHotkeysPlugin;
impl Plugin for VoiceHotkeysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VoiceHotkeys>().add_systems(
            Update,
            process
                .after(crate::studio::poll_input)
                .before(crate::studio::poll_assistant),
        );
    }
}

fn focused_action(keys: &ButtonInput<KeyCode>, action: Action) -> bool {
    (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
        && (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
        && ![
            KeyCode::AltLeft,
            KeyCode::AltRight,
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
        ]
        .iter()
        .any(|key| keys.pressed(*key))
        && keys.just_pressed(action.key())
}

fn process(
    mut hotkeys: ResMut<VoiceHotkeys>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    mut studio: ResMut<Studio>,
    mut listening: ResMut<ListeningState>,
) {
    hotkeys.poll();
    if windows.iter().any(|window| window.focused) {
        for action in Action::ALL {
            // The portal owns active global bindings even in our own window;
            // also handling the native key would toggle the same action twice.
            if !hotkeys.bindings.contains_key(&action) && focused_action(&keys, action) {
                hotkeys.queue(action);
            }
        }
    }
    for action in std::mem::take(&mut hotkeys.pending) {
        if action == Action::Settings {
            hotkeys.reveal = true;
        }
        apply(action, &mut studio, &mut listening);
    }
}

fn apply(action: Action, studio: &mut Studio, listening: &mut ListeningState) {
    match action {
        Action::Microphone => {
            if !listening.config.enabled {
                listening.status =
                    "Enable microphone transcription in Voice settings first.".into();
            } else if listening.is_recording() {
                listening.stop();
            } else if listening.is_busy() {
                listening.status =
                    "Finishing the current recording or transcript. Use Stop voice to cancel."
                        .into();
            } else if studio.library.selected.is_none() {
                listening.status = "Choose a companion before recording.".into();
            } else {
                listening.start_for(studio.library.selected, studio.composer.trim().is_empty());
            }
        }
        Action::Voice => {
            studio.library.voice.enabled = !studio.library.voice.enabled;
            if !studio.library.voice.enabled {
                studio.stop_voice();
            }
            studio.save();
        }
        Action::Stop => {
            studio.stop_voice();
            listening.cancel();
        }
        Action::Settings => {
            studio.tab = Tab::Voice;
            studio.mode = ViewMode::Studio;
        }
    }
}

pub(crate) fn settings_ui(ui: &mut egui::Ui, state: &mut VoiceHotkeys) {
    let top = ui.next_widget_position();
    kit::section(ui, "Keyboard shortcuts");
    kit::card(ui, |card| {
        for action in Action::ALL {
            card.row(action.label(), "", |ui| {
                ui.label(state.shortcut(action));
                if state.preferences.global && !state.setting_up {
                    ui.weak(if state.bindings.contains_key(&action) {
                        "Global"
                    } else {
                        "Focused only"
                    });
                }
            });
        }
        card.row(
            "Use shortcuts in other apps",
            if cfg!(windows) {
                "Windows registers the default keys; conflicts are shown per binding."
            } else {
                "Your desktop lets you approve or change the global keys."
            },
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            state.allow_global && !state.setting_up,
                            egui::Button::new(if state.bindings.is_empty() {
                                "Set up global shortcuts…"
                            } else {
                                "Change…"
                            }),
                        )
                        .clicked()
                    {
                        state.start_global();
                    }
                    if state.preferences.global && ui.small_button("Disable").clicked() {
                        state.disable_global();
                    }
                });
            },
        );
        card.block(|ui| {
            kit::note(ui, &state.status);
        });
    });
    if std::mem::take(&mut state.reveal) {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_shortcuts_default_on_and_explicit_disable_is_preserved() {
        assert!(Preferences::default().global);
        assert!(serde_json::from_str::<Preferences>("{}").unwrap().global);
        assert!(
            !serde_json::from_str::<Preferences>(r#"{"global":false}"#)
                .unwrap()
                .global
        );
    }

    #[test]
    fn modifier_chords_are_specific_and_do_not_repeat_from_held_keys() {
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::ShiftRight);
        keys.press(KeyCode::Space);
        assert!(focused_action(&keys, Action::Microphone));
        assert!(!focused_action(&keys, Action::Voice));
        keys.clear();
        assert!(!focused_action(&keys, Action::Microphone));
        keys.release(KeyCode::Space);
        keys.press(KeyCode::Space);
        keys.press(KeyCode::AltLeft);
        assert!(!focused_action(&keys, Action::Microphone));
    }
    #[test]
    fn stale_global_events_and_unassigned_actions_cannot_trigger_voice() {
        let mut state = VoiceHotkeys {
            allow_global: false,
            ..Default::default()
        };
        state
            .tx
            .try_send((
                0,
                PortalEvent::Ready(vec![(Action::Voice, "Custom chord".into())]),
            ))
            .unwrap();
        state.poll();
        assert_eq!(state.shortcut(Action::Voice), "Custom chord");
        state
            .tx
            .try_send((0, PortalEvent::Trigger(Action::Voice)))
            .unwrap();
        state
            .tx
            .try_send((0, PortalEvent::Trigger(Action::Microphone)))
            .unwrap();
        state.poll();
        assert_eq!(state.pending, vec![Action::Voice]);
        state.pending.clear();
        state.disable_global();
        state
            .tx
            .try_send((0, PortalEvent::Trigger(Action::Voice)))
            .unwrap();
        state.poll();
        assert!(state.pending.is_empty() && state.bindings.is_empty());
    }
    #[test]
    fn focused_and_global_routes_do_not_double_toggle_voice() {
        let dir = std::env::temp_dir().join(format!("waifu-key-routing-{}", uuid::Uuid::new_v4()));
        let (store, library) = waifu_studio::data::Store::open(dir.clone()).unwrap();
        let mut studio = Studio::new(store, library);
        studio.library.voice.enabled = true;
        let mut app = App::new();
        app.insert_resource(studio)
            .insert_resource(VoiceHotkeys {
                allow_global: false,
                ..Default::default()
            })
            .init_resource::<ListeningState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, process);
        let window = app
            .world_mut()
            .spawn(Window {
                focused: false,
                ..Default::default()
            })
            .id();
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
            keys.press(KeyCode::ShiftLeft);
            keys.press(KeyCode::KeyM);
        }
        app.update();
        assert!(
            app.world().resource::<Studio>().library.voice.enabled,
            "unfocused keys are ignored"
        );
        app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
        app.update();
        assert!(!app.world().resource::<Studio>().library.voice.enabled);
        {
            let state = app.world().resource::<VoiceHotkeys>();
            state
                .tx
                .try_send((
                    0,
                    PortalEvent::Ready(vec![(Action::Voice, "Assigned by desktop".into())]),
                ))
                .unwrap();
            state
                .tx
                .try_send((0, PortalEvent::Trigger(Action::Voice)))
                .unwrap();
        }
        app.update();
        assert!(
            app.world().resource::<Studio>().library.voice.enabled,
            "portal and native key must toggle only once"
        );
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        {
            let state = app.world().resource::<VoiceHotkeys>();
            state
                .tx
                .try_send((0, PortalEvent::Trigger(Action::Voice)))
                .unwrap();
        }
        app.update();
        assert!(
            !app.world().resource::<Studio>().library.voice.enabled,
            "global actions work without focus"
        );
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn stop_voice_cancels_generation_and_settings_opens_voice_without_recording() {
        let dir = std::env::temp_dir().join(format!("waifu-hotkeys-{}", uuid::Uuid::new_v4()));
        let (store, library) = waifu_studio::data::Store::open(dir.clone()).unwrap();
        let mut studio = Studio::new(store, library);
        let mut listening = ListeningState::default();
        let generation = studio.speech_generation();
        apply(Action::Stop, &mut studio, &mut listening);
        assert_ne!(studio.speech_generation(), generation);
        assert!(!studio.voice_busy && !listening.is_recording());
        studio.mode = ViewMode::Desktop;
        apply(Action::Settings, &mut studio, &mut listening);
        assert_eq!(studio.mode, ViewMode::Studio);
        assert_eq!(studio.tab, Tab::Voice);
        listening.set_enabled(false);
        apply(Action::Microphone, &mut studio, &mut listening);
        assert!(!listening.is_busy());
        assert!(listening.status.starts_with("Enable microphone"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
