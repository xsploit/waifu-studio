use bevy::{asset::AssetPath, prelude::*};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use uuid::Uuid;
use waifu_studio::{
    data::{Library, Store, Turn, find_models, trim_history},
    loading::{LoadAvatar, UnloadAvatar},
    provider::{self, ChatJob, ProviderEvent},
    voice::{VoiceEvent, VoiceWorker},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Chat,
    SceneChat,
    Characters,
    Scene,
    Connections,
    Voice,
    Tools,
    Memory,
    Tasks,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Studio,
    Desktop,
}

enum FileEvent {
    Models(Vec<PathBuf>),
    Background(PathBuf),
    Animation(PathBuf),
    Image(PathBuf, Option<Uuid>),
}

struct QueuedSpeech {
    scene_reply: bool,
    character: Uuid,
    config: waifu_studio::voice::VoiceConfig,
    text: String,
    key: Option<String>,
}
struct PendingLiveSpeech {
    id: u64,
    config: waifu_studio::voice::VoiceConfig,
    text: String,
    key: Option<String>,
}

struct PendingWsSpeech {
    id: u64,
    config: waifu_studio::voice::VoiceConfig,
    key: Option<String>,
    filter: waifu_studio::speech_text::ReplySpeechText,
    text: String,
    finished: bool,
    opened: bool,
}
/// Focused HTTP replies speak sentence groups while the LLM is still streaming.
/// Chunks join one worker turn (`VoiceWorker::enqueue`), so the next sentence
/// synthesizes during current playback without interrupting it. At most
/// `HTTP_SLOTS` chunks are requested or waiting ahead of playback; the worker
/// delivers them in order and `ready` holds them until playback drains.
struct PendingHttpSpeech {
    id: u64,
    config: waifu_studio::voice::VoiceConfig,
    key: Option<String>,
    filter: waifu_studio::speech_text::ReplySpeechText,
    text: String,
    ready: VecDeque<VoiceEvent>,
    outstanding: usize,
    finished: bool,
}
impl PendingHttpSpeech {
    fn push(&mut self, text: &str) {
        self.text.push_str(&self.filter.push(text));
    }
    fn finish(&mut self) {
        self.text.push_str(&self.filter.finish());
        self.finished = true;
    }
    fn done(&self) -> bool {
        self.finished
            && self.text.trim().is_empty()
            && self.outstanding == 0
            && self.ready.is_empty()
    }
}
const SPEECH_CHUNK_MIN: usize = 40;
const SPEECH_CHUNK_MAX: usize = 300;
/// Takes the next sentence group (40–300 characters). Western terminators split
/// only before whitespace, so decimals, URLs and a still-streaming "…" survive.
fn take_speech_chunk(text: &mut String, finished: bool) -> Option<String> {
    loop {
        let mut end = None;
        let mut last_space = None;
        let mut chars = text.char_indices().enumerate().peekable();
        while let Some((count, (index, c))) = chars.next() {
            let next = index + c.len_utf8();
            if c.is_whitespace() {
                last_space = Some(next);
            }
            let boundary = match c {
                '。' | '！' | '？' | '\n' => true,
                '.' | '!' | '?' | '…' => {
                    chars.peek().is_some_and(|(_, (_, n))| n.is_whitespace())
                }
                _ => false,
            };
            if boundary && count + 1 >= SPEECH_CHUNK_MIN {
                end = Some(next);
                break;
            }
            if count + 1 >= SPEECH_CHUNK_MAX {
                end = Some(last_space.unwrap_or(next));
                break;
            }
        }
        let end = end.or_else(|| finished.then_some(text.len()))?;
        let chunk: String = text.drain(..end).collect();
        let chunk = chunk.trim();
        if !chunk.is_empty() {
            return Some(chunk.to_owned());
        }
        if text.is_empty() {
            return None;
        }
    }
}
fn playback_speed(config: &waifu_studio::voice::VoiceConfig) -> f32 {
    if config.adapter == waifu_studio::voice::VoiceAdapter::FishLocal {
        config.speed
    } else {
        1.0
    }
}
impl PendingWsSpeech {
    fn push(&mut self, text: &str) {
        self.text.push_str(&self.filter.push(text));
    }
    fn finish(&mut self) {
        self.text.push_str(&self.filter.finish());
        self.finished = true;
    }
}

#[derive(Resource)]
pub struct Studio {
    pub facts: waifu_studio::facts_memory::FactsMemoryState,
    pub affect: waifu_studio::reply_affect::AffectLibrary,
    reply_filter: Option<waifu_studio::reply_affect::ReplyAffectFilter>,
    reply_affect_requested: bool,
    pub completed_chat: Option<(Uuid, Result<String, String>)>,
    /// (job, character, emotion) parsed from that job's opt-in affect block.
    pub completed_affect: Option<(Uuid, Uuid, waifu_studio::reply_affect::AffectEmotion)>,
    pub proposals: Vec<waifu_studio::delegation::TaskProposal>,
    pub attachments: Vec<waifu_studio::attachments::ImageAttachment>,
    attachment_worker: Mutex<waifu_studio::attachments::AttachmentWorker>,
    pub voice_key: String,
    pub voice_busy: bool,
    pub speech_character: Option<Uuid>,
    scene_speech_active: bool,
    active_voice: Option<waifu_studio::voice::VoiceConfig>,
    speech_queue: VecDeque<QueuedSpeech>,
    voice_playback_speed: f32,
    voice: Mutex<VoiceWorker>,
    voice_id: u64,
    pending_live: Option<PendingLiveSpeech>,
    pending_ws: Option<PendingWsSpeech>,
    pending_http: Option<PendingHttpSpeech>,
    reply_speech_job: Option<Uuid>,
    stop_audio: bool,
    dialog_open: Arc<AtomicBool>,
    unload_model: bool,
    pub store: Store,
    pub library: Library,
    pub tab: Tab,
    pub composer: String,
    pub turns: Vec<Turn>,
    pub pending_response: String,
    pub job: Option<ChatJob>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub connected: bool,
    pub model_lists: HashMap<usize, Vec<String>>,
    model_discovery: HashMap<usize, ModelDiscovery>,
    model_generation: u64,
    model_candidate: Option<(usize, u64, std::time::Instant)>,
    pub show_history: bool,
    pub mode: ViewMode,
    pub click_through: bool,
    pub requested_model: Option<PathBuf>,
    pub frame_requested: bool,
    provider_tx: mpsc::Sender<ProviderEvent>,
    provider_rx: Mutex<mpsc::Receiver<ProviderEvent>>,
    file_tx: mpsc::Sender<FileEvent>,
    file_rx: Mutex<mpsc::Receiver<FileEvent>>,
    chat_character: Option<Uuid>,
}

struct ModelDiscovery {
    identity: u64,
    generation: u64,
    loading: bool,
    error: Option<String>,
}

