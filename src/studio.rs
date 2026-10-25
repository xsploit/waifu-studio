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
#[cfg(test)]
use waifu_studio::reply_speech::{SPEECH_CHUNK_MAX, take_speech_chunk};
use waifu_studio::reply_timing::{ReplyTiming, Stage};
use waifu_studio::{
    data::{ExternalMessage, Library, Store, Turn, find_models, trim_history},
    extension_lifecycle::{LifecycleEvent, LifecycleJournal, ReplyOutcome, SpeechReceipts},
    extension_protocol::{EmotionOwner, ErrorCode, IntegrationOrigin, MAX_TEXT_BYTES, Target},
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
    Extensions,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Studio,
    Desktop,
}

enum FileEvent {
    Models(Vec<PathBuf>),
    LocalAudio(u64, Option<Uuid>, Result<Vec<u8>, String>),
    Background(PathBuf),
    Animation(PathBuf),
    Image(PathBuf, Option<Uuid>),
}

struct QueuedSpeech {
    scene_reply: bool,
    reply_id: Option<Uuid>,
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

struct PendingAppraisal {
    turn: Uuid,
    character: Uuid,
    affect: waifu_studio::reply_affect::AffectConfig,
    decision: waifu_studio::recipient_decision::DecisionConfig,
    job: waifu_studio::appraisal::AppraisalJob,
}

struct PendingMemorySend {
    character: Uuid,
    draft: String,
    started: std::time::Instant,
}

#[derive(Resource)]
pub struct Studio {
    pub lifecycle: LifecycleJournal,
    speech_receipts: SpeechReceipts,
    speech_reply_id: Option<Uuid>,
    pub facts: waifu_studio::facts_memory::FactsMemoryState,
    /// Evaluation models advertised by the shared decision endpoint (UI dropdowns).
    pub decision_models: waifu_studio::model_catalog::ModelCatalog,
    voice_key_hint_shown: bool,
    pub affect: waifu_studio::reply_affect::AffectLibrary,
    reply_filter: Option<waifu_studio::reply_affect::ReplyAffectFilter>,
    reply_affect_requested: bool,
    reply_affect_config: Option<waifu_studio::reply_affect::AffectConfig>,
    appraisal: Option<PendingAppraisal>,
    pending_memory_send: Option<PendingMemorySend>,
    /// Incoming-message appraisal for (chat job, character), known before the reply ends.
    pub appraised_affect: Option<(Uuid, Uuid, waifu_studio::reply_affect::AffectEmotion)>,
    pub appraisal_status: String,
    pub completed_chat: Option<(Uuid, Result<String, String>)>,
    pub next_loop_turn: bool,
    pub tool_runtime: waifu_studio::companion_tools::ToolRuntime,
    pub loop_chat_job: Option<Uuid>,
    /// (job, character, emotion) parsed from that job's opt-in affect block.
    pub completed_affect: Option<(Uuid, Uuid, waifu_studio::reply_affect::AffectEmotion)>,
    pub proposals: Vec<waifu_studio::delegation::TaskProposal>,
    pub attachments: Vec<waifu_studio::attachments::ImageAttachment>,
    pub documents: Vec<waifu_studio::attachments::DocumentAttachment>,
    pending_attachments: VecDeque<PathBuf>,
    pub attachment_drop_hovered: bool,
    attachment_worker: Mutex<waifu_studio::attachments::AttachmentWorker>,
    pub voice_key: String,
    pub voice_busy: bool,
    provider_failure_wait: Option<(u64, std::time::Instant)>,
    pub speech_character: Option<Uuid>,
    scene_speech_active: bool,
    active_voice: Option<waifu_studio::voice::VoiceConfig>,
    speech_queue: VecDeque<QueuedSpeech>,
    voice_playback_speed: f32,
    voice: Mutex<VoiceWorker>,
    voice_id: u64,
    pending_live: Option<PendingLiveSpeech>,
    pending_sse: Option<waifu_studio::reply_speech::SseReplySpeech>,
    pending_ws: Option<PendingWsSpeech>,
    pending_http: Option<waifu_studio::reply_speech::HttpReplySpeech>,
    reply_speech_job: Option<Uuid>,
    stop_audio: bool,
    local_preview: Option<Vec<u8>>,
    dialog_open: Arc<AtomicBool>,
    unload_model: bool,
    pub store: Store,
    pub library: Library,
    pub tab: Tab,
    pub composer: String,
    pub turns: Vec<Turn>,
    // Only a successful explicit history reload clears an unconfirmed external
    // commit. Ordinary unsaved local history keeps its established send behavior.
    external_history_unconfirmed: bool,
    last_external_reply_started: Option<(Uuid, std::time::Instant)>,
    pub pending_response: String,
    pub reply_timing: Option<ReplyTiming>,
    pub job: Option<ChatJob>,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Offscreen review captures must not perform incidental provider discovery.
    pub model_discovery_enabled: bool,
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
            lifecycle: Default::default(),
            speech_receipts: Default::default(),
            speech_reply_id: None,
            facts,
            decision_models: Default::default(),
            voice_key_hint_shown: false,
            affect: Default::default(),
            reply_filter: None,
            reply_affect_requested: false,
            reply_affect_config: None,
            appraisal: None,
            pending_memory_send: None,
            appraised_affect: None,
            appraisal_status: String::new(),
            completed_chat: None,
            next_loop_turn: false,
            tool_runtime: Default::default(),
            loop_chat_job: None,
            completed_affect: None,
            proposals: Vec::new(),
            attachments: vec![],
            documents: vec![],
            pending_attachments: VecDeque::new(),
            attachment_drop_hovered: false,
            attachment_worker: Mutex::new(Default::default()),
            dialog_open: Arc::new(AtomicBool::new(false)),
            unload_model: false,
            voice_key: String::new(),
            voice_busy: false,
            provider_failure_wait: None,
            speech_character: None,
            scene_speech_active: false,
            active_voice: None,
            speech_queue: VecDeque::new(),
            voice_playback_speed: 1.0,
            voice: Mutex::new(VoiceWorker::new()),
            voice_id: 0,
            pending_live: None,
            pending_sse: None,
            pending_ws: None,
            pending_http: None,
            reply_speech_job: None,
            stop_audio: false,
            local_preview: None,
            store,
            library,
            tab: Tab::Chat,
            composer: String::new(),
            turns: vec![],
            external_history_unconfirmed: false,
            last_external_reply_started: None,
            pending_response: String::new(),
            reply_timing: None,
            job: None,
            error: None,
            notice: None,
            model_discovery_enabled: true,
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
        let previous = self.library.selected;
        self.cancel_chat();
        self.affect.take_pending(id);
        self.stop_voice();
        self.attachments.clear();
        self.documents.clear();
        self.pending_attachments.clear();
        self.proposals.clear();
        self.attachment_worker = Mutex::new(Default::default());
        if !self.library.characters.iter().any(|c| c.id == id) {
            return;
        }
        self.library.select_character(id);
        self.record_selection(previous);
        self.facts.select_character(Some(id));
        self.requested_model = self.library.selected_character().map(|c| c.model.clone());
        self.frame_requested = true;
        self.composer.clear();
        self.turns.clear();
        match self.store.history(id) {
            Ok(turns) => {
                self.turns = turns;
                self.external_history_unconfirmed = false;
            }
            Err(error) => self.error = Some(format!("Conversation could not be read: {error}")),
        }
        self.chat_character = Some(id);
        self.connected = false;
        self.save();
    }
    fn record_selection(&mut self, previous: Option<Uuid>) {
        // Observe already-terminal old speech before the selection record. A
        // still-live decoder keeps its source identity and reports stop later;
        // selection never fabricates device/decoder termination.
        self.speech_receipts.poll(&mut self.lifecycle);
        self.lifecycle
            .record_selection(previous, self.library.selected);
    }
    pub fn remove_character(&mut self) {
        let Some(id) = self.library.selected else {
            return;
        };
        self.cancel_chat();
        self.stop_voice();
        self.attachments.clear();
        self.documents.clear();
        self.pending_attachments.clear();
        self.proposals.clear();
        self.attachment_worker = Mutex::new(Default::default());
        self.library.characters.retain(|c| c.id != id);
        self.affect.entries.remove(&id);
        self.library.selected = None;
        self.record_selection(Some(id));
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
    /// A model/personality combination is a saved character profile. Existing
    /// combinations restore their own history/settings; new ones never overwrite
    /// the source profile or inherit its conversation and durable memory identity.
    pub fn use_character_combination(&mut self, model: PathBuf, persona: String, name: String) {
        let Some(source) = self.library.characters.iter().find(|c| c.model == model) else {
            self.error = Some("Choose an avatar already in your library.".into());
            return;
        };
        let format = source.format.clone();
        if let Some(id) = self
            .library
            .characters
            .iter()
            .find(|c| c.model == model && c.persona == persona)
            .map(|c| c.id)
        {
            if self.library.selected != Some(id) {
                self.select_character(id);
            }
            return;
        }
        self.library.capture_selected_settings();
        let Some(mut profile) = self.library.selected_character().cloned() else {
            return;
        };
        profile.id = Uuid::new_v4();
        profile.name = name.trim().chars().take(100).collect();
        if profile.name.is_empty() {
            profile.name = "Companion profile".into();
        }
        profile.model = model;
        profile.format = format;
        profile.persona = persona;
        let id = profile.id;
        self.library.characters.push(profile);
        self.select_character(id);
    }
    pub fn speaking_character(&self) -> Option<Uuid> {
        self.speech_character
    }
    /// The previous model can remain visible during replacement. Automatic
    /// behavior must target only the selected character's ready avatar lifetime.
    pub fn animation_target(
        &self,
        avatar: &waifu_studio::loading::AvatarLoadState,
    ) -> Option<Entity> {
        (avatar.status == waifu_studio::loading::LoadStatus::Ready
            && self.requested_model.is_none()
            && self.library.selected_character().is_some_and(|character| {
                avatar
                    .requested_path
                    .as_deref()
                    .is_some_and(|path| std::path::Path::new(path) == character.model)
            }))
        .then_some(avatar.active)
        .flatten()
    }
    pub fn speech_generation(&self) -> u64 {
        self.voice_id
    }
    /// Match the immutable source owner and voice lifetime, then require actual
    /// decoder consumption. Deferred despawns cannot make old audio talk for a
    /// newly selected character or a new utterance by the same character.
    pub fn speech_is_playing(&self, source: &SpeechPlayback) -> bool {
        source.character == self.speech_character
            && source.generation == self.voice_id
            && source.playback.is_playing()
    }
    pub fn stop_voice(&mut self) {
        self.speech_receipts.cancel_generation(self.voice_id);
        self.speech_reply_id = None;
        self.provider_failure_wait = None;
        self.local_preview = None;
        self.speech_queue.clear();
        self.speech_character = None;
        self.scene_speech_active = false;
        self.active_voice = None;
        self.pending_live = None;
        self.pending_sse = None;
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
    fn wait_for_provider_failure(&mut self) {
        self.provider_failure_wait
            .get_or_insert((self.voice_id, std::time::Instant::now()));
        self.voice_busy = true;
    }
    fn check_provider_failure_wait(&mut self, now: std::time::Instant) {
        if let Some((generation, since)) = self.provider_failure_wait {
            if generation != self.voice_id {
                self.provider_failure_wait = None;
            } else if now.saturating_duration_since(since) >= std::time::Duration::from_secs(5) {
                self.fail_current_speech(
                    "Speech provider stopped before reporting its failure.".into(),
                );
            }
        }
    }
    /// Invalidate the failed owner while retaining other characters' queued replies.
    fn fail_current_speech(&mut self, message: String) {
        let queued = std::mem::take(&mut self.speech_queue);
        self.stop_voice();
        self.speech_queue = queued;
        self.error = Some(message);
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
            reply_id: None,
            character,
            config,
            text,
            key,
        });
    }
    /// Group replies retain their voice and identity without changing the editor selection.
    #[cfg(test)]
    pub fn queue_speech(
        &mut self,
        character: Uuid,
        config: waifu_studio::voice::VoiceConfig,
        text: String,
    ) -> Result<(), String> {
        self.queue_correlated_speech(None, character, config, text)
    }
    pub fn queue_reply_speech(
        &mut self,
        reply_id: Uuid,
        character: Uuid,
        config: waifu_studio::voice::VoiceConfig,
        text: String,
    ) -> Result<(), String> {
        self.queue_correlated_speech(Some(reply_id), character, config, text)
    }
    fn queue_correlated_speech(
        &mut self,
        reply_id: Option<Uuid>,
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
            reply_id,
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
    fn speech_call_routes(
        &self,
        outputs: &waifu_studio::extension_speech_output::SpeechOutputs,
    ) -> Vec<waifu_studio::extension_runtime::AudioPlaybackRoute> {
        if !self
            .active_voice
            .as_ref()
            .is_some_and(|config| config.output != waifu_studio::voice::VoiceOutput::LocalOnly)
        {
            return Vec::new();
        }
        self.speech_character
            .map(|id| outputs.ready_routes_for_character(id))
            .unwrap_or_default()
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
            reply_id,
            character,
            config,
            text,
            key,
        } = request;
        self.speech_character = Some(character);
        self.speech_reply_id = reply_id;
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
        // Connect as soon as the reply starts, in parallel with chat generation.
        // No text/flush is sent until the speech filters have produced content.
        if !request.opened && request.finished && request.text.trim().is_empty() {
            self.voice_busy = false;
            return;
        }
        let result = (|| {
            if !request.opened {
                ws.timestamps = request.config.delivery
                    == waifu_studio::voice::VoiceDelivery::TimestampWebSocket;
                ws.open(request.id, &request.config, request.key.take())?;
                self.mark_reply_speech(Stage::SpeechConnect);
                request.opened = true;
            }
            if !request.text.is_empty() {
                ws.push_text(&request.text)?;
                self.mark_reply_speech(Stage::SpeechQueued);
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
        self.speech_reply_id = Some(id);
        self.reply_speech_job = None;
        // The loop chooses whether to narrate after it knows this is progress
        // or a final answer. Never speak hidden protocol/tool proposals.
        if self.loop_chat_job == Some(id) {
            return;
        }
        // Voice is on by default; without a hosted key, stay quiet and say so once.
        let voice = &self.library.voice;
        if voice.enabled
            && matches!(
                voice.adapter,
                waifu_studio::voice::VoiceAdapter::FishHosted
                    | waifu_studio::voice::VoiceAdapter::Vercel
            )
            && self.voice_key.is_empty()
            && std::env::var_os(voice.api_key_env.trim()).is_none_or(|v| v.is_empty())
            && reqwest::Url::parse(&voice.endpoint).is_ok_and(|url| {
                !matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                )
            })
        {
            if !self.voice_key_hint_shown {
                self.voice_key_hint_shown = true;
                self.notice = Some(format!(
                    "Replies aren't spoken yet: add {} under Connections → Keys.",
                    voice.api_key_env.trim()
                ));
            }
            return;
        }
        if self.library.voice.enabled && self.library.voice.delivery.is_websocket() {
            self.reply_speech_job = Some(id);
            self.queue_ws();
        } else if self.library.voice.enabled
            && self.library.voice.delivery == waifu_studio::voice::VoiceDelivery::TimestampSse
        {
            let config = self.library.voice.clone();
            self.reply_speech_job = Some(id);
            self.speech_character = self.library.selected;
            self.voice_playback_speed = playback_speed(&config);
            self.active_voice = Some(config.clone());
            self.pending_sse = Some(waifu_studio::reply_speech::SseReplySpeech::new(
                self.voice_id,
                config.clone(),
                self.speech_key(&config),
            ));
            self.voice_busy = true;
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
            self.pending_http = Some(waifu_studio::reply_speech::HttpReplySpeech::new(
                self.voice_id,
                config.clone(),
                self.speech_key(&config),
            ));
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
        if let Some(request) = &mut self.pending_sse {
            request.push(delta);
        }
    }
    fn pump_sse(&mut self, fish: &mut waifu_studio::fish_live::FishLiveState, playing: bool) {
        let Some(request) = self.pending_sse.as_mut() else {
            return;
        };
        if let Err(error) = request.pump(fish, playing) {
            fish.cancel();
            self.stop_voice();
            self.error = Some(error);
        } else if request.is_done() && !playing {
            self.pending_sse = None;
            self.voice_busy = false;
        }
    }
    /// Requests upcoming chunks while worker slots and the lookahead bound allow,
    /// and returns the next ready clip or stream once nothing else is playing.
    fn pump_http(&mut self, playing: bool) -> Option<VoiceEvent> {
        if self.provider_failure_wait.is_some() {
            return None;
        }
        let request = self.pending_http.as_mut()?;
        let before = request.outstanding();
        let result = request.pump(&mut self.voice.lock().unwrap(), playing);
        if request.outstanding() > before {
            self.mark_reply_speech(Stage::SpeechQueued);
        }
        match result {
            Err(error) => {
                self.stop_voice();
                self.error = Some(error);
                None
            }
            Ok(next) => {
                if self.pending_http.as_ref().is_some_and(|r| r.is_done()) {
                    self.pending_http = None;
                    self.voice_busy = false;
                }
                next
            }
        }
    }
    fn mark_reply_speech(&self, stage: Stage) {
        if self.reply_speech_job.is_some()
            && let Some(timing) = &self.reply_timing
        {
            timing.mark(stage);
        }
    }
    /// Routes a current worker event; returns events that should play now.
    fn receive_voice_event(&mut self, event: VoiceEvent) -> Option<VoiceEvent> {
        if self.provider_failure_wait.is_some() {
            if let VoiceEvent::Failed { id, message } = event
                && id == self.voice_id
            {
                self.fail_current_speech(message);
            }
            return None;
        }
        let Some(request) = self
            .pending_http
            .as_mut()
            .filter(|request| request.id() == event.id())
        else {
            self.voice_busy = false;
            return Some(event);
        };
        if let Err(message) = request.receive(event) {
            self.stop_voice();
            self.error = Some(message);
        }
        None
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
        if let Some(request) = &mut self.pending_sse {
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
        config.output = waifu_studio::voice::VoiceOutput::LocalOnly;
        self.active_voice = Some(config);
        self.scene_speech_active = false;
    }
    fn receive_local_preview(
        &mut self,
        generation: u64,
        character: Option<Uuid>,
        result: Result<Vec<u8>, String>,
    ) {
        if generation == self.voice_id && character == self.library.selected {
            match result {
                Ok(bytes) => self.local_preview = Some(bytes),
                Err(error) => self.error = Some(error),
            }
        }
    }
    /// Explicit local playback; no synthesis provider or saved voice change.
    pub fn preview_audio_file(&mut self) {
        if self.dialog_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let flag = self.dialog_open.clone();
        let tx = self.file_tx.clone();
        let generation = self.voice_id;
        let character = self.library.selected;
        std::thread::spawn(move || {
            let _guard = DialogGuard(flag);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Preview local audio for lip sync")
                .add_filter("Audio", &["wav", "mp3"])
                .pick_file()
            {
                let result = read_local_preview(&path);
                let _ = tx.send(FileEvent::LocalAudio(generation, character, result));
            }
        });
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
        let mut config = self.library.voice.clone();
        config.output = waifu_studio::voice::VoiceOutput::LocalOnly;
        self.speak_with_config(self.library.selected.unwrap_or_default(), config, text);
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
        // Library imports stage profile changes synchronously. Restore the
        // original host selection before committing the final import through
        // select_character, which cancels old work and emits one source change.
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
    pub fn open_attachment(&self) {
        if self.attachments.len() + self.documents.len() >= waifu_studio::attachments::MAX_FILES
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
                .set_title("Attach files to your next message")
                .add_filter(
                    "Images and documents",
                    &[
                        "png", "jpg", "jpeg", "webp", "pdf", "docx", "txt", "md", "csv", "json",
                        "log",
                    ],
                )
                .add_filter("All files (including source code)", &["*"])
                .pick_file()
            {
                let _ = tx.send(FileEvent::Image(path, character));
            }
        });
    }
    pub fn preparing_image(&self) -> bool {
        !self.pending_attachments.is_empty() || self.attachment_worker.lock().unwrap().is_busy()
    }
    pub fn queue_attachment(&mut self, path: PathBuf) -> Result<(), String> {
        if self.library.selected.is_none() {
            return Err("Choose a character before attaching files".into());
        }
        let occupied = self.attachments.len()
            + self.documents.len()
            + self.pending_attachments.len()
            + usize::from(self.attachment_worker.lock().unwrap().is_busy());
        if occupied >= waifu_studio::attachments::MAX_FILES {
            return Err("Attach up to two images and four documents per message".into());
        }
        self.pending_attachments.push_back(path);
        Ok(())
    }
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty() || !self.documents.is_empty() || self.preparing_image()
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
        if !self.model_discovery_enabled {
            return;
        }
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
        if !self.model_discovery_enabled {
            return;
        }
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
    pub fn cancel_appraisal(&mut self) {
        if let Some(id) = self.library.selected {
            self.affect.take_pending(id);
        }
        self.appraisal = None;
        self.appraised_affect = None;
        self.reply_affect_config = None;
        self.appraisal_status.clear();
    }
    fn poll_appraisal(&mut self) {
        let Some(pending) = self.appraisal.as_ref() else {
            return;
        };
        let current_turn = self.job.as_ref().map(|job| job.id).or_else(|| {
            self.completed_chat
                .as_ref()
                .filter(|(_, result)| result.is_ok())
                .map(|(id, _)| *id)
        });
        if self.library.selected != Some(pending.character)
            || current_turn != Some(pending.turn)
            || self.library.scene.affect != pending.affect
            || self.library.decision != pending.decision
        {
            self.cancel_appraisal();
            self.appraisal_status = "Stale appraisal discarded".into();
            return;
        }
        let Some(result) = pending.job.poll() else {
            return;
        };
        let pending = self.appraisal.take().unwrap();
        match result {
            Ok(Some(reply)) => {
                self.affect
                    .entries
                    .entry(pending.character)
                    .or_default()
                    .config = pending.affect;
                match self.affect.apply_reply_tracked(pending.character, reply) {
                    Ok(transition) => {
                        if let Some(transition) = transition {
                            self.lifecycle.record(LifecycleEvent::EmotionChanged {
                                character: pending.character,
                                owner: EmotionOwner::Studio,
                                previous: transition.previous,
                                emotion: transition.emotion,
                                reply_id: Some(pending.turn),
                            });
                        }
                        self.appraised_affect =
                            Some((pending.turn, pending.character, reply.emotion));
                        self.appraisal_status = format!(
                            "Jev: {:?} · V {:+.2} A {:+.2} D {:+.2}",
                            reply.emotion, reply.valence, reply.arousal, reply.dominance
                        );
                    }
                    Err(error) => self.appraisal_status = error,
                }
            }
            Ok(None) => self.appraisal_status = "Jev abstained; mood unchanged".into(),
            Err(error) => self.appraisal_status = error,
        }
    }
    pub fn cancel_chat(&mut self) {
        self.pending_memory_send = None;
        if let Some(timing) = &self.reply_timing {
            timing.cancel();
        }
        self.cancel_appraisal();
        if let Some(id) = self.chat_character {
            self.affect.take_pending(id);
        }
        if let Some(job) = self.job.take() {
            self.lifecycle.record(LifecycleEvent::ReplyFinished {
                reply_id: job.id,
                character: self.chat_character,
                outcome: ReplyOutcome::Cancelled,
            });
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
    /// Persist attributed external context without submitting a user message,
    /// starting generation, or offering claims to either memory intake path.
    /// The host adapter separately authenticates the current target generation
    /// and rejects concurrent scene work before calling this focused sink.
    pub(crate) fn has_external_response_work(&self) -> bool {
        self.chat_busy() || self.appraisal.is_some()
    }
    pub fn chat_busy(&self) -> bool {
        self.job.is_some() || self.pending_memory_send.is_some()
    }
    pub fn deliver_external(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        text: &str,
    ) -> Result<Uuid, (ErrorCode, String)> {
        if target.validate().is_err()
            || self
                .library
                .selected_character()
                .map(|character| character.id)
                != Some(target.uuid)
            || self.chat_character != Some(target.uuid)
        {
            return Err((
                ErrorCode::StaleTarget,
                "Focused conversation changed".into(),
            ));
        }
        if self.has_external_response_work() {
            return Err((
                ErrorCode::Busy,
                "Focused conversation is generating or appraising".into(),
            ));
        }
        if origin.validate().is_err()
            || text.trim().is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.contains('\0')
        {
            return Err((
                ErrorCode::InvalidRequest,
                "Invalid external message or attribution".into(),
            ));
        }
        // A previous write may have renamed successfully even when its readback
        // failed. Never overwrite that disk history from a stale UI snapshot.
        // Greetings and composer drafts are not inserted into stored turns.
        if self.external_history_unconfirmed {
            return Err((
                ErrorCode::Unavailable,
                "Reload the conversation to reconcile the previous external delivery".into(),
            ));
        }
        let saved = match self.store.history(target.uuid) {
            Ok(saved) => saved,
            Err(_) => {
                self.external_history_unconfirmed = true;
                return Err((
                    ErrorCode::Unavailable,
                    "Conversation history could not be read; reload before delivering".into(),
                ));
            }
        };
        if saved != self.turns {
            self.external_history_unconfirmed = true;
            return Err((
                ErrorCode::Unavailable,
                "Conversation history differs from the visible history; reload before delivering"
                    .into(),
            ));
        }
        let message_id = Uuid::new_v4();
        let turn = Turn::external(
            text,
            ExternalMessage {
                message_id,
                origin: origin.clone(),
            },
        )
        .map_err(|error| (ErrorCode::InvalidRequest, error))?;
        let mut candidate = self.turns.clone();
        candidate.push(turn);
        trim_history(&mut candidate);
        if self
            .store
            .save_external_history(target.uuid, &candidate, message_id)
            .is_err()
        {
            self.external_history_unconfirmed = true;
            return Err((ErrorCode::Unavailable, "External message could not be confirmed on disk; reload the conversation before retrying".into()));
        }
        self.turns = candidate;
        self.frame_requested = true;
        Ok(message_id)
    }

    pub fn send_chat(&mut self, memory: &mut waifu_studio::memory::MemoryState) {
        self.send_chat_with_semantic(memory, None);
    }

    pub fn send_chat_with_semantic(
        &mut self,
        memory: &mut waifu_studio::memory::MemoryState,
        mut semantic: Option<&mut waifu_studio::semantic_memory::SemanticMemory>,
    ) {
        if self.chat_busy() {
            return;
        }
        if self.external_history_unconfirmed {
            self.notice = Some("Reload the conversation to reconcile the previous external delivery before sending.".into());
            return;
        }
        if self.preparing_image() {
            self.notice = Some("Preparing attachments. Send when they are ready.".into());
            return;
        }
        if self.composer.trim().is_empty() && self.has_attachments() {
            self.composer = "Please review the attached files.".into();
        }
        memory.select_character(self.library.selected);
        memory.poll();
        self.facts.select_character(self.library.selected);
        self.facts.poll();
        if let (Some(state), Some(character)) = (semantic.as_deref_mut(), self.library.selected) {
            let source = waifu_studio::semantic_memory::MemorySources {
                scope: state.scope(),
                memory,
                facts: &self.facts,
            };
            state.sync(Some(character), &source);
            if !self.composer.trim().is_empty()
                && self.composer.len() <= 32_000
                && !self.next_loop_turn
                && state.automatic_lookup_for(Some(character), &source)
                && !state.has_result_for(Some(character), &source, &self.composer)
            {
                state.query(Some(character), &source, &self.composer);
                self.pending_memory_send = Some(PendingMemorySend {
                    character,
                    draft: self.composer.clone(),
                    started: std::time::Instant::now(),
                });
                self.notice = Some("Finding related memories…".into());
                return;
            }
        }
        self.start_chat_with_semantic(memory, semantic, None);
    }

    fn poll_memory_send(
        &mut self,
        memory: &mut waifu_studio::memory::MemoryState,
        semantic: &mut waifu_studio::semantic_memory::SemanticMemory,
    ) {
        let Some(pending) = &self.pending_memory_send else {
            return;
        };
        if self.library.selected != Some(pending.character)
            || self.composer != pending.draft
            || self.job.is_some()
        {
            self.pending_memory_send = None;
            self.notice =
                Some("Memory lookup send cancelled because the draft or character changed.".into());
            return;
        }
        let source = waifu_studio::semantic_memory::MemorySources {
            scope: semantic.scope(),
            memory,
            facts: &self.facts,
        };
        let ready = semantic.has_result_for(Some(pending.character), &source, &pending.draft);
        if !ready
            && semantic.automatic_lookup_for(Some(pending.character), &source)
            && pending.started.elapsed() < std::time::Duration::from_secs(10)
        {
            return;
        }
        self.pending_memory_send = None;
        self.notice = (!ready)
            .then(|| "Semantic lookup unavailable; using local recall for this reply.".into());
        self.start_chat_with_semantic(memory, Some(semantic), None);
    }

    /// Called by the host only after delivery and current-incarnation checks.
    /// Reuses the latest attributed turn without consuming a local draft or
    /// treating third-party text as authored-user memory evidence.
    pub fn respond_to_external(
        &mut self,
        target: Target,
        message_id: Uuid,
        memory: &mut waifu_studio::memory::MemoryState,
    ) -> Result<Uuid, String> {
        if !self.library.scene.reply_to_external_messages {
            return Err("Replies to incoming messages are turned off".into());
        }
        if target.validate().is_err() || self.chat_character != Some(target.uuid) {
            return Err("External reply target is unavailable".into());
        }
        if self
            .last_external_reply_started
            .is_some_and(|(_, started)| started.elapsed() < std::time::Duration::from_secs(10))
        {
            return Err("Incoming-message replies are cooling down".into());
        }
        if self.library.selected != Some(target.uuid)
            || self.job.is_some()
            || self.appraisal.is_some()
            || self.external_history_unconfirmed
        {
            return Err("External reply is unavailable for the current conversation".into());
        }
        if !self.turns.last().is_some_and(|turn| {
            turn.role == "external"
                && turn.validate_attribution().is_ok()
                && turn
                    .external
                    .as_ref()
                    .is_some_and(|external| external.message_id == message_id)
        }) {
            return Err("External reply requires the latest attributed message".into());
        }
        match self.store.history(target.uuid) {
            Ok(saved) if saved == self.turns => (),
            _ => {
                self.external_history_unconfirmed = true;
                return Err("Reload the conversation before replying to incoming messages".into());
            }
        }
        self.start_chat_with_semantic(memory, None, Some(message_id));
        let job = self.job.as_ref().map(|job| job.id).ok_or_else(|| {
            "Message saved, but its reply could not start. Check the connection and local memory status.".to_owned()
        })?;
        self.last_external_reply_started = Some((target.uuid, std::time::Instant::now()));
        Ok(job)
    }

    fn start_chat_with_semantic(
        &mut self,
        memory: &mut waifu_studio::memory::MemoryState,
        semantic: Option<&mut waifu_studio::semantic_memory::SemanticMemory>,
        external: Option<Uuid>,
    ) {
        let query = if external.is_some() {
            self.turns
                .last()
                .expect("validated external reply source")
                .content
                .clone()
        } else {
            self.composer.clone()
        };
        if self.external_history_unconfirmed {
            self.notice = Some("Reload the conversation to reconcile the previous external delivery before sending.".into());
            return;
        }
        let send_started = std::time::Instant::now();
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
        if self.job.is_some() || query.trim().is_empty() {
            return;
        }
        let Some(character) = self.library.selected_character() else {
            self.error = Some("Add a character first".into());
            return;
        };
        if query.len() > 32_000 {
            self.error = Some("Messages are limited to 32 KB".into());
            return;
        }
        let config = self.library.providers[self.library.provider].clone();
        if config.model.trim().is_empty() {
            if external.is_none() {
                self.tab = Tab::Connections;
            }
            self.error = Some("Choose a model for this connection first".into());
            return;
        }
        if let Err(error) = config.endpoint("chat/completions") {
            self.error = Some(error);
            return;
        }
        let semantic_scope = semantic.as_ref().map(|state| state.scope());
        let semantic_notes = semantic.and_then(|state| {
            let source = waifu_studio::semantic_memory::MemorySources {
                scope: state.scope(),
                memory,
                facts: &self.facts,
            };
            state.take_context_for(self.library.selected, &source, &query)
        });
        // Semantic retrieval excludes pinned notes. Use only pins alongside it,
        // so the same unpinned note cannot also appear via lexical retrieval.
        let notes = if semantic_notes.is_some()
            && semantic_scope != Some(waifu_studio::semantic_memory::SemanticScope::Evidence)
        {
            memory.context_pins_only()
        } else {
            memory.context(&query)
        };
        let mut persona = format!(
            "{}\n\n{}",
            character.persona,
            character.delegation.prompt_instruction()
        );
        let id = character.id;
        let appraisal_input = (external.is_none()
            && !self.next_loop_turn
            && self.library.scene.affect.requests_appraisal())
        .then(|| {
            (
                character.name.clone(),
                character.persona.clone(),
                waifu_studio::appraisal::bounded(
                    query.trim(),
                    waifu_studio::appraisal::MAX_MESSAGE_BYTES,
                )
                .to_owned(),
            )
        });
        self.cancel_appraisal();
        self.facts.select_character(Some(id));
        let facts_context = if semantic_scope
            != Some(waifu_studio::semantic_memory::SemanticScope::Notes)
            && semantic_notes.is_some()
        {
            None
        } else {
            self.facts.context(&query)
        };
        self.affect.entries.entry(id).or_default().config = self.library.scene.affect.clone();
        self.affect.take_pending(id);
        self.reply_affect_requested = false;
        if let Some(instruction) = self.affect.prompt_for(id) {
            persona.push_str("\n\n");
            persona.push_str(instruction);
            self.reply_affect_requested = true;
            self.reply_affect_config = Some(self.library.scene.affect.clone());
        }
        if external.is_none() {
            // Host-generated loop instructions are not authored user memory.
            if !self.next_loop_turn {
                let turn_id = Uuid::new_v4();
                self.facts
                    .observe_user_turn(id, turn_id, self.composer.trim());
                self.facts.triage_user_turn(
                    id,
                    turn_id,
                    self.composer.trim(),
                    &self.library.decision,
                    &self.library.scene.memory_triage,
                );
            }
            self.turns.push(Turn {
                external: None,
                role: "user".into(),
                content: std::mem::take(&mut self.composer).trim().to_owned(),
            });
        }
        trim_history(&mut self.turns);
        let mut context = vec![Turn {
            external: None,
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
                external: None,
                role: "user".into(),
                content,
            });
        }
        if let Some(content) = semantic_notes {
            context.push(Turn {
                external: None,
                role: "user".into(),
                content,
            });
        }
        if let Some(content) = facts_context {
            context.push(Turn {
                external: None,
                role: "user".into(),
                content,
            });
        }
        if let Some(content) = self.library.scene.routine.chat_context() {
            context.push(Turn {
                external: None,
                role: "user".into(),
                content,
            });
        }
        if external.is_none()
            && let Some(content) = waifu_studio::attachments::document_context(&self.documents)
        {
            context.push(Turn {
                external: None,
                role: "user".into(),
                content,
            });
        }
        context.extend(self.turns[latest..].iter().cloned());
        if external.is_none() {
            self.documents.clear();
        }
        self.pending_response.clear();
        self.reply_filter = Some(Default::default());
        self.error = None;
        self.notice = None;
        self.completed_chat = None;
        self.completed_affect = None;
        let timing = ReplyTiming::new(send_started);
        timing.mark(Stage::Prepared);
        self.reply_timing = Some(timing.clone());
        let native_tools = (external.is_none()
            && !self.next_loop_turn
            && self.library.selected_character().is_some_and(|character| {
                character.delegation.tools.enabled
                    && self
                        .tool_runtime
                        .matches_settings(&character.delegation.tools)
            }))
        .then(|| self.tool_runtime.clone());
        self.job = Some(provider::spawn_chat_with_tools_timed(
            config,
            context,
            if external.is_some() {
                Vec::new()
            } else {
                std::mem::take(&mut self.attachments)
            },
            self.provider_tx.clone(),
            Some(timing.clone()),
            native_tools,
        ));
        if let Some(job_id) = self.job.as_ref().map(|job| job.id) {
            self.loop_chat_job = self.next_loop_turn.then_some(job_id);
            self.begin_reply_speech(job_id);
            if let Some((name, persona, message)) = appraisal_input {
                let decision = self.library.decision.clone();
                match waifu_studio::appraisal::AppraisalJob::start(
                    decision.clone(),
                    &name,
                    &persona,
                    &message,
                ) {
                    Ok(job) => {
                        self.appraisal = Some(PendingAppraisal {
                            turn: job_id,
                            character: id,
                            affect: self.library.scene.affect.clone(),
                            decision,
                            job,
                        });
                        self.appraisal_status = "Appraising incoming message…".into();
                    }
                    Err(error) => self.appraisal_status = error,
                }
            }
        }
        self.chat_character = Some(id);
        let save_started = std::time::Instant::now();
        if let Err(error) = self.store.save_history(id, &self.turns) {
            self.error = Some(error);
        }
        timing.history_save(save_started.elapsed());
        timing.mark(Stage::Returned);
    }
}

pub fn receive_attachment_drops(
    mut events: MessageReader<bevy::window::FileDragAndDrop>,
    mut studio: ResMut<Studio>,
) {
    use bevy::window::FileDragAndDrop;
    for event in events.read() {
        match event {
            FileDragAndDrop::HoveredFile { .. } => studio.attachment_drop_hovered = true,
            FileDragAndDrop::HoveredFileCanceled { .. } => studio.attachment_drop_hovered = false,
            FileDragAndDrop::DroppedFile { path_buf, .. } => {
                studio.attachment_drop_hovered = false;
                studio.mode = ViewMode::Studio;
                studio.click_through = false;
                studio.tab = Tab::Chat;
                if let Err(error) = studio.queue_attachment(path_buf.clone()) {
                    studio.error = Some(error);
                }
            }
        }
    }
}

pub fn poll_studio(
    mut studio: ResMut<Studio>,
    mut loads: MessageWriter<LoadAvatar>,
    mut unloads: MessageWriter<UnloadAvatar>,
    mut animation: ResMut<waifu_studio::animation::AnimationState>,
) {
    let reflection_config = studio.library.scene.automatic_reflection.clone();
    studio.facts.automatic_reflection_frame(&reflection_config);
    studio.facts.poll();
    let prepared = studio.attachment_worker.lock().unwrap().poll();
    if let Some(result) = prepared {
        match result {
            Ok(waifu_studio::attachments::PreparedAttachment::Image(image))
                if studio.attachments.len() < waifu_studio::attachments::MAX_ATTACHMENTS =>
            {
                studio.attachments.push(image)
            }
            Ok(waifu_studio::attachments::PreparedAttachment::Document(document))
                if studio.documents.len() < waifu_studio::attachments::MAX_DOCUMENTS =>
            {
                studio.documents.push(document)
            }
            Ok(_) => {
                studio.error = Some("Attach up to two images and four documents per message".into())
            }
            Err(error) => studio.error = Some(error),
        }
    }
    let files: Vec<_> = studio.file_rx.lock().unwrap().try_iter().collect();
    for event in files {
        match event {
            FileEvent::Models(paths) => studio.import_paths(paths),
            FileEvent::LocalAudio(generation, character, result) => {
                studio.receive_local_preview(generation, character, result);
            }
            FileEvent::Image(path, character) => {
                if character == studio.library.selected
                    && let Err(error) = studio.queue_attachment(path)
                {
                    studio.error = Some(error);
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
    if !studio.attachment_worker.lock().unwrap().is_busy()
        && let Some(path) = studio.pending_attachments.pop_front()
    {
        let result = studio.attachment_worker.lock().unwrap().start(path);
        if let Err(error) = result {
            studio.error = Some(error);
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
            ProviderEvent::ToolProgress(id, role, text)
                if studio.job.as_ref().is_some_and(|j| j.id == id) =>
            {
                if role == "tool_status" {
                    studio.notice = (!text.is_empty()).then_some(text);
                } else if matches!(role.as_str(), "assistant_progress" | "tool_result") {
                    if role == "assistant_progress"
                        && studio.library.voice.enabled
                        && studio
                            .library
                            .selected_character()
                            .is_some_and(|c| c.delegation.speak_intermediate_text)
                    {
                        studio.speak(text.clone());
                    }
                    studio.turns.push(Turn {
                        external: None,
                        role,
                        content: text,
                    });
                    trim_history(&mut studio.turns);
                }
            }
            ProviderEvent::Delta(id, text) if studio.job.as_ref().is_some_and(|j| j.id == id) => {
                if !text.is_empty()
                    && let Some(timing) = &studio.reply_timing
                {
                    timing.mark(Stage::Consumed);
                }
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
                    if studio.reply_affect_requested
                        && studio.library.scene.affect.requests_metadata()
                        && studio.reply_affect_config.as_ref() == Some(&studio.library.scene.affect)
                    {
                        if let (Some(character), Some(metadata)) =
                            (studio.chat_character, filtered.metadata)
                        {
                            match studio.affect.apply_reply_tracked(character, metadata) {
                                Ok(transition) => {
                                    if let Some(transition) = transition {
                                        studio.lifecycle.record(LifecycleEvent::EmotionChanged {
                                            character,
                                            owner: EmotionOwner::Studio,
                                            previous: transition.previous,
                                            emotion: transition.emotion,
                                            reply_id: Some(id),
                                        });
                                    }
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
                studio.reply_affect_config = None;
                if let Some(affect) = studio.appraised_affect.filter(|(turn, character, _)| {
                    *turn == id && Some(*character) == studio.chat_character
                }) {
                    studio.completed_affect = Some(affect);
                }
                let content = std::mem::take(&mut studio.pending_response);
                let character = studio.chat_character;
                studio.lifecycle.record(LifecycleEvent::ReplyFinished {
                    reply_id: id,
                    character,
                    outcome: ReplyOutcome::Completed {
                        text: content.clone(),
                    },
                });
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
                if studio.library.voice.enabled
                    && !streamed_speech
                    && studio.loop_chat_job != Some(id)
                {
                    studio.speak(
                        content
                            .split("```waifu_tasks")
                            .next()
                            .unwrap_or(&content)
                            .to_owned(),
                    );
                    studio.speech_reply_id = Some(id);
                }
                studio.turns.push(Turn {
                    external: None,
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
                let character = studio.chat_character;
                studio.lifecycle.record(LifecycleEvent::ReplyFinished {
                    reply_id: id,
                    character,
                    outcome: ReplyOutcome::Failed {
                        error: error.clone(),
                    },
                });
                studio.completed_affect = None;
                studio.error = Some(error);
                studio.job = None;
                studio.pending_response.clear();
                studio.reply_filter = None;
                studio.reply_affect_requested = false;
                studio.reply_affect_config = None;
                studio.connected = false;
            }
            _ => {}
        }
    }
    studio.poll_appraisal();
    if std::mem::take(&mut studio.unload_model) {
        unloads.write(UnloadAvatar);
    }
    if let Some(path) = studio.requested_model.take() {
        loads.write(LoadAvatar(AssetPath::from_path(&path).into_owned()));
    }
}

/// Retain audio receipts independently of Bevy audio-entity destruction.
pub fn poll_lifecycle(mut studio: ResMut<Studio>) {
    let Studio {
        speech_receipts,
        lifecycle,
        ..
    } = &mut *studio;
    speech_receipts.poll(lifecycle);
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
#[derive(Component)]
pub struct SpeechPlayback {
    character: Option<Uuid>,
    generation: u64,
    playback: waifu_studio::metered_audio::SpeechPlayback,
}
#[derive(Component)]
pub(crate) struct CallSpeechClock {
    clock: waifu_studio::speech_clock::SpeechClock,
    route: waifu_studio::speech_pcm_routing::SpeechRouteReceipt,
}

/// A call-only entity stays owned until the source and remote output settle.
/// Lost routes stop the clock; this system never creates local audio players.
pub(crate) fn poll_call_speech(
    mut commands: Commands,
    mut studio: ResMut<Studio>,
    mut visemes: ResMut<waifu_studio::viseme_audio::VisemeState>,
    clocks: Query<(Entity, &CallSpeechClock, &SpeechPlayback)>,
) {
    for (entity, clock, owner) in &clocks {
        if owner.generation != studio.voice_id || owner.character != studio.speech_character {
            clock.clock.cancel();
            commands.entity(entity).try_despawn();
            continue;
        }
        use waifu_studio::speech_pcm_routing::SpeechRouteOutcome;
        match clock.route.outcome() {
            SpeechRouteOutcome::Active => {}
            SpeechRouteOutcome::Completed => {
                commands.entity(entity).try_despawn();
            }
            SpeechRouteOutcome::Partial => {
                studio.notice = Some("Some call outputs stopped before speech completed.".into());
                commands.entity(entity).try_despawn();
            }
            SpeechRouteOutcome::ProviderFailed => {
                clock.clock.cancel();
                studio.wait_for_provider_failure();
                visemes.cancel();
                commands.entity(entity).try_despawn();
            }
            SpeechRouteOutcome::SourceFailed => {
                clock.clock.cancel();
                studio.fail_current_speech(
                    "Speech playback stopped before the audio completed.".into(),
                );
                visemes.cancel();
                commands.entity(entity).try_despawn();
            }
            SpeechRouteOutcome::Stopped => {
                clock.clock.cancel();
                studio.stop_voice();
                studio.error = Some(
                    "Call output stopped. Connect a ready call output before trying again.".into(),
                );
                visemes.cancel();
                commands.entity(entity).try_despawn();
            }
        }
    }
}

fn spawn_reply_speech(
    commands: &mut Commands,
    studio: &mut Studio,
    audio: &mut Assets<waifu_studio::metered_audio::MeteredSpeech>,
    source: waifu_studio::metered_audio::MeteredSpeech,
    settings: PlaybackSettings,
    routing: &waifu_studio::speech_pcm_routing::SpeechPcmRouting,
) -> bool {
    let Some(output) = studio.active_voice.as_ref().map(|config| config.output) else {
        studio.stop_voice();
        studio.error = Some("Speech output configuration is no longer active.".into());
        return false;
    };
    if output != waifu_studio::voice::VoiceOutput::CallsOnly {
        spawn_speech(commands, studio, audio, source, settings);
        return true;
    }
    let receipt = source.playback();
    let Some(route) = routing.playback_receipt(&receipt) else {
        studio.stop_voice();
        studio.error = Some(
            "No call output is ready. Connect an integration with speech playback allowed.".into(),
        );
        return false;
    };
    match waifu_studio::speech_clock::SpeechClock::start(source, settings.speed) {
        Ok(clock) => {
            studio.speech_receipts.register(
                studio.speech_character,
                studio.voice_id,
                studio.speech_reply_id,
                receipt.clone(),
            );
            commands.spawn((
                SpokenAudio,
                SpeechPlayback {
                    character: studio.speech_character,
                    generation: studio.voice_id,
                    playback: receipt,
                },
                CallSpeechClock { clock, route },
            ));
            true
        }
        Err(_) => {
            studio.stop_voice();
            studio.error = Some("Call audio clock is unavailable. Please try again.".into());
            false
        }
    }
}

fn spawn_speech(
    commands: &mut Commands,
    studio: &mut Studio,
    audio: &mut Assets<waifu_studio::metered_audio::MeteredSpeech>,
    source: waifu_studio::metered_audio::MeteredSpeech,
    settings: PlaybackSettings,
) -> Entity {
    studio.speech_receipts.register(
        studio.speech_character,
        studio.voice_id,
        studio.speech_reply_id,
        source.playback(),
    );
    commands
        .spawn((
            SpokenAudio,
            SpeechPlayback {
                character: studio.speech_character,
                generation: studio.voice_id,
                playback: source.playback(),
            },
            AudioPlayer(audio.add(source)),
            settings,
        ))
        .id()
}

/// Shared buffered-preview path for local/Voice Design previews and the opt-in
/// offscreen recorder. The returned source lets the recorder consume the same
/// decoder without opening speakers; the ordinary app leaves consumption to Bevy.
pub(crate) fn play_preview_audio(
    commands: &mut Commands,
    studio: &mut Studio,
    audio: &mut Assets<waifu_studio::metered_audio::MeteredSpeech>,
    meter: &waifu_studio::metered_audio::SpeechMeter,
    visemes: &mut waifu_studio::viseme_audio::VisemeState,
    playing: impl Iterator<Item = Entity>,
    source: AudioSource,
) -> (Entity, waifu_studio::metered_audio::MeteredSpeech) {
    studio.begin_design_preview();
    meter.stop();
    visemes.cancel();
    visemes.config = speaking_visemes(studio);
    for entity in playing {
        commands.entity(entity).try_despawn();
    }
    // Old sources were removed here; do not stop the new preview next frame.
    studio.stop_audio = false;
    let source = meter.buffered_with_visemes(source, visemes.begin_speech());
    let entity = spawn_speech(
        commands,
        studio,
        audio,
        source.clone(),
        PlaybackSettings::DESPAWN,
    );
    (entity, source)
}
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
    outputs: Res<waifu_studio::extension_speech_output::SpeechOutputs>,
    mut routing: ResMut<waifu_studio::speech_pcm_routing::SpeechPcmRouting>,
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
    if studio.provider_failure_wait.is_none() {
        studio.pump_ws(&mut ws);
    }
    fish.enabled = delivery == Some(waifu_studio::voice::VoiceDelivery::TimestampSse);
    if studio.provider_failure_wait.is_none() {
        studio.pump_sse(&mut fish, !playing.is_empty());
    }
    if studio.provider_failure_wait.is_none()
        && let Some(request) = studio.pending_live.take()
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
        if studio.provider_failure_wait.is_some() {
            if let LiveEvent::Failed { message, .. } = event {
                studio.fail_current_speech(message);
                meter.stop();
                visemes.cancel();
            }
            continue;
        }
        let sentence_reply = studio.pending_sse.is_some();
        if let Some(request) = studio.pending_sse.as_mut()
            && let Err(error) = request.receive(&event)
        {
            fish.cancel();
            studio.stop_voice();
            studio.error = Some(error);
            meter.stop();
            visemes.cancel();
            continue;
        }
        match event {
            LiveEvent::Playing { audio: source, .. } => {
                studio.mark_reply_speech(Stage::AudioQueued);
                let settings = PlaybackSettings::DESPAWN;
                let routes = studio.speech_call_routes(&outputs);
                let source = routing.streaming(
                    &meter,
                    source,
                    Some(visemes.begin_speech()),
                    routes,
                    settings.speed,
                    std::time::Instant::now(),
                );
                if !spawn_reply_speech(
                    &mut commands,
                    &mut studio,
                    &mut audio,
                    source,
                    settings,
                    &routing,
                ) {
                    fish.cancel();
                    ws.cancel();
                    visemes.cancel();
                    return;
                }
            }
            LiveEvent::Finished { .. } => {
                if !sentence_reply {
                    studio.voice_busy = false;
                }
                if studio.pending_ws.take().is_some() {
                    studio.error = Some("Speech stream ended before the reply finished.".into());
                }
            }
            LiveEvent::Failed { message, .. } => {
                studio.fail_current_speech(message);
                meter.stop();
                visemes.cancel();
            }
            LiveEvent::Timing { .. } => {}
        }
    }
    design.poll();
    let local_preview = studio.local_preview.take();
    let preview_is_local = local_preview.is_some();
    if let Some(bytes) = local_preview.or_else(|| design.take_preview()) {
        let source = AudioSource {
            bytes: bytes.into(),
        };
        if rodio::Decoder::builder()
            .with_byte_len(source.bytes.len() as u64)
            .with_data(std::io::Cursor::new(source.clone()))
            .build()
            .is_ok()
        {
            fish.cancel();
            ws.cancel();
            play_preview_audio(
                &mut commands,
                &mut studio,
                &mut audio,
                &meter,
                &mut visemes,
                playing.iter(),
                source,
            );
            speech_target.0 = studio.speech_character;
        } else {
            studio.error = Some(
                if preview_is_local {
                    "Local file is not supported WAV/MP3 audio"
                } else {
                    "Voice Design returned unsupported preview audio"
                }
                .into(),
            );
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
            SpeechRouteContext {
                routing: &mut routing,
                outputs: &outputs,
            },
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
            SpeechRouteContext {
                routing: &mut routing,
                outputs: &outputs,
            },
            event,
        )
    {
        meter.stop();
        visemes.cancel();
    }
    // Give queued provider errors priority over the bounded fallback timeout.
    studio.check_provider_failure_wait(std::time::Instant::now());
}

struct SpeechRouteContext<'a> {
    routing: &'a mut waifu_studio::speech_pcm_routing::SpeechPcmRouting,
    outputs: &'a waifu_studio::extension_speech_output::SpeechOutputs,
}
/// Plays a clip or stream; returns false (and reports) on failure or bad audio.
fn play_voice_event(
    commands: &mut Commands,
    studio: &mut Studio,
    audio: &mut Assets<waifu_studio::metered_audio::MeteredSpeech>,
    meter: &waifu_studio::metered_audio::SpeechMeter,
    visemes: &mut waifu_studio::viseme_audio::VisemeState,
    route: SpeechRouteContext<'_>,
    event: VoiceEvent,
) -> bool {
    match event {
        VoiceEvent::Ready { audio: clip, .. } => {
            play_clip(commands, studio, audio, meter, visemes, route, clip)
        }
        VoiceEvent::Streaming { audio: source, .. } => {
            studio.mark_reply_speech(Stage::AudioQueued);
            let settings = PlaybackSettings {
                speed: studio.voice_playback_speed,
                ..PlaybackSettings::DESPAWN
            };
            let routes = studio.speech_call_routes(route.outputs);
            let source = route.routing.streaming(
                meter,
                source,
                Some(visemes.begin_speech()),
                routes,
                settings.speed,
                std::time::Instant::now(),
            );
            spawn_reply_speech(commands, studio, audio, source, settings, route.routing)
        }
        VoiceEvent::Failed { message, .. } => {
            studio.fail_current_speech(message);
            meter.stop();
            visemes.cancel();
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
    route: SpeechRouteContext<'_>,
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
        studio.fail_current_speech("Speech provider returned invalid or unsupported audio".into());
        meter.stop();
        visemes.cancel();
        return false;
    }
    studio.mark_reply_speech(Stage::AudioQueued);
    let settings = PlaybackSettings {
        speed: studio.voice_playback_speed,
        ..PlaybackSettings::DESPAWN
    };
    let routes = studio.speech_call_routes(route.outputs);
    let source = route.routing.buffered(
        meter,
        source,
        Some(visemes.begin_speech()),
        routes,
        settings.speed,
        std::time::Instant::now(),
    );
    spawn_reply_speech(commands, studio, audio, source, settings, route.routing)
}

/// Bounded local read on the picker worker. NONBLOCK makes FIFO/device selection
/// return to validation rather than holding the sole file-dialog slot forever.
fn read_local_preview(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    const LIMIT: u64 = 32 * 1024 * 1024;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "Could not open local audio")?;
    let meta = file
        .metadata()
        .map_err(|_| "Could not inspect local audio")?;
    if !meta.is_file() || meta.len() > LIMIT {
        return Err("Choose a regular WAV/MP3 file of at most 32 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read local audio")?;
    if bytes.len() as u64 > LIMIT {
        return Err("Local audio exceeds 32 MiB".into());
    }
    Ok(bytes)
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
    mut memory: ResMut<waifu_studio::memory::MemoryState>,
    mut semantic: ResMut<waifu_studio::semantic_memory::SemanticMemory>,
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
    if let Some(transcript) = listening.poll_transcript()
        && apply_transcript(
            &mut studio,
            &mut memory,
            &mut semantic,
            transcript,
            listening.config.auto_send,
        )
    {
        listening.status = "Transcript sent automatically.".into();
    }
}
fn apply_transcript(
    studio: &mut Studio,
    memory: &mut waifu_studio::memory::MemoryState,
    semantic: &mut waifu_studio::semantic_memory::SemanticMemory,
    transcript: waifu_studio::listen::Transcript,
    auto_send_enabled: bool,
) -> bool {
    // A delayed completion must never be attached to a different character.
    if transcript.character.is_some() && transcript.character != studio.library.selected {
        return false;
    }
    if transcript.text.trim().is_empty() {
        return false;
    }
    let send = auto_send_enabled
        && transcript.auto_send
        && transcript.character.is_some()
        && studio.composer.trim().is_empty()
        && !studio.preparing_image()
        && studio.job.is_none();
    if !studio.composer.is_empty() {
        studio.composer.push('\n');
    }
    studio.composer.push_str(&transcript.text);
    studio.notice = Some("Transcript ready. Review it, then send.".into());
    studio.tab = Tab::Chat;
    if send {
        // Uses the normal send path, including memory readiness, model/route
        // validation, semantic freshness and production streamed reply speech.
        studio.send_chat_with_semantic(memory, Some(semantic));
        if studio.chat_busy() {
            studio.notice = Some("Transcript sent automatically.".into());
            return true;
        }
    }
    false
}

pub fn poll_assistant(
    mut semantic: ResMut<waifu_studio::semantic_memory::SemanticMemory>,
    mut board: ResMut<waifu_studio::task_board::TaskBoard>,
    mut studio: ResMut<Studio>,
    mut memory: ResMut<waifu_studio::memory::MemoryState>,
    mut mcp: ResMut<waifu_studio::mcp::McpState>,
    mut agents: ResMut<waifu_studio::agents::AgentState>,
) {
    memory.select_character(studio.library.selected);
    memory.poll();
    let source = waifu_studio::semantic_memory::MemorySources {
        scope: semantic.scope(),
        memory: &memory,
        facts: &studio.facts,
    };
    semantic.sync(studio.library.selected, &source);
    studio.poll_memory_send(&mut memory, &mut semantic);
    board.poll();
    mcp.poll();
    agents.poll();
    let settings = studio
        .library
        .selected_character()
        .map(|character| character.delegation.tools.clone())
        .unwrap_or_default();
    studio.tool_runtime.refresh(&settings, &mcp, &agents);
}

#[cfg(test)]
mod tests {
    include!("studio_external_reply_tests.rs");
    use super::*;

    #[test]
    fn native_file_drop_queues_multiple_files_and_switches_to_conversation() {
        let (mut state, directory) = studio();
        state.mode = ViewMode::Desktop;
        state.tab = Tab::Scene;
        let selected = state.library.selected.unwrap();
        let mut app = App::new();
        app.add_message::<bevy::window::FileDragAndDrop>()
            .insert_resource(state)
            .add_systems(Update, receive_attachment_drops);
        for name in ["picture.PNG", "notes.md", "document.pdf"] {
            app.world_mut()
                .write_message(bevy::window::FileDragAndDrop::DroppedFile {
                    window: Entity::PLACEHOLDER,
                    path_buf: PathBuf::from(name),
                });
        }
        app.update();
        let mut state = app.world_mut().resource_mut::<Studio>();
        assert_eq!(state.mode, ViewMode::Studio);
        assert_eq!(state.tab, Tab::Chat);
        assert_eq!(state.pending_attachments.len(), 3);
        assert!(state.preparing_image());
        state.select_character(selected);
        assert!(state.pending_attachments.is_empty() && state.documents.is_empty());
        drop(state);
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    use waifu_studio::data::Character;
    fn settle_input_fixture(
        studio: &mut Studio,
        memory: &mut waifu_studio::memory::MemoryState,
        semantic: &mut waifu_studio::semantic_memory::SemanticMemory,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            memory.poll();
            studio.facts.poll();
            semantic.sync(studio.library.selected, memory);
            if !memory.busy
                && !studio.facts.busy
                && !studio.facts.pending_saving()
                && semantic.settings_loaded()
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn transcript_auto_send_guards_preserve_drafts_and_discard_wrong_character() {
        use waifu_studio::{
            listen::Transcript, memory::MemoryState, semantic_memory::SemanticMemory,
        };
        for case in [
            "off",
            "not-armed",
            "draft",
            "busy",
            "wrong-character",
            "empty",
            "memory-loading",
            "invalid-model",
        ] {
            let (mut studio, directory) = studio();
            let character = studio.library.selected;
            let mut memory = MemoryState::load(directory.join("notes"));
            let mut semantic = SemanticMemory::load(directory.join("semantic"));
            if case == "invalid-model" {
                studio.library.providers[studio.library.provider]
                    .model
                    .clear();
                memory.select_character(character);
                settle_input_fixture(&mut studio, &mut memory, &mut semantic);
            }
            if case == "draft" {
                studio.composer = "Keep my draft".into();
            }
            if case == "busy" {
                studio.job = Some(ChatJob {
                    id: Uuid::new_v4(),
                    cancel: Arc::new(AtomicBool::new(false)),
                });
            }
            apply_transcript(
                &mut studio,
                &mut memory,
                &mut semantic,
                Transcript {
                    text: if case == "empty" {
                        "  "
                    } else {
                        "Spoken words"
                    }
                    .into(),
                    character: if case == "wrong-character" {
                        Some(Uuid::new_v4())
                    } else {
                        character
                    },
                    auto_send: case != "not-armed",
                },
                case != "off",
            );
            if matches!(case, "wrong-character" | "empty") {
                assert!(studio.composer.is_empty());
            } else if case == "draft" {
                assert_eq!(studio.composer, "Keep my draft\nSpoken words");
            } else {
                assert_eq!(studio.composer, "Spoken words");
            }
            assert_eq!(studio.job.is_some(), case == "busy", "{case}");
            assert!(
                studio.turns.is_empty(),
                "No guarded transcript was sent: {case}"
            );
            // Complete outstanding fixture loads before removing their directory.
            // A review fallback may have just started the normal notes load.
            settle_input_fixture(&mut studio, &mut memory, &mut semantic);
            drop(studio);
            drop(memory);
            drop(semantic);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn transcript_auto_send_uses_production_chat_path_with_memory_ready() {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            net::TcpListener,
        };
        use waifu_studio::{
            listen::Transcript, memory::MemoryState, semantic_memory::SemanticMemory,
        };
        let (mut studio, directory) = studio();
        let selected = studio.library.selected;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        studio.library.providers[studio.library.provider] =
            waifu_studio::provider::ProviderConfig {
                base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
                model: "offline-transcript-model".into(),
                key_env: "WAIFU_STUDIO_OFFLINE_TRANSCRIPT_TEST_UNUSED".into(),
                ..Default::default()
            };
        studio.library.voice.enabled = false;
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
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
            assert!(length < 64000);
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let response = "data: [DONE]\n\n";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
            body
        });
        let mut memory = MemoryState::load(directory.join("notes"));
        let mut semantic = SemanticMemory::load(directory.join("semantic"));
        memory.select_character(selected);
        settle_input_fixture(&mut studio, &mut memory, &mut semantic);
        apply_transcript(
            &mut studio,
            &mut memory,
            &mut semantic,
            Transcript {
                text: "Hello from the microphone".into(),
                character: selected,
                auto_send: true,
            },
            true,
        );
        assert!(studio.job.is_some());
        assert!(studio.composer.is_empty());
        assert_eq!(
            studio.turns.last().unwrap().content,
            "Hello from the microphone"
        );
        let body = server.join().unwrap();
        assert_eq!(
            body["messages"].as_array().unwrap().last().unwrap()["content"],
            "Hello from the microphone"
        );
        studio.cancel_chat();
        drop(studio);
        drop(memory);
        drop(semantic);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn streamed_affect_is_hidden_from_speech_display_and_history() {
        for changed_source in [false, true] {
            let (mut studio, directory) = studio();
            let character = studio.library.selected.unwrap();
            studio.library.voice.enabled = true;
            studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::LiveWebSocket;
            studio.library.scene.affect.enabled = true;
            studio.library.scene.affect.source =
                waifu_studio::reply_affect::AffectSource::ReplyMetadata;
            studio.library.scene.behavior.enabled = true;
            studio.affect.entries.entry(character).or_default().config =
                studio.library.scene.affect.clone();
            studio.reply_affect_requested = true;
            studio.reply_affect_config = Some(studio.library.scene.affect.clone());
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
            if changed_source {
                app.world_mut()
                    .resource_mut::<Studio>()
                    .library
                    .scene
                    .affect
                    .source = waifu_studio::reply_affect::AffectSource::UserMessageJev;
            }

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
                if changed_source {
                    waifu_studio::reply_affect::AffectEmotion::Neutral
                } else {
                    waifu_studio::reply_affect::AffectEmotion::Happy
                }
            );
            assert_eq!(
                studio.affect.entries[&character].mood.valence > 0.0,
                !changed_source
            );
            let controller = &app
                .world()
                .resource::<waifu_studio::behavior::BehaviorState>()
                .controller;
            let weights =
                controller.weights_for(Some(character), &["happy".into()], &Default::default(), "");
            assert_eq!(
                weights.get("happy").is_some_and(|weight| *weight > 0.0),
                !changed_source
            );
            assert!(!studio.affect.entries.contains_key(&Uuid::new_v4()));
            let events = app.world_mut().resource_mut::<Studio>().lifecycle.drain();
            let emotions: Vec<_> = events
                .iter()
                .filter_map(|event| match &event.event {
                    LifecycleEvent::EmotionChanged { .. } => Some(event.event.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                emotions,
                if changed_source {
                    vec![]
                } else {
                    vec![LifecycleEvent::EmotionChanged {
                        character,
                        owner: EmotionOwner::Studio,
                        previous: waifu_studio::reply_affect::AffectEmotion::Neutral,
                        emotion: waifu_studio::reply_affect::AffectEmotion::Happy,
                        reply_id: Some(id),
                    }]
                }
            );
            app.update();
            assert!(
                app.world_mut()
                    .resource_mut::<Studio>()
                    .lifecycle
                    .drain()
                    .is_empty()
            );
            drop(app);
            std::fs::remove_dir_all(directory).unwrap();
        }
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
                socket.set_nonblocking(false).unwrap();
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
        semantic.settings.scope = waifu_studio::semantic_memory::SemanticScope::Notes;
        semantic.settings.base_url = url.clone();
        semantic.settings.model = "offline-embedding".into();
        semantic.save_settings().unwrap();
        wait(|| {
            semantic.sync(character, &memory);
            semantic.ready(character, &memory)
        });
        studio.composer = "Which tea do I like?".into();
        let provider = &mut studio.library.providers[studio.library.provider];
        provider.route = None;
        provider.base_url = url;
        provider.key_env.clear();
        provider.model = "offline-chat".into();
        studio.turns = vec![
            Turn {
                external: None,
                role: "user".into(),
                content: "Earlier question".into(),
            },
            Turn {
                external: None,
                role: "assistant".into(),
                content: "Earlier answer".into(),
            },
        ];
        studio.library.scene.routine = waifu_studio::routines::RoutineConfig {
            enabled: true,
            share_with_chat: true,
            anchors: vec![waifu_studio::routines::RoutineAnchor {
                day: 0,
                minute: 0,
                activity: "Fictional tea tasting".into(),
            }],
        };
        studio.send_chat_with_semantic(&mut memory, Some(&mut semantic));
        assert!(studio.pending_memory_send.is_some() && studio.job.is_none());
        wait(|| {
            semantic.sync(character, &memory);
            studio.poll_memory_send(&mut memory, &mut semantic);
            studio.job.is_some()
        });
        let (stages, history_save, cancelled) =
            studio.reply_timing.as_ref().unwrap().snapshot().unwrap();
        assert!(stages[Stage::Prepared as usize].is_some());
        assert!(stages[Stage::Returned as usize] >= stages[Stage::Prepared as usize]);
        assert!(history_save.is_some());
        assert!(!cancelled);
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
        if waifu_studio::routines::local_clock().is_some() {
            let routine = &messages[messages.len() - 2];
            assert_eq!(routine["role"], "user");
            assert!(
                routine["content"]
                    .as_str()
                    .unwrap()
                    .contains("Fictional tea tasting")
            );
            assert!(
                studio
                    .turns
                    .iter()
                    .all(|turn| !turn.content.contains("Fictional tea tasting"))
            );
        }
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
    #[test]
    fn focused_appraisal_applies_once_and_rejects_stale_snapshots() {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            net::TcpListener,
            time::{Duration, Instant},
        };
        use waifu_studio::{
            appraisal::AppraisalJob,
            reply_affect::{AffectEmotion, AffectSource},
        };
        let (mut studio, directory) = studio();
        let character = studio.library.selected.unwrap();
        studio.library.scene.affect.enabled = true;
        studio.library.scene.affect.source = AffectSource::UserMessageJev;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        studio.library.decision.endpoint =
            format!("http://{}/evaluation", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((key, value)) = line.split_once(':')
                    && key.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            let body = serde_json::json!({"answers":{
                "emotion":{"type":"choice","choice":"curious"},
                "valence":{"type":"choice","choice":"positive"},
                "arousal":{"type":"choice","choice":"neutral"},
                "dominance":{"type":"choice","choice":"neutral"}
            }})
            .to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let turn = Uuid::new_v4();
        studio.completed_chat = Some((turn, Ok("Reply".into())));
        let config = studio.library.scene.affect.clone();
        let decision = studio.library.decision.clone();
        let job = AppraisalJob::start(decision.clone(), "A", "persona", "Hello").unwrap();
        studio.appraisal = Some(PendingAppraisal {
            turn,
            character,
            affect: config.clone(),
            decision: decision.clone(),
            job,
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while studio.appraisal.is_some() {
            studio.poll_appraisal();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        server.join().unwrap();
        assert_eq!(
            studio.affect.entries[&character].emotion,
            AffectEmotion::Curious
        );
        assert!(studio.affect.prompt_for(character).is_none());
        assert_eq!(
            studio.appraised_affect,
            Some((turn, character, AffectEmotion::Curious))
        );
        assert_eq!(
            studio
                .lifecycle
                .drain()
                .into_iter()
                .map(|event| event.event)
                .collect::<Vec<_>>(),
            vec![LifecycleEvent::EmotionChanged {
                character,
                owner: EmotionOwner::Studio,
                previous: AffectEmotion::Neutral,
                emotion: AffectEmotion::Curious,
                reply_id: Some(turn),
            }]
        );
        let mood = studio.affect.entries[&character].mood;
        studio.poll_appraisal();
        assert_eq!(studio.affect.entries[&character].mood, mood);
        assert!(studio.lifecycle.drain().is_empty());
        for change in 0..3 {
            let job = AppraisalJob::start(
                waifu_studio::recipient_decision::DecisionConfig {
                    endpoint: String::new(),
                    ..Default::default()
                },
                "A",
                "persona",
                "Hello",
            )
            .unwrap();
            studio.appraisal = Some(PendingAppraisal {
                turn,
                character,
                affect: config.clone(),
                decision: decision.clone(),
                job,
            });
            match change {
                0 => studio.library.scene.affect.source = AffectSource::ReplyMetadata,
                1 => studio.completed_chat = Some((Uuid::new_v4(), Ok("New reply".into()))),
                _ => studio.library.decision.model = "changed-model".into(),
            }
            studio.poll_appraisal();
            assert!(studio.appraisal.is_none());
            assert!(studio.appraisal_status.contains("Stale"));
            assert_eq!(studio.affect.entries[&character].mood, mood);
            assert!(studio.lifecycle.drain().is_empty());
            studio.library.scene.affect = config.clone();
            studio.library.decision = decision.clone();
            studio.completed_chat = Some((turn, Ok("Reply".into())));
            // Give the intentionally invalid worker a chance to release its slot.
            std::thread::sleep(Duration::from_millis(10));
        }
        studio.cancel_chat();
        assert!(studio.appraised_affect.is_none());
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_preview_read_is_bounded_and_rejects_nonregular_inputs() {
        let root = std::env::temp_dir().join(format!("local-preview-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("clip.wav");
        std::fs::write(&file, [1, 2, 3]).unwrap();
        assert_eq!(read_local_preview(&file).unwrap(), vec![1, 2, 3]);
        assert!(read_local_preview(&root).is_err());
        assert!(read_local_preview(&root.join("missing")).is_err());
        std::fs::File::create(&file)
            .unwrap()
            .set_len(32 * 1024 * 1024 + 1)
            .unwrap();
        assert!(read_local_preview(&file).unwrap_err().contains("32 MiB"));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let fifo = root.join("pipe.wav");
            let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: path is a valid nul-terminated pathname in our temporary directory.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            assert!(read_local_preview(&fifo).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_preview_respects_stop_generation_and_character_without_changing_voice() {
        let (mut studio, directory) = studio();
        let selected = studio.library.selected;
        let voice_before = serde_json::to_value(&studio.library.voice).unwrap();
        let generation = studio.voice_id;
        studio.receive_local_preview(generation, selected, Ok(vec![1, 2, 3]));
        assert_eq!(studio.local_preview.as_deref(), Some([1, 2, 3].as_slice()));
        studio.stop_voice();
        assert!(studio.local_preview.is_none());
        studio.receive_local_preview(generation, selected, Ok(vec![4]));
        studio.receive_local_preview(generation, selected, Err("stale error".into()));
        assert!(studio.local_preview.is_none());
        assert!(studio.error.is_none());
        studio.receive_local_preview(studio.voice_id, Some(Uuid::new_v4()), Ok(vec![5]));
        assert!(studio.local_preview.is_none());
        studio.receive_local_preview(studio.voice_id, selected, Err("read failed".into()));
        assert_eq!(studio.error.as_deref(), Some("read failed"));
        assert_eq!(
            serde_json::to_value(&studio.library.voice).unwrap(),
            voice_before
        );
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selection_source_tracks_changes_import_clear_and_keeps_same_selection_reload() {
        let (mut studio, directory) = studio();
        let a = studio.library.selected.unwrap();
        let mut second = studio.library.characters[0].clone();
        let b = Uuid::new_v4();
        second.id = b;
        studio.library.characters.push(second);
        studio.lifecycle.drain();
        let voice_generation = studio.voice_id;
        studio.select_character(a);
        assert_ne!(
            studio.voice_id, voice_generation,
            "same selection must still cancel/reload"
        );
        assert!(studio.lifecycle.drain().is_empty());
        let reply_id = Uuid::new_v4();
        studio.job = Some(ChatJob {
            id: reply_id,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        studio.select_character(b);
        studio.select_character(a);
        let events = studio.lifecycle.drain();
        assert!(matches!(events[0].event, LifecycleEvent::ReplyFinished {
            reply_id: id, character: Some(owner), outcome: ReplyOutcome::Cancelled,
        } if id == reply_id && owner == a));
        assert_eq!(
            events[1].event,
            LifecycleEvent::CharacterSelected {
                previous: Some(a),
                new: Some(b),
                selection_revision: 1,
            }
        );
        assert_eq!(
            events[2].event,
            LifecycleEvent::CharacterSelected {
                previous: Some(b),
                new: Some(a),
                selection_revision: 2,
            }
        );
        // Loading has not completed: selection events do not wait for a VRM.
        assert!(studio.requested_model.is_some());
        let model = directory.join("selection.gltf");
        std::fs::write(
            &model,
            r#"{"asset":{"version":"2.0"},"nodes":[],"scenes":[{"nodes":[]}],"scene":0}"#,
        )
        .unwrap();
        studio.import_paths(vec![model.clone()]);
        let imported = studio.library.selected.unwrap();
        let events = studio.lifecycle.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event,
            LifecycleEvent::CharacterSelected {
                previous: Some(a),
                new: Some(imported),
                selection_revision: 3,
            }
        );
        studio.import_paths(vec![model]);
        studio.import_paths(vec![directory.join("missing.gltf")]);
        assert!(studio.lifecycle.drain().is_empty());
        studio
            .library
            .characters
            .retain(|character| character.id == imported);
        studio.remove_character();
        assert_eq!(
            studio.lifecycle.drain()[0].event,
            LifecycleEvent::CharacterSelected {
                previous: Some(imported),
                new: None,
                selection_revision: 4,
            }
        );
        drop(studio);
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

    fn external_request(studio: &Studio) -> (Target, IntegrationOrigin) {
        (
            Target {
                uuid: studio.library.selected.unwrap(),
                generation: 1,
            },
            IntegrationOrigin {
                integration_id: "org.example.external".into(),
                display_name: "External fixture".into(),
            },
        )
    }

    #[test]
    fn external_first_person_claim_is_saved_without_memory_intake_triage_or_generation() {
        let (mut studio, directory) = studio();
        let (target, origin) = external_request(&studio);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !studio.facts.loaded || studio.facts.busy {
            studio.facts.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        studio.facts.enabled = true;
        studio.facts.candidates_enabled = true;
        studio.library.scene.memory_triage.enabled = true;
        studio.facts.triage_status = "idle before delivery".into();
        studio.composer = "Keep my local draft".into();
        let generation = studio.facts.generation();
        let text = "I like jasmine tea. Remember that I live in a lighthouse.";
        let id = studio.deliver_external(target, &origin, text).unwrap();
        assert!(!id.is_nil());
        assert_eq!(studio.store.history(target.uuid).unwrap(), studio.turns);
        assert_eq!(studio.turns.len(), 1);
        assert_eq!(studio.turns[0].role, "external");
        assert_eq!(studio.turns[0].content, text);
        assert_eq!(
            studio.turns[0].external.as_ref().unwrap(),
            &ExternalMessage {
                message_id: id,
                origin
            }
        );
        studio.facts.poll();
        assert_eq!(studio.facts.generation(), generation);
        assert!(studio.facts.candidates().is_empty());
        assert!(studio.facts.records().is_empty());
        assert_eq!(studio.facts.triage_status, "idle before delivery");
        assert_eq!(studio.composer, "Keep my local draft");
        assert!(studio.job.is_none() && studio.appraisal.is_none());
        assert!(studio.pending_response.is_empty() && studio.lifecycle.drain().is_empty());
        assert!(studio.provider_rx.lock().unwrap().try_recv().is_err());
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unconfirmed_external_save_blocks_normal_intake_until_verified_history_reload() {
        let (mut studio, directory) = studio();
        let (target, origin) = external_request(&studio);
        let before = studio.turns.clone();
        let original_root = studio.store.root.clone();
        // The preflight sees no history; the write fails because its parent is missing.
        studio.store.root = directory.join("missing-parent");
        assert_eq!(
            studio
                .deliver_external(target, &origin, "External context")
                .unwrap_err()
                .0,
            ErrorCode::Unavailable
        );
        assert_eq!(studio.turns, before);
        assert!(studio.external_history_unconfirmed);
        studio.store.root = original_root;
        // Represent the ambiguous post-rename state: disk committed, UI did not.
        let committed = Turn::external(
            "Already committed context",
            ExternalMessage {
                message_id: Uuid::new_v4(),
                origin: origin.clone(),
            },
        )
        .unwrap();
        studio
            .store
            .save_history(target.uuid, std::slice::from_ref(&committed))
            .unwrap();
        studio.composer = "I like local tea".into();
        let mut memory = waifu_studio::memory::MemoryState::load(directory.join("notes"));
        let facts_generation = studio.facts.generation();
        studio.send_chat(&mut memory);
        assert_eq!(studio.composer, "I like local tea");
        assert_eq!(studio.turns, before);
        assert!(studio.job.is_none() && studio.appraisal.is_none());
        assert_eq!(memory.selected_character(), None);
        assert_eq!(studio.facts.generation(), facts_generation);
        assert!(studio.facts.candidates().is_empty());
        assert_eq!(
            studio.store.history(target.uuid).unwrap(),
            vec![committed.clone()]
        );
        assert_eq!(
            studio
                .deliver_external(target, &origin, "Retry")
                .unwrap_err()
                .0,
            ErrorCode::Unavailable
        );
        // A failed explicit reload must not lift quarantine.
        let path = directory
            .join("conversations")
            .join(format!("{}.json", target.uuid));
        std::fs::write(&path, "invalid history").unwrap();
        studio.select_character(target.uuid);
        assert!(studio.external_history_unconfirmed);
        studio
            .store
            .save_history(target.uuid, std::slice::from_ref(&committed))
            .unwrap();
        studio.select_character(target.uuid);
        assert!(!studio.external_history_unconfirmed);
        assert_eq!(studio.turns, vec![committed.clone()]);
        studio
            .deliver_external(target, &origin, "Next external context")
            .unwrap();
        assert_eq!(studio.turns[0], committed);
        assert_eq!(studio.turns.len(), 2);
        assert_eq!(studio.store.history(target.uuid).unwrap(), studio.turns);
        drop(memory);
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_preflight_rejects_disk_ui_mismatch_and_busy_or_wrong_character() {
        let (mut studio, directory) = studio();
        let (target, origin) = external_request(&studio);
        studio.job = Some(ChatJob {
            id: Uuid::new_v4(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(
            studio
                .deliver_external(target, &origin, "Context")
                .unwrap_err()
                .0,
            ErrorCode::Busy
        );
        studio.job = None;
        let wrong = Target {
            uuid: Uuid::new_v4(),
            ..target
        };
        assert_eq!(
            studio
                .deliver_external(wrong, &origin, "Context")
                .unwrap_err()
                .0,
            ErrorCode::StaleTarget
        );
        assert!(studio.turns.is_empty() && !studio.external_history_unconfirmed);
        let committed = Turn::external(
            "Disk-only context",
            ExternalMessage {
                message_id: Uuid::new_v4(),
                origin: origin.clone(),
            },
        )
        .unwrap();
        studio
            .store
            .save_history(target.uuid, std::slice::from_ref(&committed))
            .unwrap();
        assert_eq!(
            studio
                .deliver_external(target, &origin, "New context")
                .unwrap_err()
                .0,
            ErrorCode::Unavailable
        );
        assert!(studio.turns.is_empty() && studio.external_history_unconfirmed);
        assert_eq!(studio.store.history(target.uuid).unwrap(), vec![committed]);
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reply_journal_preserves_completions_beyond_latest_result_and_selection() {
        let (mut studio, directory) = studio();
        studio.library.voice.enabled = false;
        let character = studio.chat_character;
        let sender = studio.provider_tx.clone();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(studio)
            .init_resource::<waifu_studio::animation::AnimationState>()
            .add_message::<LoadAvatar>()
            .add_message::<UnloadAvatar>()
            .add_systems(Update, poll_studio);
        let mut replies = Vec::new();
        for text in ["First complete reply", "Second complete reply"] {
            let id = Uuid::new_v4();
            replies.push(id);
            let mut studio = app.world_mut().resource_mut::<Studio>();
            studio.job = Some(ChatJob {
                id,
                cancel: Arc::new(AtomicBool::new(false)),
            });
            studio.pending_response = text.into();
            // Source ownership comes from the job, not mutable editor selection.
            studio.library.selected = Some(Uuid::new_v4());
            sender.send(ProviderEvent::Finished(id)).unwrap();
            app.update();
        }
        let mut studio = app.world_mut().resource_mut::<Studio>();
        assert_eq!(studio.completed_chat.as_ref().unwrap().0, replies[1]);
        let events = studio.lifecycle.drain();
        assert_eq!(events.len(), 2);
        for (event, reply) in events.into_iter().zip(replies) {
            assert!(
                matches!(event.event, LifecycleEvent::ReplyFinished { reply_id, character: owner, outcome: ReplyOutcome::Completed { .. } } if reply_id == reply && owner == character)
            );
        }
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cancellation_records_original_reply_owner_once_before_switch() {
        let (mut studio, directory) = studio();
        let owner = studio.chat_character;
        let id = Uuid::new_v4();
        studio.job = Some(ChatJob {
            id,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        studio.cancel_chat();
        studio.chat_character = Some(Uuid::new_v4());
        studio.cancel_chat();
        let events = studio.lifecycle.drain();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].event, LifecycleEvent::ReplyFinished { reply_id, character, outcome: ReplyOutcome::Cancelled } if reply_id == id && character == owner)
        );
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn calls_only_wave(samples: usize) -> AudioSource {
        let size = (samples * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + size).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8000u32.to_le_bytes());
        bytes.extend(16000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(size.to_le_bytes());
        bytes.resize(bytes.len() + size as usize, 0);
        AudioSource {
            bytes: bytes.into(),
        }
    }
    #[test]
    fn calls_only_missing_route_has_no_local_fallback_or_decoder_claim() {
        use bevy::ecs::world::CommandQueue;
        let (mut studio, directory) = studio();
        studio.active_voice = Some(waifu_studio::voice::VoiceConfig {
            output: waifu_studio::voice::VoiceOutput::CallsOnly,
            ..Default::default()
        });
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let source = meter.buffered(calls_only_wave(800));
        let receipt = source.playback();
        let mut world = World::new();
        let mut commands = CommandQueue::default();
        let mut audio = Assets::default();
        assert!(!spawn_reply_speech(
            &mut Commands::new(&mut commands, &world),
            &mut studio,
            &mut audio,
            source,
            PlaybackSettings::DESPAWN,
            &waifu_studio::speech_pcm_routing::SpeechPcmRouting::default()
        ));
        commands.apply(&mut world);
        assert_eq!(world.query::<&SpokenAudio>().iter(&world).count(), 0);
        assert_eq!(
            world
                .query::<&AudioPlayer<waifu_studio::metered_audio::MeteredSpeech>>()
                .iter(&world)
                .count(),
            0
        );
        assert!(audio.is_empty());
        assert!(!receipt.has_started());
        assert!(
            studio
                .error
                .as_deref()
                .unwrap()
                .contains("No call output is ready")
        );
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn uncaptured_reply_output_never_falls_back_to_local_player() {
        use bevy::ecs::world::CommandQueue;
        let (mut studio, directory) = studio();
        studio.library.voice.output = waifu_studio::voice::VoiceOutput::CallsOnly;
        assert!(studio.active_voice.is_none());
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let source = meter.buffered(calls_only_wave(800));
        let mut world = World::new();
        let mut commands = CommandQueue::default();
        let mut audio = Assets::default();
        assert!(!spawn_reply_speech(
            &mut Commands::new(&mut commands, &world),
            &mut studio,
            &mut audio,
            source,
            PlaybackSettings::DESPAWN,
            &waifu_studio::speech_pcm_routing::SpeechPcmRouting::default()
        ));
        commands.apply(&mut world);
        assert_eq!(world.query::<&SpokenAudio>().iter(&world).count(), 0);
        assert!(audio.is_empty());
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn failed_voice_event_preserves_provider_error_and_other_queued_replies() {
        use bevy::ecs::world::CommandQueue;
        let (mut studio, directory) = studio();
        let config = waifu_studio::voice::VoiceConfig {
            enabled: true,
            output: waifu_studio::voice::VoiceOutput::CallsOnly,
            ..Default::default()
        };
        studio.speech_character = studio.library.selected;
        studio.active_voice = Some(config.clone());
        studio
            .queue_speech(
                studio.library.selected.unwrap(),
                config,
                "Next reply".into(),
            )
            .unwrap();
        let generation = studio.voice_id;
        studio.wait_for_provider_failure();
        assert_eq!(studio.voice_id, generation);
        assert!(studio.voice_busy);
        studio.advance_speech_queue(false);
        assert_eq!(studio.voice_id, generation);
        assert_eq!(studio.speech_queue.len(), 1);
        assert!(
            studio
                .receive_voice_event(VoiceEvent::Failed {
                    id: generation,
                    message: "Early provider failure details".into(),
                })
                .is_none()
        );
        assert_eq!(studio.voice_id, generation.wrapping_add(1));
        assert_eq!(
            studio.error.as_deref(),
            Some("Early provider failure details")
        );
        assert!(studio.provider_failure_wait.is_none());
        let generation = studio.voice_id;
        let world = World::new();
        let mut commands = CommandQueue::default();
        let mut audio = Assets::default();
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let mut visemes = waifu_studio::viseme_audio::VisemeState::default();
        let mut routing = waifu_studio::speech_pcm_routing::SpeechPcmRouting::default();
        let outputs = waifu_studio::extension_speech_output::SpeechOutputs::default();
        assert!(!play_voice_event(
            &mut Commands::new(&mut commands, &world),
            &mut studio,
            &mut audio,
            &meter,
            &mut visemes,
            SpeechRouteContext {
                routing: &mut routing,
                outputs: &outputs
            },
            VoiceEvent::Failed {
                id: generation,
                message: "Fixture speech provider failed".into()
            }
        ));
        assert_eq!(studio.voice_id, generation.wrapping_add(1));
        assert_eq!(
            studio.error.as_deref(),
            Some("Fixture speech provider failed")
        );
        assert_eq!(studio.speech_queue.len(), 1);
        assert!(studio.provider_failure_wait.is_none());
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn provider_failure_wait_is_bounded_and_cannot_accept_new_audio() {
        let (mut studio, directory) = studio();
        let generation = studio.voice_id;
        studio.wait_for_provider_failure();
        let event = VoiceEvent::Ready {
            id: generation,
            audio: waifu_studio::voice::VoiceAudio {
                bytes: calls_only_wave(80).bytes.to_vec(),
                mime: "audio/wav".into(),
                format: "wav".into(),
            },
        };
        assert!(studio.receive_voice_event(event).is_none());
        assert!(studio.voice_busy);
        let started = studio.provider_failure_wait.unwrap().1;
        studio.check_provider_failure_wait(started + std::time::Duration::from_secs(5));
        assert_eq!(studio.voice_id, generation.wrapping_add(1));
        assert!(studio.provider_failure_wait.is_none());
        assert!(
            studio
                .error
                .as_deref()
                .unwrap()
                .contains("Speech provider stopped")
        );
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn calls_only_eof_waits_for_settlement_and_late_route_loss_is_reported() {
        use waifu_studio::{
            extension_runtime::{AudioPlaybackRoute, ExtensionRuntime},
            extension_speech_output::SpeechOutputs,
            speech_pcm_routing::SpeechPcmRouting,
        };
        let (mut studio, directory) = studio();
        studio.speech_character = studio.library.selected;
        studio.active_voice = Some(waifu_studio::voice::VoiceConfig {
            output: waifu_studio::voice::VoiceOutput::CallsOnly,
            ..Default::default()
        });
        studio.stop_audio = false;
        let target = Target {
            uuid: studio.speech_character.unwrap(),
            generation: 0,
        };
        let route = AudioPlaybackRoute {
            installation_id: Uuid::new_v4(),
            session: Uuid::new_v4(),
            process_generation: 1,
            target,
            origin: IntegrationOrigin {
                integration_id: "test.output".into(),
                display_name: "Fixture".into(),
            },
        };
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let mut routing = SpeechPcmRouting::default();
        let source = routing.buffered(
            &meter,
            calls_only_wave(80),
            None,
            vec![route],
            1.0,
            std::time::Instant::now(),
        );
        let receipt = source.playback();
        let route = routing.playback_receipt(&receipt).unwrap();
        let clock = waifu_studio::speech_clock::SpeechClock::start(source, 1.0).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !receipt.reached_eof() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let mut world = World::new();
        world.spawn((
            SpokenAudio,
            CallSpeechClock { clock, route },
            SpeechPlayback {
                character: studio.speech_character,
                generation: studio.voice_id,
                playback: receipt,
            },
        ));
        world.insert_resource(studio);
        world.insert_resource(waifu_studio::viseme_audio::VisemeState::default());
        let mut schedule = Schedule::default();
        schedule.add_systems(poll_call_speech);
        schedule.run(&mut world);
        assert_eq!(world.query::<&CallSpeechClock>().iter(&world).count(), 1);
        assert!(world.resource::<Studio>().error.is_none());
        // The synthetic route is unavailable to the real authority resolver.
        routing.tick(
            &mut SpeechOutputs::default(),
            &ExtensionRuntime::default(),
            Some(target),
            std::time::Instant::now(),
        );
        schedule.run(&mut world);
        assert_eq!(world.query::<&CallSpeechClock>().iter(&world).count(), 0);
        assert!(
            world
                .resource::<Studio>()
                .error
                .as_deref()
                .unwrap()
                .contains("Call output stopped")
        );
        drop(world);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn calls_only_constructs_no_audio_player_and_lost_route_stops_clock() {
        use bevy::ecs::world::CommandQueue;
        use waifu_studio::{
            extension_runtime::{AudioPlaybackRoute, ExtensionRuntime},
            extension_speech_output::SpeechOutputs,
            speech_pcm_routing::SpeechPcmRouting,
        };
        let (mut studio, directory) = studio();
        studio.speech_character = studio.library.selected;
        studio.active_voice = Some(waifu_studio::voice::VoiceConfig {
            output: waifu_studio::voice::VoiceOutput::CallsOnly,
            ..Default::default()
        });
        let target = Target {
            uuid: studio.speech_character.unwrap(),
            generation: 0,
        };
        // Synthetic captured route exercises the app constructor. The separate
        // real-child clock integration covers actual ready authority/media.
        let route = AudioPlaybackRoute {
            installation_id: Uuid::new_v4(),
            session: Uuid::new_v4(),
            process_generation: 1,
            target,
            origin: IntegrationOrigin {
                integration_id: "test.output".into(),
                display_name: "Fixture".into(),
            },
        };
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let mut routing = SpeechPcmRouting::default();
        let source = routing.buffered(
            &meter,
            calls_only_wave(80_000),
            None,
            vec![route],
            1.0,
            std::time::Instant::now(),
        );
        let mut world = World::new();
        let mut commands = CommandQueue::default();
        let mut audio = Assets::default();
        assert!(spawn_reply_speech(
            &mut Commands::new(&mut commands, &world),
            &mut studio,
            &mut audio,
            source,
            PlaybackSettings::DESPAWN,
            &routing
        ));
        commands.apply(&mut world);
        assert_eq!(world.query::<&CallSpeechClock>().iter(&world).count(), 1);
        assert_eq!(
            world
                .query::<&AudioPlayer<waifu_studio::metered_audio::MeteredSpeech>>()
                .iter(&world)
                .count(),
            0
        );
        assert!(audio.is_empty());
        routing.tick(
            &mut SpeechOutputs::default(),
            &ExtensionRuntime::default(),
            Some(target),
            std::time::Instant::now(),
        );
        world.insert_resource(studio);
        world.insert_resource(routing);
        world.insert_resource(waifu_studio::viseme_audio::VisemeState::default());
        let mut schedule = Schedule::default();
        schedule.add_systems(poll_call_speech);
        schedule.run(&mut world);
        assert_eq!(world.query::<&CallSpeechClock>().iter(&world).count(), 0);
        let studio = world.resource::<Studio>();
        assert!(studio.stop_audio);
        assert!(
            studio
                .error
                .as_deref()
                .unwrap()
                .contains("Call output stopped")
        );
        drop(world);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn spawned_short_speech_receipt_survives_entity_removal() {
        use bevy::{audio::Decodable, ecs::world::CommandQueue};
        let (mut studio, directory) = studio();
        studio.speech_character = studio.library.selected;
        let reply_id = Uuid::new_v4();
        studio.speech_reply_id = Some(reply_id);
        let owner = studio.speech_character;
        let mut wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x04\0\0\0".to_vec();
        wav.extend([0u8; 4]);
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let source = meter.buffered(AudioSource { bytes: wav.into() });
        let mut decoder = source.decoder();
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let mut audio = Assets::default();
        let entity = spawn_speech(
            &mut Commands::new(&mut queue, &world),
            &mut studio,
            &mut audio,
            source,
            PlaybackSettings::DESPAWN,
        );
        queue.apply(&mut world);
        while decoder.next().is_some() {}
        world.despawn(entity);
        studio.speech_character = Some(Uuid::new_v4());
        let Studio {
            speech_receipts,
            lifecycle,
            ..
        } = &mut studio;
        speech_receipts.poll(lifecycle);
        let events = lifecycle.drain();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(events[0].event, LifecycleEvent::SpeechStarted { character, reply_id: Some(id), .. } if character == owner && id == reply_id)
        );
        assert!(
            matches!(events[1].event, LifecycleEvent::SpeechStopped { character, reply_id: Some(id), .. } if character == owner && id == reply_id)
        );
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn spawned_speech_tracks_audio_consumption_and_immutable_owner_and_generation() {
        use bevy::{audio::Decodable, ecs::world::CommandQueue};
        let (mut studio, directory) = studio();
        studio.speech_character = studio.library.selected;
        let mut wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x04\0\0\0".to_vec();
        wav.extend([0u8; 4]);
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let source = meter.buffered(AudioSource { bytes: wav.into() });
        let mut decoder = source.decoder();
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let mut audio = Assets::default();
        spawn_speech(
            &mut Commands::new(&mut queue, &world),
            &mut studio,
            &mut audio,
            source,
            PlaybackSettings::DESPAWN,
        );
        queue.apply(&mut world);
        let source = world.query::<&SpeechPlayback>().single(&world).unwrap();
        assert!(!studio.speech_is_playing(source));
        assert_eq!(decoder.next(), Some(0.0));
        assert!(studio.speech_is_playing(source));
        studio.speech_character = Some(Uuid::new_v4());
        assert!(
            !studio.speech_is_playing(source),
            "old character still exists"
        );
        studio.speech_character = studio.library.selected;
        studio.voice_id = studio.voice_id.wrapping_add(1);
        assert!(
            !studio.speech_is_playing(source),
            "same character, new speech"
        );
        studio.voice_id = studio.voice_id.wrapping_sub(1);
        assert!(studio.speech_is_playing(source));
        meter.stop();
        assert!(
            !studio.speech_is_playing(source),
            "stop precedes entity removal"
        );
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn buffered_preview_replaces_old_audio_without_cancelling_itself_next_frame() {
        use bevy::{audio::Decodable, ecs::world::CommandQueue};
        let (mut studio, directory) = studio();
        let source = || {
            let mut wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x04\0\0\0".to_vec();
            wav.extend([0u8; 4]);
            AudioSource { bytes: wav.into() }
        };
        let meter = waifu_studio::metered_audio::SpeechMeter::default();
        let old = meter.buffered(source());
        let mut old_decoder = old.decoder();
        old_decoder.next();
        let mut world = World::new();
        let old_entity = world.spawn(SpokenAudio).id();
        let mut queue = CommandQueue::default();
        let mut audio = Assets::default();
        let mut visemes = waifu_studio::viseme_audio::VisemeState::default();
        let old_generation = studio.voice_id;
        let (_, preview) = play_preview_audio(
            &mut Commands::new(&mut queue, &world),
            &mut studio,
            &mut audio,
            &meter,
            &mut visemes,
            std::iter::once(old_entity),
            source(),
        );
        assert_ne!(studio.voice_id, old_generation);
        assert!(
            !studio.stop_audio,
            "the new preview must survive poll_voice"
        );
        assert_eq!(old_decoder.next(), None);
        queue.apply(&mut world);
        assert!(world.get_entity(old_entity).is_err());
        let receipt = world.query::<&SpeechPlayback>().single(&world).unwrap();
        assert!(!studio.speech_is_playing(receipt));
        let mut decoder = preview.decoder();
        decoder.next();
        assert!(studio.speech_is_playing(receipt));
        drop(decoder);
        assert!(!studio.speech_is_playing(receipt));
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
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
        // Hosted speech without any key is skipped quietly; give this session one.
        studio.voice_key = "session-secret".into();
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
    fn websocket_connects_before_reply_text_and_cancel_discards_it() {
        use std::{net::TcpListener, sync::mpsc, time::Duration};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let (connected_tx, connected_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut socket = tungstenite::accept(socket).unwrap();
                        // Only a start event may be sent before the first text.
                        let start = socket.read().unwrap();
                        assert!(start.is_binary());
                        connected_tx.send(()).unwrap();
                        let _ = socket.read(); // cancelled connection closes
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "No early WebSocket connection"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        let (mut studio, directory) = studio();
        studio.library.voice.enabled = true;
        studio.library.voice.endpoint = endpoint;
        studio.library.voice.direction.clear();
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::LiveWebSocket;
        studio.voice_key = "test-session-key".into();
        studio.begin_reply_speech(Uuid::new_v4());
        assert!(studio.pending_ws.as_ref().unwrap().text.is_empty());
        let mut ws = waifu_studio::fish_ws::FishWsState::default();
        ws.enabled = true;
        studio.pump_ws(&mut ws);
        connected_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(studio.pending_ws.as_ref().unwrap().opened);
        studio.stop_voice();
        ws.cancel();
        server.join().unwrap();
        assert!(studio.pending_ws.is_none());
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
        mock_tts_transport(false)
    }
    fn mock_tts_transport(sse: bool) -> (String, mpsc::Receiver<String>) {
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
                let (mime, body) = if sse {
                    use base64::Engine;
                    let payload = serde_json::json!({
                        "audio_base64": base64::engine::general_purpose::STANDARD.encode(&wav),
                        "content": "fixture", "alignment": null,
                        "chunk_seq": 0, "chunk_audio_offset_sec": 0.0,
                    });
                    (
                        "text/event-stream",
                        format!("event: message\ndata: {payload}\n\n").into_bytes(),
                    )
                } else {
                    ("audio/wav", wav)
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
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
    fn sse_reply_requests_sentences_early_and_waits_for_playback_then_stop_discards_tail() {
        use waifu_studio::{
            fish_live::{FishLiveState, LiveEvent},
            voice::{VoiceAdapter, VoiceConfig, VoiceDelivery},
        };
        let (mut studio, directory) = studio();
        let (endpoint, requests) = mock_tts_transport(true);
        studio.library.voice = VoiceConfig {
            enabled: true,
            endpoint,
            delivery: VoiceDelivery::TimestampSse,
            ..VoiceConfig::preset(VoiceAdapter::FishHosted)
        };
        studio.voice_key = "offline-test-key".into();
        let id = Uuid::new_v4();
        let first = "This first sentence is ready while the model is streaming.";
        let second = "This second sentence waits until playback has drained.";
        studio.begin_reply_speech(id);
        studio.feed_reply_speech(id, &format!("{first} {second} Tail"));
        let mut fish = FishLiveState::default();
        studio.pump_sse(&mut fish, false);
        assert_eq!(
            requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            first
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut done = false;
        let mut played = false;
        while !done && std::time::Instant::now() < deadline {
            for event in fish.poll() {
                if let LiveEvent::Playing { audio, .. } = &event {
                    use bevy::audio::Decodable;
                    assert_eq!(audio.decoder().collect::<Vec<_>>(), vec![0., 0.]);
                    played = true;
                }
                done |= matches!(event, LiveEvent::Finished { .. });
                studio
                    .pending_sse
                    .as_mut()
                    .unwrap()
                    .receive(&event)
                    .unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(played && done);
        assert!(studio.voice_busy, "LLM is not finished");
        studio.pump_sse(&mut fish, true);
        assert!(
            requests.try_recv().is_err(),
            "network completion cannot cancel draining playback"
        );
        studio.pump_sse(&mut fish, false);
        assert_eq!(
            requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            second
        );
        studio.stop_voice();
        fish.cancel();
        studio.feed_reply_speech(id, "Do not speak after stop.");
        assert!(studio.finish_reply_speech(id));
        assert!(studio.pending_sse.is_none());
        studio.pump_sse(&mut fish, false);
        assert!(requests.try_recv().is_err());
        std::fs::remove_dir_all(directory).unwrap();
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
        assert_eq!(studio.pending_http.as_ref().unwrap().ready_count(), 1);
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
        assert_eq!(studio.pending_http.as_ref().unwrap().outstanding(), 1);
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
    fn http_reply_failure_stops_reply_and_explicit_transports_still_work() {
        let (mut studio, directory) = studio();
        studio.library.voice.enabled = true;
        // Hosted speech without any key is skipped quietly; give this session one.
        studio.voice_key = "session-secret".into();
        // A new setup takes the HTTP path; selecting WebSocket still bypasses it.
        studio.begin_reply_speech(Uuid::new_v4());
        assert!(studio.pending_http.is_some() && studio.pending_ws.is_none());
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::LiveWebSocket;
        studio.begin_reply_speech(Uuid::new_v4());
        assert!(studio.pending_ws.is_some() && studio.pending_http.is_none());
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::TimestampSse;
        studio.begin_reply_speech(Uuid::new_v4());
        assert!(studio.pending_http.is_none() && studio.pending_sse.is_some());
        assert!(studio.reply_speech_job.is_some());
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::Buffered;
        let id = Uuid::new_v4();
        studio.begin_reply_speech(id);
        let voice = studio.voice_id;
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
    fn voice_previews_are_local_without_changing_saved_reply_output() {
        let (mut studio, directory) = studio();
        studio.library.voice.output = waifu_studio::voice::VoiceOutput::LocalAndCalls;
        // This transport queues a request; no polling/provider access occurs.
        studio.library.voice.delivery = waifu_studio::voice::VoiceDelivery::TimestampWebSocket;
        studio.preview_voice();
        assert_eq!(
            studio.active_voice.as_ref().unwrap().output,
            waifu_studio::voice::VoiceOutput::LocalOnly
        );
        assert_eq!(
            studio.library.voice.output,
            waifu_studio::voice::VoiceOutput::LocalAndCalls
        );
        studio.begin_design_preview();
        assert_eq!(
            studio.active_voice.as_ref().unwrap().output,
            waifu_studio::voice::VoiceOutput::LocalOnly
        );
        assert_eq!(
            studio.library.voice.output,
            waifu_studio::voice::VoiceOutput::LocalAndCalls
        );
        studio.stop_voice();
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
                    external: None,
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
    fn quick_combinations_preserve_source_restore_existing_and_persist_separate_history() {
        let (mut studio, directory) = studio();
        let original = studio.library.selected.unwrap();
        let model = studio.library.selected_character().unwrap().model.clone();
        studio.library.voice.direction = "Warm source voice".into();
        studio.library.scene.background = [0.2, 0.3, 0.4];
        studio
            .store
            .save_history(
                original,
                &[Turn {
                    external: None,
                    role: "user".into(),
                    content: "Source history".into(),
                }],
            )
            .unwrap();
        studio.use_character_combination(
            model.clone(),
            "Different personality".into(),
            "New combination".into(),
        );
        let combination = studio.library.selected.unwrap();
        assert_ne!(combination, original);
        assert!(studio.turns.is_empty());
        assert_eq!(
            studio.library.selected_character().unwrap().persona,
            "Different personality"
        );
        assert_eq!(studio.library.voice.direction, "Warm source voice");
        assert_eq!(studio.library.scene.background, [0.2, 0.3, 0.4]);
        assert_eq!(
            studio
                .library
                .characters
                .iter()
                .find(|c| c.id == original)
                .unwrap()
                .persona,
            "Friendly"
        );
        studio.select_character(original);
        assert_eq!(studio.turns[0].content, "Source history");
        studio.use_character_combination(
            model,
            "Different personality".into(),
            "Ignored name".into(),
        );
        assert_eq!(studio.library.selected, Some(combination));
        assert_eq!(studio.library.characters.len(), 2);
        let (_, restored) = Store::open(directory.clone()).unwrap();
        assert_eq!(restored.selected, Some(combination));
        assert_eq!(
            restored.selected_character().unwrap().name,
            "New combination"
        );
        let count = studio.library.characters.len();
        studio.use_character_combination("unknown.vrm".into(), "Other".into(), "Invalid".into());
        assert_eq!(studio.library.characters.len(), count);
        assert_eq!(studio.library.selected, Some(combination));
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
