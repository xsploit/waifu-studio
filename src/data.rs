use crate::{inspect_file, provider::ProviderConfig};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Character {
    #[serde(default)]
    pub delegation: crate::delegation::DelegationConfig,
    pub id: Uuid,
    pub name: String,
    pub model: PathBuf,
    pub persona: String,
    pub format: String,
    #[serde(default)]
    pub greeting: String,
    /// None inherits the library setting, preserving older saved profiles.
    #[serde(default)]
    pub scene: Option<SceneSettings>,
    #[serde(default)]
    pub provider: Option<usize>,
    #[serde(default)]
    pub voice: Option<crate::voice::VoiceConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// Conversation storage is a rolling window, not long-term semantic memory.
/// Whole oldest messages are discarded to retain at most 200 turns / 2 MiB.
pub fn trim_history(turns: &mut Vec<Turn>) {
    let start = history_start(turns);
    if start > 0 {
        turns.drain(..start);
    }
    if turns.capacity() > 400 {
        turns.shrink_to_fit();
    }
}

fn history_start(turns: &[Turn]) -> usize {
    let mut bytes = 0usize;
    let mut start = turns.len();
    for turn in turns.iter().rev().take(200) {
        let Some(next) = bytes.checked_add(turn.content.len()) else {
            break;
        };
        if next > 2 * 1024 * 1024 {
            break;
        }
        bytes = next;
        start -= 1;
    }
    start
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMotionMode {
    #[default]
    Off,
    Suggest,
    AutoPlay,
    /// Plays the user's binding for the reply's explicit affect emotion. No
    /// decision request; replies without parsed metadata play nothing.
    EmotionMapped,
}

/// One user-chosen emotion -> motion binding. The path only plays while it is
/// an entry of the current scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmotionMotionBinding {
    pub emotion: crate::reply_affect::AffectEmotion,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplyMotionConfig {
    pub mode: ReplyMotionMode,
    pub cooldown_seconds: u32,
    /// Bindings form a playlist per emotion in selection order; empty for older profiles.
    pub emotion_bindings: Vec<EmotionMotionBinding>,
}
impl Default for ReplyMotionConfig {
    fn default() -> Self {
        Self {
            mode: ReplyMotionMode::Off,
            cooldown_seconds: 30,
            emotion_bindings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneSettings {
    pub routine: crate::routines::RoutineConfig,
    pub reply_motion: ReplyMotionConfig,
    pub gaze: crate::gaze::GazeConfig,
    pub affect: crate::reply_affect::AffectConfig,
    pub behavior: crate::behavior::BehaviorConfig,
    pub appearance: crate::appearance::AppearanceConfig,
    pub facial: crate::expressions::FacialConfig,
    pub visemes: crate::viseme_audio::VisemeConfig,
    pub environment: crate::environments::EnvironmentKind,
    pub room_view: bool,
    pub visuals: crate::visuals::VisualSettings,
    pub background: [f32; 3],
    pub image: Option<PathBuf>,
    pub portrait: bool,
    pub idle_motion: bool,
    pub shader: u8,
    pub outlines: bool,
}
impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            routine: Default::default(),
            reply_motion: Default::default(),
            gaze: Default::default(),
            affect: Default::default(),
            facial: Default::default(),
            visemes: Default::default(),
            appearance: Default::default(),
            behavior: Default::default(),
            environment: Default::default(),
            room_view: false,
            visuals: Default::default(),
            background: [0.115, 0.12, 0.12],
            image: None,
            portrait: true,
            idle_motion: true,
            shader: 0,
            outlines: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub decision: crate::recipient_decision::DecisionConfig,
    pub bundled_seen: std::collections::BTreeSet<Uuid>,
    pub version: u32,
    pub characters: Vec<Character>,
    pub selected: Option<Uuid>,
    pub providers: Vec<ProviderConfig>,
    pub provider: usize,
    pub scene: SceneSettings,
    pub voice: crate::voice::VoiceConfig,
    pub listen: crate::listen::ListenConfig,
}
impl Default for Library {
    fn default() -> Self {
        Self {
            version: 2,
            decision: Default::default(),
            bundled_seen: Default::default(),
            characters: vec![],
            selected: None,
            providers: ProviderConfig::presets(),
            provider: 0,
            scene: SceneSettings::default(),
            voice: crate::voice::VoiceConfig::default(),
            listen: crate::listen::ListenConfig::default(),
        }
    }
}

impl Library {
    /// One-time upgrade of pre-streaming profiles. Later user edits are retained.
    pub fn migrate_streaming_defaults(&mut self) -> bool {
        if self.version != 1 {
            return false;
        }
        self.voice.use_live_pipeline();
        for character in &mut self.characters {
            if let Some(voice) = &mut character.voice {
                voice.use_live_pipeline();
            }
        }
        self.version = 2;
        true
    }

    pub fn selected_character(&self) -> Option<&Character> {
        self.characters.iter().find(|c| Some(c.id) == self.selected)
    }

    pub fn selected_character_mut(&mut self) -> Option<&mut Character> {
        self.characters
            .iter_mut()
            .find(|c| Some(c.id) == self.selected)
    }

    pub fn capture_selected_settings(&mut self) {
        let scene = self.scene.clone();
        let provider = self.provider;
        let voice = self.voice.clone();
        if let Some(character) = self.selected_character_mut() {
            character.scene = Some(scene);
            character.provider = Some(provider);
            character.voice = Some(voice);
        }
    }

    /// Saves the previous character's active settings before restoring the next.
    pub fn select_character(&mut self, id: Uuid) -> bool {
        if !self.characters.iter().any(|c| c.id == id) {
            return false;
        }
        self.capture_selected_settings();
        self.selected = Some(id);
        let character = self.selected_character().unwrap();
        let scene = character.scene.clone();
        let provider = character.provider;
        let voice = character.voice.clone();
        if let Some(voice) = voice {
            self.voice = voice;
        }
        if let Some(scene) = scene {
            self.scene = scene;
        }
        if let Some(provider) = provider {
            self.provider = provider.min(self.providers.len().saturating_sub(1));
        }
        true
    }

    pub fn import(&mut self, path: &Path) -> Result<Uuid, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        if let Some(character) = self.characters.iter().find(|c| c.model == path) {
            let id = character.id;
            self.select_character(id);
            return Ok(id);
        }
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (name, format) = match extension.as_str() {
            "vrm" => {
                let info = inspect_file(&path).map_err(|e| e.to_string())?;
                (info.name, format!("VRM {}", info.version))
            }
            "glb" | "gltf" => {
                let model = gltf::Gltf::open(&path).map_err(|e| e.to_string())?;
                if model.scenes().next().is_none() {
                    return Err("Model has no scene".into());
                }
                (
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    "glTF 2.0".into(),
                )
            }
            _ => return Err("Choose a .vrm, .glb, or .gltf model".into()),
        };
        let id = Uuid::new_v4();
        self.capture_selected_settings();
        self.characters.push(Character { delegation: Default::default(), id, name: name.clone(), model: path, format,
            greeting: format!("Hi, I'm {name}. What's on your mind?"),
            scene: Some(self.scene.clone()), provider: Some(self.provider), voice: Some(self.voice.clone()),
            persona: format!("You are {name}, a friendly virtual character companion. Speak naturally and keep replies conversational. Your personality can be customized here.") });
        self.selected = Some(id);
        Ok(id)
    }
}

pub struct Store {
    pub root: PathBuf,
}
impl Store {
    pub fn open(root: PathBuf) -> Result<(Self, Library), String> {
        fs::create_dir_all(root.join("conversations")).map_err(|e| e.to_string())?;
        let path = root.join("library.json");
        let mut library: Library = if path.try_exists().map_err(|e| e.to_string())? {
            serde_json::from_slice(&read_bounded(&path, 8_000_000)?)
                .map_err(|e| format!("Cannot read saved library; original file left intact: {e}"))?
        } else {
            Library::default()
        };
        if !matches!(library.version, 1 | 2) {
            return Err("Unsupported library version; original file left intact".into());
        }
        library.migrate_streaming_defaults();
        if library.providers.is_empty() {
            library.providers = ProviderConfig::presets();
        }
        library.provider = library.provider.min(library.providers.len() - 1);
        let mut ids = std::collections::HashSet::new();
        if library
            .characters
            .iter()
            .any(|character| !ids.insert(character.id))
        {
            return Err(
                "Saved library contains duplicate character IDs; original file left intact".into(),
            );
        }
        if library.selected_character().is_none() {
            library.selected = library.characters.first().map(|character| character.id);
        }
        library.scene.appearance = library.scene.appearance.sanitized();
        for character in &mut library.characters {
            if let Some(scene) = &mut character.scene {
                scene.appearance = std::mem::take(&mut scene.appearance).sanitized();
            }
            if character
                .provider
                .is_some_and(|index| index >= library.providers.len())
            {
                character.provider = None;
            }
        }
        Ok((Self { root }, library))
    }
    pub fn save(&self, library: &Library) -> Result<(), String> {
        let mut snapshot = library.clone();
        snapshot.capture_selected_settings();
        atomic_json(&self.root.join("library.json"), &snapshot, 8_000_000)
    }
    pub fn history(&self, id: Uuid) -> Result<Vec<Turn>, String> {
        let path = self.root.join("conversations").join(format!("{id}.json"));
        if !path.try_exists().map_err(|e| e.to_string())? {
            return Ok(vec![]);
        }
        let mut turns: Vec<Turn> = serde_json::from_slice(&read_bounded(&path, 16_000_000)?)
            .map_err(|e| format!("Cannot read conversation; original file left intact: {e}"))?;
        trim_history(&mut turns);
        Ok(turns)
    }
    pub fn save_history(&self, id: Uuid, turns: &[Turn]) -> Result<(), String> {
        atomic_json(
            &self.root.join("conversations").join(format!("{id}.json")),
            &turns[history_start(turns)..],
            16_000_000,
        )
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Saved data exceeds the supported size; original file left intact".into());
    }
    Ok(bytes)
}

fn atomic_json<T: Serialize + ?Sized>(path: &Path, data: &T, limit: usize) -> Result<(), String> {
    struct BoundedBytes {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for BoundedBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "Saved data exceeds its size limit; existing file left intact",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer_pretty(&mut writer, data).map_err(|e| e.to_string())?;
    let bytes = writer.bytes;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<(), std::io::Error> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map_err(|e| e.to_string())
}

/// Bounded, explicit directory scan. Does not follow symlinked directories.
pub fn find_models(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    fn walk(root: &Path, depth: usize, output: &mut Vec<PathBuf>) {
        if output.len() >= 1000 {
            return;
        }
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth > 0 {
                if matches!(
                    entry.file_name().to_str(),
                    Some("target" | "node_modules" | ".git" | "cache")
                ) {
                    continue;
                }
                walk(&path, depth - 1, output);
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "vrm" | "glb" | "gltf"
                        )
                    })
            {
                output.push(path);
            }
            if output.len() >= 1000 {
                break;
            }
        }
    }
    let mut paths = vec![];
    walk(root, max_depth, &mut paths);
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_scenes_keep_automatic_reply_motion_disabled() {
        let scene: SceneSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(scene.reply_motion.mode, ReplyMotionMode::Off);
        assert_eq!(scene.reply_motion.cooldown_seconds, 30);
        let scene: SceneSettings =
            serde_json::from_str(r#"{"reply_motion":{"mode":"auto_play","cooldown_seconds":45}}"#)
                .unwrap();
        let restored: SceneSettings =
            serde_json::from_value(serde_json::to_value(&scene).unwrap()).unwrap();
        assert_eq!(restored.reply_motion, scene.reply_motion);
        assert_eq!(restored.reply_motion.mode, ReplyMotionMode::AutoPlay);
        assert!(restored.reply_motion.emotion_bindings.is_empty());
        let mapped: ReplyMotionConfig = serde_json::from_str(
            r#"{"mode":"emotion_mapped","emotion_bindings":[{"emotion":"happy","path":"/m/joy.bvh"}]}"#,
        )
        .unwrap();
        assert_eq!(mapped.mode, ReplyMotionMode::EmotionMapped);
        assert_eq!(mapped.cooldown_seconds, 30);
        assert_eq!(
            mapped.emotion_bindings,
            [EmotionMotionBinding {
                emotion: crate::reply_affect::AffectEmotion::Happy,
                path: "/m/joy.bvh".into(),
            }]
        );
    }

    #[test]
    fn round_trip_preserves_profiles_but_does_not_write_api_keys() {
        let root = std::env::temp_dir().join(format!("studio-{}", Uuid::new_v4()));
        let (store, mut library) = Store::open(root.clone()).unwrap();
        library.providers[0].session_key = "do-not-write-me".into();
        store.save(&library).unwrap();
        let saved = fs::read_to_string(root.join("library.json")).unwrap();
        assert!(!saved.contains("do-not-write-me"));
        let (_, restored) = Store::open(root.clone()).unwrap();
        assert_eq!(restored.providers.len(), 3);
        assert!(restored.providers[0].session_key.is_empty());
        let id = Uuid::new_v4();
        store
            .save_history(
                id,
                &[Turn {
                    role: "user".into(),
                    content: "Hello".into(),
                }],
            )
            .unwrap();
        assert_eq!(store.history(id).unwrap()[0].content, "Hello");
        assert!(store.history(Uuid::new_v4()).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    fn character(name: &str) -> Character {
        Character {
            delegation: Default::default(),
            id: Uuid::new_v4(),
            name: name.into(),
            model: PathBuf::from("avatar.vrm"),
            persona: "A friendly companion".into(),
            format: "VRM 1".into(),
            greeting: "Hello!".into(),
            scene: None,
            provider: None,
            voice: None,
        }
    }

    #[test]
    fn switching_characters_restores_their_scene_and_provider() {
        let mut library = Library::default();
        let first = character("First");
        let second = character("Second");
        let (first_id, second_id) = (first.id, second.id);
        library.characters = vec![first, second];
        assert!(library.select_character(first_id));
        library.scene.shader = 1;
        library.scene.routine.enabled = true;
        library
            .scene
            .routine
            .anchors
            .push(crate::routines::RoutineAnchor {
                day: 2,
                minute: 600,
                activity: "Drawing".into(),
            });
        library.scene.facial.mouth_gain = 3.5;
        library.scene.visemes.enabled = true;
        library.scene.visemes.profile_path = Some("calibration-a.json".into());
        library.scene.visemes.vowel_gains = [0.5; 5];
        library.scene.behavior.enabled = true;
        library
            .scene
            .appearance
            .set_hidden("/Scene/Hat/Hat.0", true);
        library.voice.delivery = crate::voice::VoiceDelivery::TimestampSse;
        library.provider = 2;
        assert!(library.select_character(second_id));
        library.scene.shader = 2;
        library.scene.routine = Default::default();
        library.scene.facial.mouth_gain = 1.25;
        library.scene.visemes = Default::default();
        library.scene.behavior.enabled = false;
        library.scene.appearance.reset();
        library.voice.delivery = crate::voice::VoiceDelivery::Buffered;
        library.provider = 1;
        assert!(library.select_character(first_id));
        assert_eq!(library.scene.shader, 1);
        assert!(library.scene.routine.enabled);
        assert_eq!(library.scene.routine.anchors[0].activity, "Drawing");
        assert_eq!(library.scene.facial.mouth_gain, 3.5);
        assert!(library.scene.visemes.enabled && library.scene.behavior.enabled);
        assert_eq!(
            library.scene.visemes.profile_path,
            Some("calibration-a.json".into())
        );
        assert_eq!(library.scene.visemes.vowel_gains, [0.5; 5]);
        assert!(library.scene.appearance.is_hidden("/Scene/Hat/Hat.0"));
        assert_eq!(
            library.voice.delivery,
            crate::voice::VoiceDelivery::TimestampSse
        );
        assert_eq!(library.provider, 2);
        assert!(library.select_character(second_id));
        assert_eq!(library.scene.shader, 2);
        assert_eq!(
            library.scene.routine,
            crate::routines::RoutineConfig::default()
        );
        assert_eq!(library.scene.facial.mouth_gain, 1.25);
        assert!(!library.scene.visemes.enabled && !library.scene.behavior.enabled);
        assert!(library.scene.visemes.profile_path.is_none());
        assert!(library.scene.appearance.hidden_parts.is_empty());
        assert_eq!(
            library.voice.delivery,
            crate::voice::VoiceDelivery::Buffered
        );
        assert_eq!(library.provider, 1);
        assert!(!library.select_character(Uuid::new_v4()));
        assert_eq!(library.selected, Some(second_id));
    }

    #[test]
    fn saved_streaming_upgrade_runs_once_and_preserves_profile_choices() {
        use crate::voice::{VoiceAdapter, VoiceConfig, VoiceDelivery};
        let root = std::env::temp_dir().join(format!("studio-stream-upgrade-{}", Uuid::new_v4()));
        let (store, mut library) = Store::open(root.clone()).unwrap();
        library.version = 1;
        library.voice.delivery = VoiceDelivery::Buffered;
        library.voice.model = "drama-3".into();
        library.voice.reference_id = "selected-reference".into();
        library.voice.speed = 1.2;
        library.voice.enabled = true;
        let mut first = character("First");
        first.voice = Some(library.voice.clone());
        let mut second = character("Second");
        second.voice = Some(VoiceConfig::preset(VoiceAdapter::FishLocal));
        second.voice.as_mut().unwrap().delivery = VoiceDelivery::Buffered;
        let first_id = first.id;
        library.characters = vec![first, second];
        library.selected = Some(first_id);
        store.save(&library).unwrap();

        let (store, mut upgraded) = Store::open(root.clone()).unwrap();
        assert_eq!(upgraded.version, 2);
        assert_eq!(upgraded.selected, Some(first_id));
        assert_eq!(upgraded.voice.delivery, VoiceDelivery::LiveWebSocket);
        assert_eq!(upgraded.voice.model, "drama-3");
        assert_eq!(upgraded.voice.reference_id, "selected-reference");
        assert_eq!(upgraded.voice.speed, 1.2);
        assert!(upgraded.voice.enabled);
        assert_eq!(
            upgraded.characters[0].voice.as_ref().unwrap().delivery,
            VoiceDelivery::LiveWebSocket
        );
        assert_eq!(
            upgraded.characters[1].voice.as_ref().unwrap().delivery,
            VoiceDelivery::HttpWavStream
        );
        assert!(!upgraded.characters[1].voice.as_ref().unwrap().enabled);

        // Once upgraded, an explicit user transport choice survives subsequent opens.
        upgraded.voice.delivery = VoiceDelivery::Buffered;
        store.save(&upgraded).unwrap();
        let (_, reopened) = Store::open(root.clone()).unwrap();
        assert_eq!(reopened.voice.delivery, VoiceDelivery::Buffered);
        assert_eq!(
            reopened.characters[0].voice.as_ref().unwrap().delivery,
            VoiceDelivery::Buffered
        );
        assert!(!upgraded.migrate_streaming_defaults());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn older_profiles_gain_optional_defaults_and_corrupt_data_stays_intact() {
        let old = serde_json::json!({"id": Uuid::new_v4(), "name":"Old avatar", "model":"old.vrm",
            "persona":"Hello", "format":"VRM 0"});
        let restored: Character = serde_json::from_value(old).unwrap();
        assert!(restored.greeting.is_empty());
        assert!(restored.scene.is_none());
        assert!(restored.provider.is_none());
        let root = std::env::temp_dir().join(format!("studio-corrupt-{}", Uuid::new_v4()));
        let (store, _) = Store::open(root.clone()).unwrap();
        fs::write(root.join("library.json"), b"{broken").unwrap();
        assert!(Store::open(root.clone()).is_err());
        assert_eq!(fs::read(root.join("library.json")).unwrap(), b"{broken");
        let id = Uuid::new_v4();
        let path = root.join("conversations").join(format!("{id}.json"));
        fs::write(&path, b"[broken").unwrap();
        assert!(store.history(id).is_err());
        assert_eq!(fs::read(path).unwrap(), b"[broken");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_profiles_keep_customization_and_independent_histories() {
        let root = std::env::temp_dir().join(format!("studio-profiles-{}", Uuid::new_v4()));
        let (store, mut library) = Store::open(root.clone()).unwrap();
        let first = character("First");
        let second = character("Second");
        let (first_id, second_id) = (first.id, second.id);
        library.characters = vec![first, second];
        library.select_character(first_id);
        library.scene.background = [0.1, 0.2, 0.3];
        library.provider = 2;
        store.save(&library).unwrap();
        store
            .save_history(
                first_id,
                &[Turn {
                    role: "user".into(),
                    content: "first conversation".into(),
                }],
            )
            .unwrap();
        store
            .save_history(
                second_id,
                &[Turn {
                    role: "assistant".into(),
                    content: "second conversation".into(),
                }],
            )
            .unwrap();
        let (_, restored) = Store::open(root.clone()).unwrap();
        let profile = restored.selected_character().unwrap();
        assert_eq!(profile.greeting, "Hello!");
        assert_eq!(profile.provider, Some(2));
        assert_eq!(profile.scene.as_ref().unwrap().background, [0.1, 0.2, 0.3]);
        assert_eq!(
            store.history(first_id).unwrap()[0].content,
            "first conversation"
        );
        assert_eq!(
            store.history(second_id).unwrap()[0].content,
            "second conversation"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn oversized_save_preserves_the_previous_file() {
        let root = std::env::temp_dir().join(format!("studio-bounds-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("bounded.json");
        atomic_json(&path, &"original", 32).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(atomic_json(&path, &"x".repeat(100), 32).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rolling_history_keeps_recent_whole_unicode_messages_within_both_limits() {
        let mut turns: Vec<Turn> = (0..205)
            .map(|i| Turn {
                role: "user".into(),
                content: i.to_string(),
            })
            .collect();
        trim_history(&mut turns);
        assert_eq!(turns.len(), 200);
        assert_eq!(turns[0].content, "5");
        let message = "🌸".repeat(200_000);
        let mut turns = vec![
            Turn {
                role: "user".into(),
                content: message
            };
            3
        ];
        trim_history(&mut turns);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].content.chars().count(), 200_000);
        turns.push(Turn {
            role: "user".into(),
            content: "x".repeat(2 * 1024 * 1024 + 1),
        });
        trim_history(&mut turns);
        assert!(turns.is_empty());
    }
    #[test]
    fn emotion_playlists_round_trip_multiple_bindings_in_selection_order() {
        let config: ReplyMotionConfig = serde_json::from_str(r#"{"mode":"emotion_mapped","emotion_bindings":[{"emotion":"happy","path":"/m/Wave.bvh"},{"emotion":"happy","path":"/m/Idle.bvh"},{"emotion":"sad","path":"/m/Thinking.bvh"}]}"#).unwrap();
        let restored: ReplyMotionConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(restored, config);
        assert_eq!(restored.emotion_bindings.len(), 3);
        assert_eq!(
            restored.emotion_bindings[0].path,
            PathBuf::from("/m/Wave.bvh")
        );
        assert_eq!(
            restored.emotion_bindings[1].path,
            PathBuf::from("/m/Idle.bvh")
        );
    }
}