impl Studio {
    pub fn new(store: Store, library: Library) -> Self {
        let (provider_tx, provider_rx) = mpsc::channel();
        let (file_tx, file_rx) = mpsc::channel();
        let id = library.selected;
        let facts =
            waifu_studio::facts_memory::FactsMemoryState::load(store.root.join("facts-memory"));
        let mut studio = Self {
            facts,
            affect: Default::default(),
            reply_filter: None,
            reply_affect_requested: false,
            completed_chat: None,
            completed_affect: None,
            proposals: Vec::new(),
            attachments: vec![],
            attachment_worker: Mutex::new(Default::default()),
            dialog_open: Arc::new(AtomicBool::new(false)),
            unload_model: false,
            voice_key: String::new(),
            voice_busy: false,
            speech_character: None,
            scene_speech_active: false,
            active_voice: None,
            speech_queue: VecDeque::new(),
            voice_playback_speed: 1.0,
            voice: Mutex::new(VoiceWorker::new()),
            voice_id: 0,
            pending_live: None,
            pending_ws: None,
            pending_http: None,
            reply_speech_job: None,
            stop_audio: false,
            store,
            library,
            tab: Tab::Chat,
            composer: String::new(),
            turns: vec![],
            pending_response: String::new(),
            job: None,
            error: None,
            notice: None,
            connected: false,
            model_lists: HashMap::new(),
            model_discovery: HashMap::new(),
            model_generation: 0,
            model_candidate: None,
            show_history: false,
            mode: ViewMode::Studio,
            click_through: false,
            requested_model: None,
            frame_requested: true,
            provider_tx,
            provider_rx: Mutex::new(provider_rx),
            file_tx,
            file_rx: Mutex::new(file_rx),
            chat_character: None,
        };
        if let Some(id) = id {
            studio.select_character(id);
        }
        studio
    }
    pub fn select_character(&mut self, id: Uuid) {
        self.cancel_chat();
        self.affect.take_pending(id);
        self.stop_voice();
        self.attachments.clear();
        self.proposals.clear();
        self.attachment_worker = Mutex::new(Default::default());
        if !self.library.characters.iter().any(|c| c.id == id) {
            return;
        }
        self.library.select_character(id);
        self.facts.select_character(Some(id));
        self.requested_model = self.library.selected_character().map(|c| c.model.clone());
        self.frame_requested = true;
        self.composer.clear();
        self.turns.clear();
        match self.store.history(id) {
            Ok(turns) => self.turns = turns,
            Err(error) => self.error = Some(format!("Conversation could not be read: {error}")),
        }
        self.chat_character = Some(id);
        self.connected = false;
        self.save();
    }
    pub fn remove_character(&mut self) {
        let Some(id) = self.library.selected else {
            return;
        };
        self.cancel_chat();
        self.stop_voice();
        self.attachments.clear();
        self.proposals.clear();
        self.attachment_worker = Mutex::new(Default::default());
        self.library.characters.retain(|c| c.id != id);
        self.affect.entries.remove(&id);
        self.library.selected = None;
        self.facts.select_character(None);
        self.turns.clear();
        self.unload_model = true;
        self.requested_model = None;
        if let Some(next) = self.library.characters.first().map(|c| c.id) {
            self.select_character(next);
        }
        self.save();
    }
    pub fn duplicate_character(&mut self) {
        self.library.capture_selected_settings();
        if let Some(mut character) = self.library.selected_character().cloned() {
            character.id = Uuid::new_v4();
            character.name.push_str(" (copy)");
            let id = character.id;
            self.library.characters.push(character);
            self.select_character(id);
        }
    }
    pub fn stop_voice(&mut self) {
        self.speech_queue.clear();
        self.speech_character = None;
        self.scene_speech_active = false;
        self.active_voice = None;
        self.pending_live = None;
        self.pending_ws = None;
        self.pending_http = None;
        self.voice.lock().unwrap().cancel();
        self.voice_id = self.voice_id.wrapping_add(1);
        self.voice_busy = false;
        self.stop_audio = true;
    }
    pub fn speak(&mut self, text: String) {
        let character = self.library.selected.unwrap_or_default();
        self.speak_with_config(character, self.library.voice.clone(), text);
    }
    fn speech_key(&self, config: &waifu_studio::voice::VoiceConfig) -> Option<String> {
        (config.adapter == self.library.voice.adapter
            && config.endpoint == self.library.voice.endpoint
            && !self.voice_key.is_empty())
        .then(|| self.voice_key.clone())
    }
    /// Explicit preview/focused speech interrupts and clears any group queue.
    pub fn speak_with_config(
        &mut self,
        character: Uuid,
        config: waifu_studio::voice::VoiceConfig,
        text: String,
    ) {
        let key = self.speech_key(&config);
        self.stop_voice();
        self.start_owned_speech(QueuedSpeech {
            scene_reply: false,
            character,
            config,
            text,
            key,
        });
    }
    /// Group replies retain their voice and identity without changing the editor selection.
    pub fn queue_speech(
        &mut self,
        character: Uuid,
        config: waifu_studio::voice::VoiceConfig,
        text: String,
    ) -> Result<(), String> {
        if !config.enabled {
            return Ok(());
        }
        if !self.library.characters.iter().any(|c| c.id == character) {
            return Err("Speech character no longer exists".into());
        }
        if self.speech_queue.len() >= 8 {
            return Err("Speech queue is full (8 replies)".into());
        }
        if text.trim().is_empty() || text.chars().count() > 12_000 {
            return Err("Speech must contain 1–12000 characters".into());
        }
        let key = self.speech_key(&config);
        self.speech_queue.push_back(QueuedSpeech {
            scene_reply: true,
            character,
            config,
            text,
            key,
        });
        Ok(())
    }
    /// Roster edits invalidate only removed scene speakers; other replies keep FIFO order.
    pub fn retain_scene_speech(&mut self, allowed: &[Uuid]) {
        self.speech_queue
            .retain(|reply| !reply.scene_reply || allowed.contains(&reply.character));
        if self.scene_speech_active
            && self
                .speech_character
                .is_some_and(|id| !allowed.contains(&id))
        {
            let remaining = std::mem::take(&mut self.speech_queue);
            self.stop_voice();
            self.speech_queue = remaining;
        }
    }
    /// Scene cancellation does not interrupt a focused reply or explicit voice preview.
    pub fn cancel_scene_speech(&mut self) {
        self.speech_queue.retain(|reply| !reply.scene_reply);
        if self.scene_speech_active {
            self.stop_voice();
        }
    }
    fn advance_speech_queue(&mut self, playing: bool) {
        if self.voice_busy || playing {
            return;
        }
        self.speech_character = None;
        self.scene_speech_active = false;
        self.active_voice = None;
        while let Some(request) = self.speech_queue.pop_front() {
            if self
                .library
                .characters
                .iter()
                .any(|c| c.id == request.character)
            {
                self.voice_id = self.voice_id.wrapping_add(1);
                self.start_owned_speech(request);
                break;
            }
        }
    }
    fn start_owned_speech(&mut self, request: QueuedSpeech) {
        let QueuedSpeech {
            scene_reply,
            character,
            config,
            text,
            key,
        } = request;
        self.speech_character = Some(character);
        self.scene_speech_active = scene_reply;
        self.active_voice = Some(config.clone());
        self.voice_playback_speed = playback_speed(&config);
        if config.delivery == waifu_studio::voice::VoiceDelivery::TimestampSse {
            self.pending_live = Some(PendingLiveSpeech {
                id: self.voice_id,
                config,
                text,
                key,
            });
            self.voice_busy = true;
            return;
        }
        if config.delivery.is_websocket() {
            self.queue_ws_config(config, key);
            if let Some(request) = &mut self.pending_ws {
                request.push(&text);
                request.finish();
            }
            return;
        }
        let result = self
            .voice
            .lock()
            .unwrap()
            .start(self.voice_id, config, text, key);
        match result {
            Ok(()) => self.voice_busy = true,
            Err(error) => self.error = Some(error),
        }
    }
    fn queue_ws(&mut self) {
        self.speech_character = self.library.selected;
        self.active_voice = Some(self.library.voice.clone());
        self.queue_ws_config(
            self.library.voice.clone(),
            self.speech_key(&self.library.voice),
        );
    }
    fn queue_ws_config(&mut self, config: waifu_studio::voice::VoiceConfig, key: Option<String>) {
        let direction_chars = if config.direction.trim().is_empty() {
            0
        } else {
            config.direction.trim().chars().count() + 3
        };
        self.pending_ws = Some(PendingWsSpeech {
            id: self.voice_id,
            config,
            key,
            filter: waifu_studio::speech_text::ReplySpeechText::with_limit(
                12_000usize.saturating_sub(direction_chars),
            ),
            text: String::new(),
            finished: false,
            opened: false,
        });
        self.voice_busy = true;
    }
    fn pump_ws(&mut self, ws: &mut waifu_studio::fish_ws::FishWsState) {
        let Some(mut request) = self.pending_ws.take() else {
            return;
        };
        if request.id != self.voice_id {
            return;
        }
        if !request.opened && request.text.trim().is_empty() {
            if request.finished {
                self.voice_busy = false;
            } else {
                self.pending_ws = Some(request);
            }
            return;
        }
        let result = (|| {
            if !request.opened {
                ws.timestamps = request.config.delivery
                    == waifu_studio::voice::VoiceDelivery::TimestampWebSocket;
                ws.open(request.id, &request.config, request.key.take())?;
                request.opened = true;
            }
            if !request.text.is_empty() {
                ws.push_text(&request.text)?;
                request.text.clear();
            }
            if request.finished {
                ws.finish()?;
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            ws.cancel();
            self.voice_busy = false;
            self.error = Some(error);
        } else if !request.finished {
            self.pending_ws = Some(request);
        }
    }
    fn begin_reply_speech(&mut self, id: Uuid) {
        self.stop_voice();
        self.reply_speech_job = None;
        if self.library.voice.enabled && self.library.voice.delivery.is_websocket() {
            self.reply_speech_job = Some(id);
            self.queue_ws();
        } else if self.library.voice.enabled
            && matches!(
                self.library.voice.delivery,
                waifu_studio::voice::VoiceDelivery::Buffered
                    | waifu_studio::voice::VoiceDelivery::HttpWavStream
            )
        {
            let config = self.library.voice.clone();
            self.reply_speech_job = Some(id);
            self.speech_character = self.library.selected;
            self.voice_playback_speed = playback_speed(&config);
            self.active_voice = Some(config.clone());
            self.pending_http = Some(PendingHttpSpeech {
                id: self.voice_id,
                key: self.speech_key(&config),
                config,
                filter: Default::default(),
                text: String::new(),
                ready: VecDeque::new(),
                outstanding: 0,
                finished: false,
            });
            self.voice_busy = true;
        }
    }
    fn feed_reply_speech(&mut self, id: Uuid, delta: &str) {
        if self.reply_speech_job != Some(id) {
            return;
        }
        if let Some(request) = &mut self.pending_ws {
            request.push(delta);
        }
        if let Some(request) = &mut self.pending_http {
            request.push(delta);
        }
    }
    /// Requests upcoming chunks while worker slots and the lookahead bound allow,
    /// and returns the next ready clip or stream once nothing else is playing.
    fn pump_http(&mut self, playing: bool) -> Option<VoiceEvent> {
        let request = self.pending_http.as_mut()?;
        let next = if playing {
            None
        } else {
            request.ready.pop_front()
        };
        while request.outstanding + request.ready.len() < waifu_studio::voice::HTTP_SLOTS
            && self.voice.lock().unwrap().can_prefetch()
            && let Some(text) = take_speech_chunk(&mut request.text, request.finished)
        {
            let result = self.voice.lock().unwrap().enqueue(
                request.id,
                request.config.clone(),
                text,
                request.key.clone(),
            );
            if let Err(error) = result {
                self.stop_voice();
                self.error = Some(error);
                return None;
            }
            request.outstanding += 1;
        }
        if request.done() {
            self.pending_http = None;
            self.voice_busy = false;
        }
        next
    }
    /// Routes a current worker event; returns events that should play now.
    fn receive_voice_event(&mut self, event: VoiceEvent) -> Option<VoiceEvent> {
        let Some(request) = self
            .pending_http
            .as_mut()
            .filter(|request| request.id == event.id())
        else {
            self.voice_busy = false;
            return Some(event);
        };
        request.outstanding = request.outstanding.saturating_sub(1);
        match event {
            VoiceEvent::Ready { .. } | VoiceEvent::Streaming { .. } => {
                request.ready.push_back(event);
                None
            }
            VoiceEvent::Failed { message, .. } => {
                self.stop_voice();
                self.error = Some(message);
                None
            }
        }
    }
    fn finish_reply_speech(&mut self, id: Uuid) -> bool {
        if self.reply_speech_job != Some(id) {
            return false;
        }
        if let Some(request) = &mut self.pending_ws {
            request.finish();
        }
        if let Some(request) = &mut self.pending_http {
            request.finish();
        }
        // Remains true even after Stop speech: never re-synthesize a stopped reply.
        true
    }
    fn begin_design_preview(&mut self) {
        self.stop_voice();
        self.speech_character = self.library.selected;
        let mut config = self.library.voice.clone();
        // Voice Design supplies buffered audio; no synthesis transport owns it.
        config.delivery = waifu_studio::voice::VoiceDelivery::Buffered;
        self.active_voice = Some(config);
        self.scene_speech_active = false;
    }
    pub fn preview_voice(&mut self) {
        let text = self
            .turns
            .iter()
            .rev()
            .find(|t| t.role == "assistant")
            .map(|t| t.content.clone())
            .or_else(|| {
                self.library
                    .selected_character()
                    .map(|c| c.greeting.clone())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "Hello! It is good to see you.".into());
        self.speak(text);
    }
    pub fn save(&mut self) {
        self.library.capture_selected_settings();
        if let Err(error) = self.store.save(&self.library) {
            self.error = Some(format!("Could not save settings: {error}"));
        }
    }
    pub fn import_paths(&mut self, paths: Vec<PathBuf>) {
        self.library.capture_selected_settings();
        let previous = self.library.selected;
        let mut last = None;
        let mut count = 0;
        let mut failures = vec![];
        for path in paths {
            match self.library.import(&path) {
                Ok(id) => {
                    last = Some(id);
                    count += 1;
                }
                Err(error) => failures.push(format!(
                    "{}: {error}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }
        if let Some(id) = previous {
            self.library.select_character(id);
        } else {
            self.library.selected = None;
        }
        if let Some(id) = last {
            self.select_character(id);
        }
        self.notice = Some(format!("Added {count} character(s) to your library"));
        if !failures.is_empty() {
            self.error = Some(failures.join("\n"));
        }
        self.save();
    }
    pub fn open_animation(&self) {
        if self.dialog_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let tx = self.file_tx.clone();
        let flag = self.dialog_open.clone();
        std::thread::spawn(move || {
            let _guard = DialogGuard(flag);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose a humanoid animation")
                .add_filter("Humanoid motion", &["bvh", "vrma", "fbx"])
                .pick_file()
            {
                let _ = tx.send(FileEvent::Animation(path));
            }
        });
    }
    pub fn open_image(&self) {
        if self.attachments.len() >= waifu_studio::attachments::MAX_ATTACHMENTS
            || self.dialog_open.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let tx = self.file_tx.clone();
        let flag = self.dialog_open.clone();
        let character = self.library.selected;
        std::thread::spawn(move || {
            let _guard = DialogGuard(flag);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Attach an image to your next message")
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            {
                let _ = tx.send(FileEvent::Image(path, character));
            }
        });
    }
    pub fn preparing_image(&self) -> bool {
        self.attachment_worker.lock().unwrap().is_busy()
    }
    pub fn open_models(&self) {
        if self.dialog_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let flag = self.dialog_open.clone();
        let tx = self.file_tx.clone();
        std::thread::spawn(move || {
            let _guard = DialogGuard(flag);
            if let Some(paths) = rfd::FileDialog::new()
                .set_title("Add characters")
                .add_filter("3D characters", &["vrm", "glb", "gltf"])
                .pick_files()
            {
                let _ = tx.send(FileEvent::Models(paths));
            }
        });
    }
    pub fn scan_folder(&self) {
        if self.dialog_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let flag = self.dialog_open.clone();
        let tx = self.file_tx.clone();
        std::thread::spawn(move || {
            let _guard = DialogGuard(flag);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Scan a character folder")
                .pick_folder()
            {
                let _ = tx.send(FileEvent::Models(find_models(&path, 8)));
            }
        });
    }
    pub fn open_background(&self) {
        if self.dialog_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let flag = self.dialog_open.clone();
        let tx = self.file_tx.clone();
        std::thread::spawn(move || {
            let _guard = DialogGuard(flag);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose a background")
                .add_filter("Images", &["png", "jpg", "jpeg"])
                .pick_file()
            {
                let _ = tx.send(FileEvent::Background(path));
            }
        });
    }
    pub fn refresh_models(&mut self) {
        let index = self.library.provider;
        if let Some(config) = self.library.providers.get(index) {
            let identity = config.catalog_identity();
            if self
                .model_discovery
                .get(&index)
                .is_some_and(|state| state.loading && state.identity == identity)
            {
                return;
            }
            self.model_generation += 1;
            let generation = self.model_generation;
            self.model_discovery.insert(
                index,
                ModelDiscovery {
                    identity,
                    generation,
                    loading: true,
                    error: None,
                },
            );
            provider::fetch_models(index, generation, config.clone(), self.provider_tx.clone());
        }
    }
    /// Discovery runs once per connection/credential change, including failures.
    pub fn ensure_models(&mut self) {
        let index = self.library.provider;
        let Some(config) = self.library.providers.get(index) else {
            return;
        };
        let identity = config.catalog_identity();
        if let Some(state) = self.model_discovery.get(&index) {
            if state.identity == identity {
                return;
            }
            self.model_lists.remove(&index);
            match self.model_candidate {
                Some((candidate, key, since)) if candidate == index && key == identity => {
                    if since.elapsed() < std::time::Duration::from_millis(400) {
                        return;
                    }
                }
                _ => {
                    self.model_candidate = Some((index, identity, std::time::Instant::now()));
                    return;
                }
            }
        } else if self.model_lists.contains_key(&index) {
            return;
        }
        self.refresh_models();
    }
    pub fn models_status(&self) -> (bool, Option<&str>) {
        self.model_discovery
            .get(&self.library.provider)
            .map(|state| (state.loading, state.error.as_deref()))
            .unwrap_or_default()
    }
    pub fn invalidate_model_catalogs(&mut self) {
        self.model_lists.clear();
        self.model_discovery.clear();
        self.model_candidate = None;
    }
    pub fn cancel_chat(&mut self) {
        if let Some(id) = self.chat_character {
            self.affect.take_pending(id);
        }
        if let Some(job) = self.job.take() {
            if self.reply_speech_job == Some(job.id) {
                self.stop_voice();
            }
            job.cancel();
        }
        self.reply_speech_job = None;
        self.pending_response.clear();
        self.reply_filter = None;
        self.reply_affect_requested = false;
    }
    pub fn send_chat(&mut self, memory: &mut waifu_studio::memory::MemoryState) {
        self.send_chat_with_semantic(memory, None);
    }

    pub fn send_chat_with_semantic(
        &mut self,
        memory: &mut waifu_studio::memory::MemoryState,
        semantic: Option<&mut waifu_studio::semantic_memory::SemanticMemory>,
    ) {
        memory.select_character(self.library.selected);
        self.facts.select_character(self.library.selected);
        self.facts.poll();
        if memory.busy {
            self.notice = Some("Local notes are loading or saving. Send again when ready.".into());
            return;
        }
        if self.facts.busy {
            self.notice =
                Some("The evidence ledger is loading or saving. Send again when ready.".into());
            return;
        }
        if self.job.is_some() || self.composer.trim().is_empty() {
            return;
        }
        let Some(character) = self.library.selected_character() else {
            self.error = Some("Add a character first".into());
            return;
        };
        if self.composer.len() > 32_000 {
            self.error = Some("Messages are limited to 32 KB".into());
            return;
        }
        let config = self.library.providers[self.library.provider].clone();
        if config.model.trim().is_empty() {
            self.tab = Tab::Connections;
            self.error = Some("Choose a model for this connection first".into());
            return;
        }
        if let Err(error) = config.endpoint("chat/completions") {
            self.error = Some(error);
            return;
        }
        let semantic_notes = semantic.and_then(|state| {
            state.take_context_for(self.library.selected, memory, &self.composer)
        });
        // Semantic retrieval excludes pinned notes. Use only pins alongside it,
        // so the same unpinned note cannot also appear via lexical retrieval.
        let notes = if semantic_notes.is_some() {
            memory.context_pins_only()
        } else {
            memory.context(&self.composer)
        };
        let mut persona = format!(
            "{}\n\n{}",
            character.persona,
            character.delegation.prompt_instruction()
        );
        let id = character.id;
        self.facts.select_character(Some(id));
        let facts_context = self.facts.context(&self.composer);
        self.affect.entries.entry(id).or_default().config = self.library.scene.affect.clone();
        self.affect.take_pending(id);
        self.reply_affect_requested = false;
        if let Some(instruction) = self.affect.prompt_for(id) {
            persona.push_str("\n\n");
            persona.push_str(instruction);
            self.reply_affect_requested = true;
        }
        self.facts
            .observe_user_turn(id, Uuid::new_v4(), self.composer.trim());
        self.turns.push(Turn {
            role: "user".into(),
            content: std::mem::take(&mut self.composer).trim().to_owned(),
        });
        trim_history(&mut self.turns);
        let mut context = vec![Turn {
            role: "system".into(),
            content: persona,
        }];
        let history_start = self.turns.len().saturating_sub(40);
        let latest = self.turns.len().saturating_sub(1);
        // Keep the established conversation prefix stable for provider caches.
        // Query-dependent recall belongs immediately before this user turn.
        context.extend(self.turns[history_start..latest].iter().cloned());
        // Retrieved notes are data, never elevated into system instructions.
        if let Some(content) = notes {
            context.push(Turn {
                role: "user".into(),
                content,
            });
        }
        if let Some(content) = semantic_notes {
            context.push(Turn {
                role: "user".into(),
                content,
            });
        }
        if let Some(content) = facts_context {
            context.push(Turn {
                role: "user".into(),
                content,
            });
        }
        context.extend(self.turns[latest..].iter().cloned());
        self.pending_response.clear();
        self.reply_filter = Some(Default::default());
        self.error = None;
        self.notice = None;
        self.completed_chat = None;
        self.completed_affect = None;
        self.job = Some(provider::spawn_chat_with_images(
            config,
            context,
            std::mem::take(&mut self.attachments),
            self.provider_tx.clone(),
        ));
        if let Some(job_id) = self.job.as_ref().map(|job| job.id) {
            self.begin_reply_speech(job_id);
        }
        self.chat_character = Some(id);
        if let Err(error) = self.store.save_history(id, &self.turns) {
            self.error = Some(error);
        }
    }
}

pub fn poll_studio(
    mut studio: ResMut<Studio>,
    mut loads: MessageWriter<LoadAvatar>,
    mut unloads: MessageWriter<UnloadAvatar>,
    mut animation: ResMut<waifu_studio::animation::AnimationState>,
) {
    studio.facts.poll();
    let prepared = studio.attachment_worker.lock().unwrap().poll();
    if let Some(result) = prepared {
        match result {
            Ok(image) if studio.attachments.len() < waifu_studio::attachments::MAX_ATTACHMENTS => {
                studio.attachments.push(image)
            }
            Err(error) => studio.error = Some(error),
            _ => {}
        }
    }
    let files: Vec<_> = studio.file_rx.lock().unwrap().try_iter().collect();
    for event in files {
        match event {
            FileEvent::Models(paths) => studio.import_paths(paths),
            FileEvent::Image(path, character) => {
                if character == studio.library.selected
                    && studio.attachments.len() < waifu_studio::attachments::MAX_ATTACHMENTS
                {
                    let result = studio.attachment_worker.lock().unwrap().start(path);
                    if let Err(error) = result {
                        studio.error = Some(error);
                    }
                }
            }
            FileEvent::Animation(path) => {
                if let Err(error) = animation.load_path(path) {
                    studio.error = Some(error);
                }
            }
            FileEvent::Background(path) => {
                studio.library.scene.image = Some(path);
                studio.save();
            }
        }
    }
    let events: Vec<_> = studio.provider_rx.lock().unwrap().try_iter().collect();
    for event in events {
        match event {
            ProviderEvent::Models(index, generation, result)
                if studio.model_discovery.get(&index).is_some_and(|state| {
                    state.generation == generation
                        && studio
                            .library
                            .providers
                            .get(index)
                            .is_some_and(|config| config.catalog_identity() == state.identity)
                }) =>
            {
                let state = studio.model_discovery.get_mut(&index).unwrap();
                state.loading = false;
                match result {
                    Ok(models) => {
                        state.error = None;
                        studio.model_lists.insert(index, models);
                    }
                    Err(error) => state.error = Some(error),
                }
            }
            ProviderEvent::Delta(id, text) if studio.job.as_ref().is_some_and(|j| j.id == id) => {
                let visible = studio.reply_filter.get_or_insert_default().push(&text);
                studio.feed_reply_speech(id, &visible);
                studio.pending_response.push_str(&visible)
            }
            ProviderEvent::Finished(id) if studio.job.as_ref().is_some_and(|j| j.id == id) => {
                studio.completed_affect = None;
                if let Some(filter) = studio.reply_filter.take() {
                    let filtered = filter.finish();
                    studio.feed_reply_speech(id, &filtered.tail);
                    studio.pending_response.push_str(&filtered.tail);
                    if studio.reply_affect_requested {
                        if let (Some(character), Some(metadata)) =
                            (studio.chat_character, filtered.metadata)
                        {
                            match studio.affect.apply_reply(character, metadata) {
                                Ok(()) => {
                                    studio.completed_affect =
                                        Some((id, character, metadata.emotion));
                                }
                                Err(error) => studio.notice = Some(error),
                            }
                        } else if let Some(error) = filtered.error {
                            studio.notice = Some(error);
                        }
                    }
                }
                studio.reply_affect_requested = false;
                let content = std::mem::take(&mut studio.pending_response);
                studio.completed_chat = Some((id, Ok(content.clone())));
                if studio
                    .library
                    .selected_character()
                    .is_some_and(|c| c.delegation.enabled)
                {
                    match waifu_studio::delegation::parse_proposals(&content) {
                        Ok(proposals) if !proposals.is_empty() => {
                            studio.notice = Some(format!(
                                "{} task suggestions ready to review in Tasks",
                                proposals.len()
                            ));
                            studio.proposals = proposals;
                        }
                        Err(error) => studio.error = Some(error),
                        _ => {}
                    }
                }
                let streamed_speech = studio.finish_reply_speech(id);
                if studio.library.voice.enabled && !streamed_speech {
                    studio.speak(
                        content
                            .split("```waifu_tasks")
                            .next()
                            .unwrap_or(&content)
                            .to_owned(),
                    );
                }
                studio.turns.push(Turn {
                    role: "assistant".into(),
                    content,
                });
                trim_history(&mut studio.turns);
                studio.job = None;
                studio.connected = true;
                if let Some(character) = studio.chat_character
                    && let Err(error) = studio.store.save_history(character, &studio.turns)
                {
                    studio.error = Some(error);
                }
            }
            ProviderEvent::Failed(id, error) if studio.job.as_ref().is_some_and(|j| j.id == id) => {
                if studio.reply_speech_job == Some(id) {
                    studio.stop_voice();
                    studio.reply_speech_job = None;
                }
                studio.completed_chat = Some((id, Err(error.clone())));
                studio.completed_affect = None;
                studio.error = Some(error);
                studio.job = None;
                studio.pending_response.clear();
                studio.reply_filter = None;
                studio.reply_affect_requested = false;
                studio.connected = false;
            }
            _ => {}
        }
    }
    if std::mem::take(&mut studio.unload_model) {
        unloads.write(UnloadAvatar);
    }
    if let Some(path) = studio.requested_model.take() {
        loads.write(LoadAvatar(AssetPath::from_path(&path).into_owned()));
    }
}

pub fn sync_viseme_profile(
    studio: Res<Studio>,
    mut visemes: ResMut<waifu_studio::viseme_audio::VisemeState>,
    mut selected: Local<Option<Uuid>>,
) {
    let character = studio.speech_character.or(studio.library.selected);
    if *selected != character {
        // poll_voice can start a new owner after this system runs. Its
        // begin_speech already owns a fresh epoch; do not cancel it next frame.
        if studio.speech_character.is_none() {
            visemes.cancel();
        }
        visemes.cancel_profile_dialog();
        *selected = character;
    }
    let config = speaking_visemes(&studio);
    if visemes.config != config {
        visemes.config = config;
    }
    visemes.poll();
}

fn speaking_visemes(studio: &Studio) -> waifu_studio::viseme_audio::VisemeConfig {
    match studio.speech_character {
        Some(id) if Some(id) != studio.library.selected => studio
            .library
            .characters
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.scene.as_ref())
            .map(|s| s.visemes.clone())
            .unwrap_or_default(),
        _ => studio.library.scene.visemes.clone(),
    }
}

#[derive(Component)]
pub struct SpokenAudio;
#[allow(clippy::too_many_arguments)] // Independent Bevy resources.
pub fn poll_voice(
    mut visemes: ResMut<waifu_studio::viseme_audio::VisemeState>,
    mut speech_target: ResMut<waifu_studio::expressions::SpeechTarget>,
    mut ws: ResMut<waifu_studio::fish_ws::FishWsState>,
    mut fish: ResMut<waifu_studio::fish_live::FishLiveState>,
    mut commands: Commands,
    mut studio: ResMut<Studio>,
    playing: Query<Entity, With<SpokenAudio>>,
    mut audio: ResMut<Assets<waifu_studio::metered_audio::MeteredSpeech>>,
    meter: Res<waifu_studio::metered_audio::SpeechMeter>,
    mut design: ResMut<waifu_studio::voice_design::VoiceDesignState>,
) {
    if std::mem::take(&mut studio.stop_audio) {
        meter.stop();
        visemes.cancel();
        fish.cancel();
        ws.cancel();
        for entity in &playing {
            commands.entity(entity).despawn();
        }
    }
    // Before polling events: deferred audio spawns from this frame must never
    // be mistaken for an empty playback queue.
    studio.advance_speech_queue(!playing.is_empty());
    speech_target.0 = studio.speech_character;
    let profile = speaking_visemes(&studio);
    if visemes.config != profile {
        visemes.config = profile;
    }
    let delivery = studio.active_voice.as_ref().map(|c| c.delivery);
    ws.enabled = delivery.is_some_and(|d| d.is_websocket());
    studio.pump_ws(&mut ws);
    fish.enabled = delivery == Some(waifu_studio::voice::VoiceDelivery::TimestampSse);
    if let Some(request) = studio.pending_live.take()
        && let Err(error) = fish.speak(request.id, &request.config, request.text, request.key)
    {
        studio.voice_busy = false;
        studio.error = Some(error);
    }
    for event in fish.poll().into_iter().chain(ws.poll()) {
        use waifu_studio::fish_live::LiveEvent;
        if event.id() != studio.voice_id {
            continue;
        }
        match event {
            LiveEvent::Playing { audio: source, .. } => {
                commands.spawn((
                    SpokenAudio,
                    AudioPlayer::<waifu_studio::metered_audio::MeteredSpeech>(
                        audio.add(meter.streaming_with_visemes(source, visemes.begin_speech())),
                    ),
                    PlaybackSettings::DESPAWN,
                ));
            }
            LiveEvent::Finished { .. } => {
                studio.voice_busy = false;
                if studio.pending_ws.take().is_some() {
                    studio.error = Some("Speech stream ended before the reply finished.".into());
                }
            }
            LiveEvent::Failed { message, .. } => {
                studio.pending_ws = None;
                studio.voice_busy = false;
                studio.error = Some(message);
                meter.stop();
                visemes.cancel();
            }
            LiveEvent::Timing { .. } => {}
        }
    }
    design.poll();
    if let Some(bytes) = design.take_preview() {
        let source = AudioSource {
            bytes: bytes.into(),
        };
        if rodio::Decoder::builder()
            .with_byte_len(source.bytes.len() as u64)
            .with_data(std::io::Cursor::new(source.clone()))
            .build()
            .is_ok()
        {
            studio.begin_design_preview();
            speech_target.0 = studio.speech_character;
            visemes.cancel();
            visemes.config = speaking_visemes(&studio);
            fish.cancel();
            ws.cancel();
            for entity in &playing {
                commands.entity(entity).despawn();
            }
            // Already stopped old entities here; do not stop the new preview next frame.
            studio.stop_audio = false;
            commands.spawn((
                SpokenAudio,
                AudioPlayer::<waifu_studio::metered_audio::MeteredSpeech>(
                    audio.add(meter.buffered_with_visemes(source, visemes.begin_speech())),
                ),
                PlaybackSettings::DESPAWN,
            ));
        } else {
            studio.error = Some("Voice Design returned unsupported preview audio".into());
        }
    }
    let event = studio.voice.lock().unwrap().try_recv();
    let mut spawned = false;
    if let Some(event) = event
        .filter(|event| event.id() == studio.voice_id)
        .and_then(|event| studio.receive_voice_event(event))
    {
        spawned = play_voice_event(
            &mut commands,
            &mut studio,
            &mut audio,
            &meter,
            &mut visemes,
            event,
        );
    }
    if let Some(event) = studio.pump_http(spawned || !playing.is_empty())
        && !play_voice_event(
            &mut commands,
            &mut studio,
            &mut audio,
            &meter,
            &mut visemes,
            event,
        )
    {
        studio.stop_voice();
        studio.error = Some("Speech provider returned invalid or unsupported audio".into());
    }
}

/// Plays a clip or stream; returns false (and reports) on failure or bad audio.
fn play_voice_event(
    commands: &mut Commands,
    studio: &mut Studio,
    audio: &mut Assets<waifu_studio::metered_audio::MeteredSpeech>,
    meter: &waifu_studio::metered_audio::SpeechMeter,
    visemes: &mut waifu_studio::viseme_audio::VisemeState,
    event: VoiceEvent,
) -> bool {
    match event {
        VoiceEvent::Ready { audio: clip, .. } => {
            play_clip(commands, studio, audio, meter, visemes, clip)
        }
        VoiceEvent::Streaming { audio: source, .. } => {
            commands.spawn((
                SpokenAudio,
                AudioPlayer::<waifu_studio::metered_audio::MeteredSpeech>(
                    audio.add(meter.streaming_with_visemes(source, visemes.begin_speech())),
                ),
                PlaybackSettings {
                    speed: studio.voice_playback_speed,
                    ..PlaybackSettings::DESPAWN
                },
            ));
            true
        }
        VoiceEvent::Failed { message, .. } => {
            meter.stop();
            visemes.cancel();
            studio.error = Some(message);
            false
        }
    }
}
/// Spawns validated buffered speech; returns false (and reports) on bad audio.
fn play_clip(
    commands: &mut Commands,
    studio: &mut Studio,
    audio: &mut Assets<waifu_studio::metered_audio::MeteredSpeech>,
    meter: &waifu_studio::metered_audio::SpeechMeter,
    visemes: &mut waifu_studio::viseme_audio::VisemeState,
    clip: waifu_studio::voice::VoiceAudio,
) -> bool {
    let source = AudioSource {
        bytes: clip.bytes.into(),
    };
    if rodio::Decoder::builder()
        .with_byte_len(source.bytes.len() as u64)
        .with_data(std::io::Cursor::new(source.clone()))
        .build()
        .is_err()
    {
        studio.error = Some("Speech provider returned invalid or unsupported audio".into());
        meter.stop();
        visemes.cancel();
        return false;
    }
    let handle = audio.add(meter.buffered_with_visemes(source, visemes.begin_speech()));
    commands.spawn((
        SpokenAudio,
        AudioPlayer::<waifu_studio::metered_audio::MeteredSpeech>(handle),
        PlaybackSettings {
            speed: studio.voice_playback_speed,
            ..PlaybackSettings::DESPAWN
        },
    ));
    true
}

struct DialogGuard(Arc<AtomicBool>);
impl Drop for DialogGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub fn poll_input(
    mut voice_library: ResMut<waifu_studio::voice_library::VoiceLibraryState>,
    mut studio: ResMut<Studio>,
    mut listening: ResMut<waifu_studio::listen::ListeningState>,
    mut selected: Local<Option<Uuid>>,
) {
    if *selected != studio.library.selected {
        voice_library.cancel();
        voice_library.page = Default::default();
        voice_library.take_selected();
        let previous_status = listening.status.clone();
        let was_busy = listening.is_busy();
        listening.cancel();
        if !was_busy && previous_status == "Microphone idle" {
            listening.status = previous_status;
        }
        *selected = studio.library.selected;
    }
    voice_library.poll();
    if let Some(transcript) = listening.poll() {
        if !studio.composer.is_empty() {
            studio.composer.push('\n');
        }
        studio.composer.push_str(&transcript);
        studio.notice = Some("Transcript ready. Review it, then send.".into());
        studio.tab = Tab::Chat;
    }
}

pub fn poll_assistant(
    mut semantic: ResMut<waifu_studio::semantic_memory::SemanticMemory>,
    mut board: ResMut<waifu_studio::task_board::TaskBoard>,
    studio: Res<Studio>,
    mut memory: ResMut<waifu_studio::memory::MemoryState>,
    mut mcp: ResMut<waifu_studio::mcp::McpState>,
    mut agents: ResMut<waifu_studio::agents::AgentState>,
) {
    memory.select_character(studio.library.selected);
    memory.poll();
    semantic.sync(studio.library.selected, &memory);
    board.poll();
    mcp.poll();
    agents.poll();
}

#[cfg(test)]
mod tests {
    use super::*;
    use waifu_studio::data::Character;
    #[test]
    fn streamed_affect_is_hidden_from_speech_display_and_history() {
        let (mut studio, directory) = studio();
        let character = studio.library.selected.unwrap();
        studio.library.voice.enabled = true;
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::LiveWebSocket;
        studio.library.scene.affect.enabled = true;
        studio.library.scene.behavior.enabled = true;
        studio.affect.entries.entry(character).or_default().config =
            studio.library.scene.affect.clone();
        studio.reply_affect_requested = true;
        studio.reply_filter = Some(Default::default());
        let id = Uuid::new_v4();
        studio.job = Some(ChatJob {
            id,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        studio.begin_reply_speech(id);
        let sender = studio.provider_tx.clone();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(100),
            ))
            .insert_resource(studio)
            .insert_resource(waifu_studio::expressions::FacialState {
                available: vec!["happy".into()],
                ..Default::default()
            })
            .init_resource::<waifu_studio::behavior::BehaviorState>()
            .init_resource::<waifu_studio::animation::AnimationState>()
            .add_message::<LoadAvatar>()
            .add_message::<UnloadAvatar>()
            .add_systems(
                Update,
                (poll_studio, crate::behavior_runtime::poll_behavior).chain(),
            );
        sender
            .send(ProviderEvent::Delta(id, "Hello! <wai".into()))
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<Studio>().pending_response, "Hello! ");
        sender.send(ProviderEvent::Delta(id, "fu-affect>{\"emotion\":\"happy\",\"valence\":0.8,\"arousal\":0.4,\"dominance\":0.2}</waifu-affect>".into())).unwrap();
        sender.send(ProviderEvent::Finished(id)).unwrap();
        app.update();
        let studio = app.world().resource::<Studio>();
        assert_eq!(studio.pending_ws.as_ref().unwrap().text, "Hello! ");
        assert_eq!(studio.turns.last().unwrap().content, "Hello! ");
        assert_eq!(
            studio
                .store
                .history(character)
                .unwrap()
                .last()
                .unwrap()
                .content,
            "Hello! "
        );
        assert_eq!(
            studio.affect.entries[&character].emotion,
            waifu_studio::reply_affect::AffectEmotion::Happy
        );
        assert!(studio.affect.entries[&character].mood.valence > 0.0);
        let controller = &app
            .world()
            .resource::<waifu_studio::behavior::BehaviorState>()
            .controller;
        let weights =
            controller.weights_for(Some(character), &["happy".into()], &Default::default(), "");
        assert!(weights.get("happy").is_some_and(|weight| *weight > 0.0));
        assert!(!studio.affect.entries.contains_key(&Uuid::new_v4()));
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn retrieved_notes_reach_request_once_and_never_enter_saved_history() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        use waifu_studio::{memory::MemoryState, semantic_memory::SemanticMemory};

        fn wait(mut done: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done() {
                assert!(Instant::now() < deadline, "background operation timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (sent, received) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                assert!(length < 64 * 1024);
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let chat = first.contains("/chat/completions");
                let response = if chat {
                    sent.send(body).unwrap();
                    "data: [DONE]\n\n".to_owned()
                } else {
                    let data: Vec<_> = body["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(i, _)| serde_json::json!({"index":i,"embedding":[1.0,0.0]}))
                        .collect();
                    serde_json::json!({"data":data}).to_string()
                };
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    if chat { "text/event-stream" } else { "application/json" }, response.len()).unwrap();
                if chat {
                    return;
                }
            }
            panic!("chat request never arrived");
        });
        let (mut studio, directory) = studio();
        let character = studio.library.selected;
        wait(|| {
            studio.facts.poll();
            studio.facts.loaded && !studio.facts.busy
        });
        studio.facts.set_enabled(true).unwrap();
        wait(|| {
            studio.facts.poll();
            !studio.facts.busy
        });
        studio
            .facts
            .add_record(
                waifu_studio::facts_memory::RecordKind::Preference,
                "Tea should be brewed at 80 degrees".into(),
                "User explicitly said 80 degrees".into(),
            )
            .unwrap();
        wait(|| {
            studio.facts.poll();
            !studio.facts.busy
        });
        let mut memory = MemoryState::load(directory.join("notes"));
        memory.select_character(character);
        wait(|| {
            memory.poll();
            !memory.busy
        });
        memory
            .add_note("The secret tea preference is jasmine".into())
            .unwrap();
        wait(|| {
            memory.poll();
            !memory.busy
        });
        memory.set_enabled(true).unwrap();
        wait(|| {
            memory.poll();
            !memory.busy
        });
        let mut semantic = SemanticMemory::load(directory.join("vectors"));
        wait(|| {
            semantic.sync(character, &memory);
            semantic.settings_loaded()
        });
        semantic.settings.enabled = true;
        semantic.settings.base_url = url.clone();
        semantic.settings.model = "offline-embedding".into();
        semantic.save_settings().unwrap();
        wait(|| {
            semantic.sync(character, &memory);
            semantic.ready(character, &memory)
        });
        studio.composer = "Which tea do I like?".into();
        semantic.query(character, &memory, &studio.composer);
        wait(|| {
            semantic.sync(character, &memory);
            semantic.has_result_for(character, &memory, &studio.composer)
        });
        let provider = &mut studio.library.providers[studio.library.provider];
        provider.route = None;
        provider.base_url = url;
        provider.key_env.clear();
        provider.model = "offline-chat".into();
        studio.turns = vec![
            Turn {
                role: "user".into(),
                content: "Earlier question".into(),
            },
            Turn {
                role: "assistant".into(),
                content: "Earlier answer".into(),
            },
        ];
        studio.send_chat_with_semantic(&mut memory, Some(&mut semantic));
        let request = received.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap();
        let wire = request.to_string();
        assert_eq!(
            wire.matches("The secret tea preference is jasmine").count(),
            1
        );
        assert_eq!(
            wire.matches("Tea should be brewed at 80 degrees").count(),
            1
        );
        let messages = request["messages"].as_array().unwrap();
        assert_eq!(messages[1]["content"], "Earlier question");
        assert_eq!(messages[2]["content"], "Earlier answer");
        assert!(messages[3]["content"].as_str().unwrap().contains("jasmine"));
        assert!(
            messages[4]["content"]
                .as_str()
                .unwrap()
                .contains("80 degrees")
        );
        assert_eq!(messages.last().unwrap()["content"], "Which tea do I like?");
        assert_eq!(studio.turns.len(), 3);
        assert_eq!(studio.turns[2].content, "Which tea do I like?");
        let stored = studio.store.history(character.unwrap()).unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[2].content, "Which tea do I like?");
        assert!(!semantic.has_result_for(character, &memory, "Which tea do I like?"));
        studio.cancel_chat();
        drop(semantic);
        drop(memory);
        std::fs::remove_dir_all(directory).unwrap();
    }
    fn studio() -> (Studio, PathBuf) {
        let directory = std::env::temp_dir().join(format!("studio-controller-{}", Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let id = Uuid::new_v4();
        library.characters.push(Character {
            delegation: Default::default(),
            id,
            name: "Test".into(),
            model: "model.vrm".into(),
            persona: "Friendly".into(),
            greeting: "Hello".into(),
            format: "VRM 1.0".into(),
            scene: None,
            voice: None,
            provider: None,
        });
        library.selected = Some(id);
        (Studio::new(store, library), directory)
    }
    #[test]
    fn character_speech_queue_is_fifo_bounded_and_preserves_focused_settings() {
        let (mut studio, directory) = studio();
        let first = studio.library.selected.unwrap();
        let mut other = studio.library.characters[0].clone();
        other.id = Uuid::new_v4();
        let second = other.id;
        studio.library.characters.push(other);
        let saved_voice = serde_json::to_value(&studio.library.voice).unwrap();
        let config = waifu_studio::voice::VoiceConfig {
            enabled: true,
            delivery: waifu_studio::voice::VoiceDelivery::TimestampSse,
            ..Default::default()
        };
        studio
            .queue_speech(first, config.clone(), "First".into())
            .unwrap();
        studio
            .queue_speech(second, config.clone(), "Second".into())
            .unwrap();
        studio.advance_speech_queue(false);
        assert_eq!(studio.speech_character, Some(first));
        assert_eq!(studio.pending_live.as_ref().unwrap().text, "First");
        let old_id = studio.voice_id;
        studio.voice_busy = false;
        studio.advance_speech_queue(true);
        assert_eq!(
            studio.speech_queue.len(),
            1,
            "Network completion must wait for playback drain"
        );
        studio.advance_speech_queue(false);
        assert_eq!(studio.speech_character, Some(second));
        assert_ne!(
            studio.voice_id, old_id,
            "Stale generation cannot own the next reply"
        );
        assert_eq!(studio.pending_live.as_ref().unwrap().text, "Second");
        assert_eq!(studio.library.selected, Some(first));
        assert_eq!(
            serde_json::to_value(&studio.library.voice).unwrap(),
            saved_voice
        );
        for _ in 0..8 {
            studio
                .queue_speech(first, config.clone(), "Queued".into())
                .unwrap();
        }
        assert!(
            studio
                .queue_speech(first, config, "Too many".into())
                .is_err()
        );
        studio.stop_voice();
        assert!(studio.speech_queue.is_empty());
        assert!(studio.speech_character.is_none());
        studio.advance_speech_queue(false);
        assert!(studio.pending_live.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn design_preview_replaces_scene_owner_with_focused_profile() {
        let (mut studio, directory) = studio();
        let focused = studio.library.selected.unwrap();
        studio.speech_character = Some(Uuid::new_v4());
        studio.scene_speech_active = true;
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::LiveWebSocket;
        studio.begin_design_preview();
        assert_eq!(studio.speech_character, Some(focused));
        assert!(!studio.scene_speech_active);
        assert_eq!(
            studio.active_voice.as_ref().unwrap().delivery,
            waifu_studio::voice::VoiceDelivery::Buffered
        );
        assert_eq!(speaking_visemes(&studio), studio.library.scene.visemes);
        assert_eq!(
            studio.library.voice.delivery,
            waifu_studio::voice::VoiceDelivery::LiveWebSocket
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn scene_roster_and_cancel_preserve_other_replies_and_focused_preview() {
        let (mut studio, directory) = studio();
        let first = studio.library.selected.unwrap();
        let mut other = studio.library.characters[0].clone();
        other.id = Uuid::new_v4();
        let second = other.id;
        studio.library.characters.push(other);
        let config = waifu_studio::voice::VoiceConfig {
            enabled: true,
            delivery: waifu_studio::voice::VoiceDelivery::TimestampSse,
            ..Default::default()
        };
        studio
            .queue_speech(first, config.clone(), "Removed".into())
            .unwrap();
        studio
            .queue_speech(second, config.clone(), "Keep one".into())
            .unwrap();
        studio
            .queue_speech(first, config.clone(), "Remove queued".into())
            .unwrap();
        studio
            .queue_speech(second, config.clone(), "Keep two".into())
            .unwrap();
        studio.advance_speech_queue(false);
        let generation = studio.voice_id;
        studio.retain_scene_speech(&[second]);
        assert_ne!(studio.voice_id, generation);
        assert!(studio.pending_live.is_none());
        assert_eq!(
            studio
                .speech_queue
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Keep one", "Keep two"]
        );
        studio.advance_speech_queue(false);
        assert_eq!(studio.speech_character, Some(second));
        studio.cancel_scene_speech();
        assert!(studio.pending_live.is_none());
        assert!(studio.speech_queue.is_empty());
        studio.speak_with_config(first, config.clone(), "Focused preview".into());
        studio
            .queue_speech(second, config, "Scene queued".into())
            .unwrap();
        let generation = studio.voice_id;
        studio.retain_scene_speech(&[]);
        studio.cancel_scene_speech();
        assert_eq!(studio.voice_id, generation);
        assert_eq!(
            studio.pending_live.as_ref().unwrap().text,
            "Focused preview"
        );
        assert!(studio.speech_queue.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn queued_speech_respects_opt_out_and_does_not_cross_provider_keys() {
        let (mut studio, directory) = studio();
        let id = studio.library.selected.unwrap();
        studio.voice_key = "session-secret".into();
        let mut config = studio.library.voice.clone();
        config.enabled = false;
        studio
            .queue_speech(id, config.clone(), "Disabled".into())
            .unwrap();
        assert!(studio.speech_queue.is_empty());
        config.enabled = true;
        config.endpoint = "https://different.example/v1/tts".into();
        studio
            .queue_speech(id, config, "Other endpoint".into())
            .unwrap();
        assert!(studio.speech_queue[0].key.is_none());
        studio.library.characters.clear();
        studio.advance_speech_queue(false);
        assert!(studio.speech_queue.is_empty());
        assert!(studio.speech_character.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn websocket_reply_filters_tasks_and_stop_prevents_completed_reply_replay() {
        let (mut studio, directory) = studio();
        studio.library.voice.enabled = true;
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::LiveWebSocket;
        let id = Uuid::new_v4();
        studio.begin_reply_speech(id);
        studio.feed_reply_speech(Uuid::new_v4(), "stale");
        studio.feed_reply_speech(id, "Hello. ```wai");
        studio.feed_reply_speech(id, "fu_tasks\n[{\"prompt\":\"do not speak\"}]");
        assert_eq!(studio.pending_ws.as_ref().unwrap().text, "Hello. ");
        assert!(studio.finish_reply_speech(id));
        assert!(studio.pending_ws.as_ref().unwrap().finished);
        studio.stop_voice();
        assert!(studio.pending_ws.is_none());
        assert!(studio.finish_reply_speech(id));
        assert!(!studio.finish_reply_speech(Uuid::new_v4()));
        assert!(!studio.voice_busy);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn speech_chunks_split_sentences_within_bounds() {
        let mut text =
            "Pi is 3.14 and that is fine. Short! Then a longer sentence follows here. Tail"
                .to_owned();
        assert_eq!(
            take_speech_chunk(&mut text, false).unwrap(),
            "Pi is 3.14 and that is fine. Short! Then a longer sentence follows here."
        );
        assert!(take_speech_chunk(&mut text, false).is_none());
        assert_eq!(take_speech_chunk(&mut text, true).unwrap(), "Tail");
        assert!(take_speech_chunk(&mut text, true).is_none());
        let mut text = format!("{}。次", "語".repeat(45));
        assert_eq!(
            take_speech_chunk(&mut text, false).unwrap(),
            format!("{}。", "語".repeat(45))
        );
        let mut text = "word ".repeat(100);
        let chunk = take_speech_chunk(&mut text, false).unwrap();
        assert!(chunk.chars().count() <= SPEECH_CHUNK_MAX && chunk.ends_with("word"));
        let mut text = "\n".repeat(80);
        assert!(take_speech_chunk(&mut text, true).is_none());
    }
    /// Offline Fish-compatible TTS: reports each requested text, answers with WAV.
    fn mock_tts() -> (String, mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                let body = loop {
                    let n = stream.read(&mut buffer).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..n]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let length: usize = String::from_utf8_lossy(&request[..end])
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break request[end + 4..end + 4 + length].to_vec();
                    }
                };
                let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let _ = sender.send(json["text"].as_str().unwrap().to_owned());
                let mut wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x04\0\0\0".to_vec();
                wav.extend_from_slice(&[0; 4]);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    wav.len()
                );
                let _ = stream.write_all(&wav);
            }
        });
        (url, receiver)
    }
    fn next_voice_event(studio: &Studio) -> VoiceEvent {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(event) = studio.voice.lock().unwrap().try_recv() {
                return event;
            }
            assert!(std::time::Instant::now() < deadline, "no voice event");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    fn http_reply_studio() -> (Studio, PathBuf, mpsc::Receiver<String>) {
        let (mut studio, directory) = studio();
        let (url, requests) = mock_tts();
        studio.library.voice =
            waifu_studio::voice::VoiceConfig::preset(waifu_studio::voice::VoiceAdapter::FishLocal);
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::Buffered;
        studio.library.voice.endpoint = url;
        studio.library.voice.enabled = true;
        (studio, directory, requests)
    }
    #[test]
    fn http_reply_synthesizes_sentences_during_stream_with_one_clip_prefetched() {
        let (mut studio, directory, requests) = http_reply_studio();
        let first = "The first sentence is long enough to synthesize early.";
        let second = "The second one follows while the first clip plays.";
        let id = Uuid::new_v4();
        studio.begin_reply_speech(id);
        assert!(studio.voice_busy);
        studio.feed_reply_speech(
            Uuid::new_v4(),
            "stale reply text that must never be spoken. ",
        );
        studio.feed_reply_speech(id, &format!("{first} The second one"));
        assert!(studio.pump_http(false).is_none());
        // Overlap: the first sentence is requested before the LLM finishes.
        assert_eq!(
            requests
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            first
        );
        let event = next_voice_event(&studio);
        assert!(studio.receive_voice_event(event).is_none());
        let Some(VoiceEvent::Ready { audio: clip, .. }) = studio.pump_http(false) else {
            panic!("first clip plays");
        };
        assert!(!clip.bytes.is_empty());
        studio.feed_reply_speech(
            id,
            " follows while the first clip plays. ```waifu_tasks\n[{\"prompt\":\"secret\"}]```",
        );
        assert!(studio.finish_reply_speech(id));
        // Prefetch: the next chunk synthesizes while the first clip is playing.
        assert!(studio.pump_http(true).is_none());
        assert_eq!(
            requests
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            second
        );
        let event = next_voice_event(&studio);
        assert!(studio.receive_voice_event(event).is_none());
        // Bounded: the ready clip waits for playback and nothing else starts.
        assert!(studio.pump_http(true).is_none());
        assert_eq!(studio.pending_http.as_ref().unwrap().ready.len(), 1);
        assert!(studio.voice_busy);
        assert!(studio.pump_http(false).is_some());
        assert!(studio.pending_http.is_none() && !studio.voice_busy);
        assert!(
            requests
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn http_reply_stop_discards_in_flight_chunk_and_never_replays() {
        let (mut studio, directory, requests) = http_reply_studio();
        let id = Uuid::new_v4();
        studio.begin_reply_speech(id);
        let reply_voice = studio.voice_id;
        studio.feed_reply_speech(id, "This sentence is already being synthesized now. More");
        studio.pump_http(false);
        requests
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        studio.stop_voice();
        assert!(studio.pending_http.is_none() && !studio.voice_busy);
        studio.feed_reply_speech(id, " text after stop is ignored.");
        assert!(studio.finish_reply_speech(id));
        assert!(studio.pump_http(false).is_none());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(studio.voice.lock().unwrap().try_recv().is_none());
        assert_ne!(studio.voice_id, reply_voice);
        assert!(
            requests
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    /// Offline Fish-local WAV stream server: reports each requested text and
    /// withholds the end of the first stream until released.
    fn mock_stream_tts() -> (String, mpsc::Receiver<String>, mpsc::Sender<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        let (release, held) = mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(Some(held)));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let (sender, held) = (sender.clone(), held.clone());
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut buffer = [0; 4096];
                    let body = loop {
                        let n = stream.read(&mut buffer).unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..n]);
                        let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let length: usize = String::from_utf8_lossy(&request[..end])
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break request[end + 4..end + 4 + length].to_vec();
                        }
                    };
                    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let text = json["text"].as_str().unwrap();
                    let hold = if text.starts_with("The first sentence") {
                        held.lock().unwrap().take()
                    } else {
                        None
                    };
                    let _ = sender.send(text.to_owned());
                    let mut wav = b"RIFF\xff\xff\xff\xffWAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\xff\xff\xff\xff".to_vec();
                    wav.extend((0..1600).flat_map(|_| 8192i16.to_le_bytes()));
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.write_all(&wav);
                    let _ = stream.flush();
                    if let Some(hold) = hold {
                        let _ = hold.recv_timeout(std::time::Duration::from_secs(10));
                    }
                });
            }
        });
        (url, receiver, release)
    }
    #[test]
    fn http_wav_reply_prefetches_next_chunk_during_playback_and_character_switch_cancels() {
        let (mut studio, directory) = studio();
        let (url, requests, release) = mock_stream_tts();
        studio.library.voice =
            waifu_studio::voice::VoiceConfig::preset(waifu_studio::voice::VoiceAdapter::FishLocal);
        assert_eq!(
            studio.library.voice.delivery,
            waifu_studio::voice::VoiceDelivery::HttpWavStream
        );
        studio.library.voice.endpoint = url;
        studio.library.voice.enabled = true;
        let first = "The first sentence is long enough to synthesize early.";
        let second = "The second sentence downloads while the first one plays.";
        let id = Uuid::new_v4();
        studio.begin_reply_speech(id);
        studio.feed_reply_speech(id, &format!("{first} {second} More"));
        assert!(studio.pump_http(false).is_none());
        let wait = std::time::Duration::from_secs(10);
        let mut arrived = vec![requests.recv_timeout(wait).unwrap()];
        let event = next_voice_event(&studio);
        assert!(studio.receive_voice_event(event).is_none());
        let Some(VoiceEvent::Streaming { audio: playing, .. }) = studio.pump_http(false) else {
            panic!("first stream plays");
        };
        // The next chunk is requested while the first stream is still open.
        assert!(studio.pump_http(true).is_none());
        arrived.push(requests.recv_timeout(wait).unwrap());
        arrived.sort();
        assert_eq!(arrived, vec![first.to_owned(), second.to_owned()]);
        assert!(!playing.is_finished() && !playing.is_failed());
        assert_eq!(studio.pending_http.as_ref().unwrap().outstanding, 1);
        let reply_voice = studio.voice_id;
        let selected = studio.library.selected.unwrap();
        studio.select_character(selected);
        assert!(playing.is_failed());
        assert!(studio.pending_http.is_none() && !studio.voice_busy && studio.stop_audio);
        assert_ne!(studio.voice_id, reply_voice);
        release.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(studio.voice.lock().unwrap().try_recv().is_none());
        assert!(studio.pump_http(false).is_none());
        assert!(
            requests
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn http_reply_failure_stops_reply_and_websocket_default_is_unchanged() {
        let (mut studio, directory) = studio();
        studio.library.voice.enabled = true;
        // Default delivery is the live WebSocket and never uses HTTP chunking.
        studio.begin_reply_speech(Uuid::new_v4());
        assert!(studio.pending_ws.is_some() && studio.pending_http.is_none());
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::TimestampSse;
        studio.begin_reply_speech(Uuid::new_v4());
        assert!(studio.pending_http.is_none() && studio.reply_speech_job.is_none());
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::Buffered;
        let id = Uuid::new_v4();
        studio.begin_reply_speech(id);
        let voice = studio.voice_id;
        studio.pending_http.as_mut().unwrap().outstanding = 1;
        assert!(
            studio
                .receive_voice_event(VoiceEvent::Failed {
                    id: voice,
                    message: "boom".into(),
                })
                .is_none()
        );
        assert_eq!(studio.error.as_deref(), Some("boom"));
        assert!(studio.pending_http.is_none() && !studio.voice_busy && studio.stop_audio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn websocket_preview_reserves_text_budget_for_voice_direction() {
        let (mut studio, directory) = studio();
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::TimestampWebSocket;
        studio.library.voice.direction = "Warm".into();
        studio.speak("語".repeat(13_000));
        let request = studio.pending_ws.as_ref().unwrap();
        assert!(request.finished);
        assert_eq!(
            request.text.chars().count(),
            12_000 - "[Warm] ".chars().count()
        );
        studio.stop_voice();
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn live_speech_queue_is_newest_only_and_character_change_cancels_it() {
        let (mut studio, directory) = studio();
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::TimestampSse;
        studio.speak("First".into());
        let first_id = studio.voice_id;
        studio.speak("Newest".into());
        assert_ne!(studio.voice_id, first_id);
        assert_eq!(studio.pending_live.as_ref().unwrap().text, "Newest");
        assert!(studio.voice_busy && studio.stop_audio);
        let id = studio.library.selected.unwrap();
        studio.select_character(id);
        assert!(studio.pending_live.is_none());
        assert!(!studio.voice_busy);
        assert!(studio.stop_audio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn duplicate_has_independent_history_and_keeps_customization() {
        let (mut studio, directory) = studio();
        let original = studio.library.selected.unwrap();
        studio.library.scene.background = [0.2, 0.3, 0.4];
        studio
            .store
            .save_history(
                original,
                &[Turn {
                    role: "user".into(),
                    content: "Old conversation".into(),
                }],
            )
            .unwrap();
        studio.duplicate_character();
        assert_ne!(studio.library.selected, Some(original));
        assert_eq!(studio.library.characters.len(), 2);
        assert!(studio.turns.is_empty());
        assert_eq!(studio.library.scene.background, [0.2, 0.3, 0.4]);
        studio.select_character(original);
        assert_eq!(studio.turns[0].content, "Old conversation");
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn removing_last_profile_requests_unload_and_clears_pending_work() {
        let (mut studio, directory) = studio();
        studio.remove_character();
        assert!(studio.library.selected.is_none());
        assert!(studio.requested_model.is_none());
        assert!(studio.unload_model && studio.stop_audio);
        assert!(studio.turns.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
