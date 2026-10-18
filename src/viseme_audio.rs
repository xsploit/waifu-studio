//! Bounded playback-to-MFCC bridge. The audio callback only writes atomic PCM
//! slots; profile IO, FFT, scoring and smoothing run on one background worker.
use crate::lipsync::{
    ChannelMode, LipSyncAnalyzer, LipSyncProfile, LipSyncSettings, MAX_PROFILE_BYTES,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

const PCM_CAPACITY: usize = 16_384;
const OUTPUT_MAX_AGE_US: u64 = 250_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisemeConfig {
    pub enabled: bool,
    pub profile_path: Option<PathBuf>,
    pub min_volume: f32,
    pub max_volume: f32,
    pub smoothness: f32,
    pub block_size: u32,
    pub use_phoneme_blend: bool,
    pub vowel_gains: [f32; 5],
    pub average_channels: bool,
}
impl Default for VisemeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile_path: None,
            min_volume: -2.5,
            max_volume: -1.5,
            smoothness: 0.05,
            block_size: 512,
            use_phoneme_blend: false,
            vowel_gains: [1.0; 5],
            average_channels: false,
        }
    }
}
impl VisemeConfig {
    fn settings(&self) -> Result<LipSyncSettings, String> {
        if !(128..=2048).contains(&self.block_size) {
            return Err("Analysis block size must be 128–2048 (at 48 kHz)".into());
        }
        let settings = LipSyncSettings {
            min_volume: self.min_volume,
            max_volume: self.max_volume,
            smoothness: self.smoothness,
            use_phoneme_blend: self.use_phoneme_blend,
            vowel_gains: self.vowel_gains,
            hop_seconds: self.block_size as f32 / 48_000.0,
            channel_mode: ChannelMode::First,
        };
        settings.validate().map_err(|e| e.to_string())?;
        Ok(settings)
    }
}

// One producer (a claimed speech decoder), one consumer (the analysis worker).
// Atomics avoid unsafe cell aliasing and never overwrite an unread sample.
struct SampleRing {
    slots: Box<[AtomicU32]>,
    read: AtomicUsize,
    write: AtomicUsize,
    dropped: AtomicU64,
}
impl SampleRing {
    fn new() -> Self {
        Self {
            slots: (0..PCM_CAPACITY).map(|_| AtomicU32::new(0)).collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }
    fn push(&self, sample: f32) {
        let write = self.write.load(Ordering::Relaxed);
        let next = (write + 1) & (PCM_CAPACITY - 1);
        if next == self.read.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.slots[write].store(sample.to_bits(), Ordering::Relaxed);
        self.write.store(next, Ordering::Release);
    }
    fn pop(&self, out: &mut [f32]) -> usize {
        let mut read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let mut count = 0;
        while read != write && count < out.len() {
            out[count] = f32::from_bits(self.slots[read].load(Ordering::Relaxed));
            read = (read + 1) & (PCM_CAPACITY - 1);
            count += 1;
        }
        self.read.store(read, Ordering::Release);
        count
    }
    fn clear(&self) {
        self.read
            .store(self.write.load(Ordering::Acquire), Ordering::Release);
    }
}
struct SharedOutput {
    version: AtomicU64,
    weights: [AtomicU32; 5],
    updated: AtomicU64,
    revision: AtomicU64,
    valid: AtomicBool,
}
impl SharedOutput {
    fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            weights: std::array::from_fn(|_| AtomicU32::new(0)),
            updated: AtomicU64::new(0),
            revision: AtomicU64::new(0),
            valid: AtomicBool::new(false),
        }
    }
    fn invalidate(&self) {
        self.valid.store(false, Ordering::Release);
    }
    fn publish(&self, weights: [f32; 5], now: u64, revision: u64) {
        self.version.fetch_add(1, Ordering::AcqRel);
        for (slot, value) in self.weights.iter().zip(weights) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
        self.updated.store(now, Ordering::Relaxed);
        self.revision.store(revision, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Release);
        self.valid.store(true, Ordering::Release);
    }
    fn snapshot(&self, now: u64, revision: u64) -> Option<[f32; 5]> {
        if !self.valid.load(Ordering::Acquire) {
            return None;
        }
        let version = self.version.load(Ordering::Acquire);
        if version & 1 != 0 {
            return None;
        }
        let weights =
            std::array::from_fn(|i| f32::from_bits(self.weights[i].load(Ordering::Relaxed)));
        let updated = self.updated.load(Ordering::Relaxed);
        let actual_revision = self.revision.load(Ordering::Relaxed);
        // Keep the payload reads before the closing version check, including
        // on weakly ordered architectures. The opening acquire pairs with the
        // worker's completed publication; all payload fields are atomic.
        std::sync::atomic::fence(Ordering::Acquire);
        if self.version.load(Ordering::Relaxed) != version
            || actual_revision != revision
            || now.saturating_sub(updated) > OUTPUT_MAX_AGE_US
        {
            return None;
        }
        if weights
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            Some(weights)
        } else {
            None
        }
    }
}
struct Session {
    ring: SampleRing,
    output: SharedOutput,
    generation: Arc<AtomicU64>,
    expected: u64,
    revision: AtomicU64,
    rate: AtomicU32,
    finished: AtomicBool,
    epoch: Instant,
}
impl Session {
    fn new(generation: Arc<AtomicU64>, expected: u64) -> Self {
        Self {
            ring: SampleRing::new(),
            output: SharedOutput::new(),
            generation,
            expected,
            revision: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            finished: AtomicBool::new(false),
            epoch: Instant::now(),
        }
    }
    fn current(&self) -> bool {
        !self.finished.load(Ordering::Acquire)
            && self.generation.load(Ordering::Acquire) == self.expected
    }
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
}
/// Created on the frame thread. Clone only after a playback decoder has won its
/// sole-producer claim. Exactly one clone may configure, push or finish.
/// configure/push/finish never allocate, lock, send channels or run analysis.
#[derive(Clone)]
pub struct VisemeTap {
    session: Option<Arc<Session>>,
    channels: usize,
    phase: usize,
    sum: f32,
    average: bool,
}
impl VisemeTap {
    fn inactive() -> Self {
        Self {
            session: None,
            channels: 1,
            phase: 0,
            sum: 0.0,
            average: false,
        }
    }
    pub fn configure(&mut self, sample_rate: u32, channels: usize) {
        self.phase = 0;
        self.sum = 0.0;
        if !(1..=32).contains(&channels) {
            self.finish();
            return;
        }
        self.channels = channels;
        if let Some(session) = &self.session {
            session.rate.store(sample_rate, Ordering::Release);
        }
    }
    #[inline]
    pub fn push(&mut self, sample: f32) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.current() {
            return;
        }
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if self.average {
            self.sum += sample;
        } else if self.phase == 0 {
            session.ring.push(sample);
        }
        self.phase += 1;
        if self.phase == self.channels {
            if self.average {
                session.ring.push(self.sum / self.channels as f32);
            }
            self.phase = 0;
            self.sum = 0.0;
        }
    }
    pub fn finish(&mut self) {
        if let Some(session) = &self.session {
            session.finished.store(true, Ordering::Release);
        }
    }
}

enum Command {
    Load {
        id: u64,
        path: PathBuf,
    },
    Start {
        session: Arc<Session>,
        profile: Arc<LipSyncProfile>,
        settings: LipSyncSettings,
    },
    Settings {
        expected: u64,
        revision: u64,
        settings: LipSyncSettings,
    },
}
enum Event {
    Loaded {
        id: u64,
        path: PathBuf,
        result: Result<Arc<LipSyncProfile>, String>,
    },
    Status {
        expected: u64,
        text: String,
    },
}
struct ActiveAnalysis {
    session: Arc<Session>,
    profile: Arc<LipSyncProfile>,
    settings: LipSyncSettings,
    revision: u64,
    analyzer: Option<LipSyncAnalyzer>,
    rate: u32,
    overflow: u64,
}

#[derive(Resource)]
pub struct VisemeState {
    pub config: VisemeConfig,
    /// Successfully validated profile, which may differ from an invalid draft.
    pub profile_path: Option<PathBuf>,
    pub profile_draft: String,
    pub status: String,
    applied: VisemeConfig,
    profile: Option<Arc<LipSyncProfile>>,
    generation: Arc<AtomicU64>,
    session: Option<Arc<Session>>,
    commands: SyncSender<Command>,
    events: Mutex<Receiver<Event>>,
    load_id: u64,
    loading: bool,
    dialog: Option<Mutex<Receiver<Option<PathBuf>>>>,
}
impl Default for VisemeState {
    fn default() -> Self {
        let (commands, receiver) = mpsc::sync_channel(4);
        let (sender, events) = mpsc::sync_channel(8);
        let worker = std::thread::Builder::new()
            .name("native-mfcc-visemes".into())
            .spawn(move || worker(receiver, sender));
        Self {
            config: VisemeConfig::default(),
            applied: VisemeConfig::default(),
            profile_path: None,
            profile_draft: String::new(),
            status: if worker.is_ok() {
                "RMS fallback · import a calibration profile to enable MFCC vowels".into()
            } else {
                "MFCC worker could not start · RMS fallback".into()
            },
            profile: None,
            generation: Arc::new(AtomicU64::new(0)),
            session: None,
            commands,
            events: Mutex::new(events),
            load_id: 0,
            loading: false,
            dialog: None,
        }
    }
}
impl VisemeState {
    pub fn begin_speech(&mut self) -> VisemeTap {
        self.cancel();
        self.poll();
        if !self.config.enabled {
            return VisemeTap::inactive();
        }
        let Some(profile) = self.profile.clone() else {
            return VisemeTap::inactive();
        };
        let settings = match self.config.settings() {
            Ok(s) => s,
            Err(error) => {
                self.status = error;
                return VisemeTap::inactive();
            }
        };
        let expected = self.generation.load(Ordering::Acquire);
        let session = Arc::new(Session::new(self.generation.clone(), expected));
        let command = Command::Start {
            session: session.clone(),
            profile,
            settings,
        };
        match self.commands.try_send(command) {
            Ok(()) => {
                self.session = Some(session.clone());
                self.status = "Waiting for playback samples · MFCC worker".into();
                VisemeTap {
                    session: Some(session),
                    channels: 1,
                    phase: 0,
                    sum: 0.0,
                    average: self.config.average_channels,
                }
            }
            Err(_) => {
                self.status = "MFCC worker busy or unavailable · RMS fallback".into();
                VisemeTap::inactive()
            }
        }
    }
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(session) = self.session.take() {
            session.finished.store(true, Ordering::Release);
        }
    }
    pub fn weights(&self) -> Option<[f32; 5]> {
        if !self.config.enabled {
            return None;
        }
        let session = self.session.as_ref()?;
        if !session.current() {
            return None;
        }
        session
            .output
            .snapshot(session.now(), session.revision.load(Ordering::Acquire))
    }
    pub fn dropped_samples(&self) -> u64 {
        self.session
            .as_ref()
            .map(|s| s.ring.dropped.load(Ordering::Relaxed))
            .unwrap_or(0)
    }
    pub fn loading(&self) -> bool {
        self.loading
    }
    pub fn vowel_coverage(&self) -> Option<[bool; 5]> {
        self.profile.as_ref().map(|p| p.vowel_coverage())
    }
    pub fn import_profile(&mut self, path: PathBuf) -> Result<(), String> {
        self.config.profile_path = Some(path);
        self.apply_config(true)
    }
    pub fn clear_profile(&mut self) {
        self.config.profile_path = None;
        let _ = self.apply_config(true);
    }
    fn apply_config(&mut self, force_load: bool) -> Result<(), String> {
        let previous = self.applied.clone();
        self.applied = self.config.clone();
        if force_load || previous.profile_path != self.config.profile_path {
            self.cancel();
            self.profile = None;
            self.profile_path = None;
            self.load_id = self.load_id.wrapping_add(1);
            self.loading = false;
            if let Some(path) = self.config.profile_path.clone() {
                self.profile_draft = path.to_string_lossy().into_owned();
                self.commands
                    .try_send(Command::Load {
                        id: self.load_id,
                        path,
                    })
                    .map_err(|_| {
                        self.status = "Profile worker busy/unavailable · retry Load profile".into();
                        self.status.clone()
                    })?;
                self.loading = true;
                self.status = "Loading and validating calibration profile…".into();
            } else {
                self.profile_draft.clear();
                self.status = "No calibration profile · RMS fallback".into();
            }
            return Ok(());
        }
        if !self.config.enabled || previous.average_channels != self.config.average_channels {
            self.cancel();
        }
        let settings = match self.config.settings() {
            Ok(s) => s,
            Err(error) => {
                self.cancel();
                self.status = format!("{error} · RMS fallback");
                return Err(error);
            }
        };
        if let Some(session) = &self.session {
            let revision = session.revision.fetch_add(1, Ordering::AcqRel) + 1;
            if self
                .commands
                .try_send(Command::Settings {
                    expected: session.expected,
                    revision,
                    settings,
                })
                .is_err()
            {
                self.cancel();
                self.status = "MFCC tuning queue busy/unavailable · RMS fallback; new tuning applies on next speech".into();
                return Err(self.status.clone());
            }
        }
        Ok(())
    }
    pub fn poll(&mut self) {
        if self.config != self.applied {
            let _ = self.apply_config(false);
        }
        while let Ok(event) = self
            .events
            .get_mut()
            .expect("exclusive event receiver")
            .try_recv()
        {
            match event {
                Event::Loaded { id, path, result }
                    if id == self.load_id && self.config.profile_path.as_ref() == Some(&path) =>
                {
                    self.loading = false;
                    match result {
                        Ok(profile) => {
                            self.status = format!(
                                "Loaded {} calibrated phonemes · {} Hz target · {} samples",
                                profile.phonemes.len(),
                                profile.target_sample_rate,
                                profile.sample_count
                            );
                            self.profile = Some(profile);
                            self.profile_path = Some(path);
                        }
                        Err(error) => {
                            self.status = format!("{error} · RMS fallback");
                            self.profile = None;
                            self.profile_path = None;
                        }
                    }
                }
                Event::Status { expected, text }
                    if self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.expected == expected) =>
                {
                    self.status = text
                }
                _ => {}
            }
        }
    }
    pub fn open_profile_dialog(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("viseme-profile-dialog".into())
            .spawn(move || {
                let path = rfd::FileDialog::new()
                    .set_title("Import uLipSync / wLipSync calibration")
                    .add_filter("Calibration profile", &["json", "bin"])
                    .pick_file();
                let _ = sender.send(path);
            })
            .is_ok()
        {
            self.dialog = Some(Mutex::new(receiver));
        }
    }
    /// Discard a pending selection when its owning character/profile changes.
    /// The native dialog may close later, but its result can no longer be applied.
    pub fn cancel_profile_dialog(&mut self) {
        self.dialog = None;
    }
    /// Called by the UI so imported paths participate in its persistence change receipt.
    pub fn poll_dialog(&mut self) {
        let event = self
            .dialog
            .as_mut()
            .and_then(|r| r.get_mut().ok()?.try_recv().ok());
        if let Some(path) = event {
            self.dialog = None;
            if let Some(path) = path {
                let _ = self.import_profile(path);
            }
        }
    }
}
impl Drop for VisemeState {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn load_profile(path: &Path) -> Result<Arc<LipSyncProfile>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "Could not open calibration profile")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Could not inspect calibration profile")?;
    if !metadata.is_file() || metadata.len() > MAX_PROFILE_BYTES as u64 {
        return Err("Choose a regular calibration JSON/WLIP file no larger than 8 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PROFILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read calibration profile")?;
    LipSyncProfile::from_bytes(&bytes)
        .map(Arc::new)
        .map_err(|e| e.to_string())
}
fn handle_command(
    command: Command,
    active: &mut Option<ActiveAnalysis>,
    events: &SyncSender<Event>,
) {
    match command {
        Command::Load { id, path } => {
            *active = None;
            let result = load_profile(&path);
            let _ = events.send(Event::Loaded { id, path, result });
        }
        Command::Start {
            session,
            profile,
            settings,
        } => {
            if session.current() {
                *active = Some(ActiveAnalysis {
                    session,
                    profile,
                    settings,
                    revision: 0,
                    analyzer: None,
                    rate: 0,
                    overflow: 0,
                });
            }
        }
        Command::Settings {
            expected,
            revision,
            settings,
        } => {
            if let Some(active) = active
                && active.session.expected == expected
            {
                active.settings = settings;
                active.revision = revision;
                active.analyzer = None;
                active.session.ring.clear();
                active.session.output.invalidate();
            }
        }
    }
}
fn worker(commands: Receiver<Command>, events: SyncSender<Event>) {
    let mut active: Option<ActiveAnalysis> = None;
    let mut scratch = [0.0f32; 1024];
    loop {
        match commands.recv_timeout(if active.is_some() {
            Duration::from_millis(3)
        } else {
            Duration::from_millis(100)
        }) {
            Ok(command) => handle_command(command, &mut active, &events),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(command) = commands.try_recv() {
            handle_command(command, &mut active, &events);
        }
        let Some(current) = &mut active else {
            continue;
        };
        if !current.session.current() {
            active = None;
            continue;
        }
        let rate = current.session.rate.load(Ordering::Acquire);
        if rate == 0 {
            continue;
        }
        if current.analyzer.is_none() || current.rate != rate {
            match LipSyncAnalyzer::new(current.profile.clone(), rate, current.settings) {
                Ok(analyzer) => {
                    current.analyzer = Some(analyzer);
                    current.rate = rate;
                    let _ = events.try_send(Event::Status {
                        expected: current.session.expected,
                        text: "Native MFCC vowel analysis active · A I U E O".into(),
                    });
                }
                Err(error) => {
                    current.session.output.invalidate();
                    let _ = events.try_send(Event::Status {
                        expected: current.session.expected,
                        text: format!("{error} · RMS fallback"),
                    });
                    active = None;
                    continue;
                }
            }
        }
        for _ in 0..8 {
            if !current.session.current() {
                break;
            }
            let overflow = current.session.ring.dropped.load(Ordering::Acquire);
            if overflow != current.overflow {
                current.overflow = overflow;
                current.session.ring.clear();
                current.analyzer.as_mut().unwrap().reset();
                current.session.output.invalidate();
                break;
            }
            let count = current.session.ring.pop(&mut scratch);
            if count == 0 {
                break;
            }
            let analyzer = current.analyzer.as_mut().unwrap();
            let frames = analyzer.push_mono(&scratch[..count]).unwrap_or(0);
            if frames > 0
                && current.session.current()
                && current.session.revision.load(Ordering::Acquire) == current.revision
                && current.session.ring.dropped.load(Ordering::Acquire) == current.overflow
            {
                current.session.output.publish(
                    analyzer.output().vowels,
                    current.session.now(),
                    current.revision,
                );
            }
        }
    }
}

pub struct VisemePlugin;
impl Plugin for VisemePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisemeState>()
            .add_systems(Update, poll_visemes);
    }
}
fn poll_visemes(mut state: ResMut<VisemeState>) {
    state.poll();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lipsync::{CompareMethod, ProfilePhoneme, Vowel};

    // Artificial one-vowel calibration exercises routing, not recognition quality.
    fn fixture_profile() -> Arc<LipSyncProfile> {
        Arc::new(LipSyncProfile {
            target_sample_rate: 16_000,
            sample_count: 1024,
            mel_filter_bank_channels: 30,
            compare_method: CompareMethod::L2Norm,
            use_standardization: false,
            phonemes: vec![ProfilePhoneme {
                name: "A".into(),
                vowel: Some(Vowel::A),
                mfcc: [1.0; 12],
            }],
            means: [0.0; 12],
            standard_deviations: [1.0; 12],
        })
    }
    fn fixture_state() -> VisemeState {
        let mut state = VisemeState::default();
        state.config.enabled = true;
        state.config.smoothness = 0.0;
        state.config.min_volume = -5.0;
        state.config.max_volume = -3.0;
        state.applied = state.config.clone();
        state.profile = Some(fixture_profile());
        state
    }
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "worker did not reach expected state"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn sine_wave(rate: u32, samples: usize) -> Vec<u8> {
        let size = samples as u32 * 2;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + size).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(size.to_le_bytes());
        for i in 0..samples {
            let value =
                ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12000.0) as i16;
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn metered_decoded_samples_reach_worker_and_duplicate_eof_cannot_finish_session() {
        use crate::metered_audio::SpeechMeter;
        use bevy::audio::Decodable;
        let mut state = fixture_state();
        let tap = state.begin_speech();
        let meter = SpeechMeter::default();
        let source = meter.buffered_with_visemes(
            AudioSource {
                bytes: sine_wave(16_000, 8192).into(),
            },
            tap,
        );
        let mut decoder = source.decoder();
        assert_eq!(
            state.weights(),
            None,
            "queued but unplayed audio must not animate"
        );
        let consumed: Vec<f32> = decoder.by_ref().take(4096).collect();
        let mut reference =
            LipSyncAnalyzer::new(fixture_profile(), 16_000, state.config.settings().unwrap())
                .unwrap();
        reference.push_mono(&consumed).unwrap();
        wait_until(|| {
            state.poll();
            state.weights().is_some()
        });
        assert_eq!(state.weights(), Some(reference.output().vowels));
        assert!(state.weights().unwrap()[0] > 0.9);
        let mut duplicate = source.decoder();
        assert_eq!(duplicate.next(), None);
        drop(duplicate);
        assert!(state.weights().is_some());
        decoder.by_ref().for_each(drop);
        assert_eq!(
            state.weights(),
            None,
            "actual decoder EOF immediately silences vowels"
        );
        let fresh_tap = state.begin_speech();
        let fresh = meter.buffered_with_visemes(
            AudioSource {
                bytes: sine_wave(16_000, 8192).into(),
            },
            fresh_tap,
        );
        let mut fresh_decoder = fresh.decoder();
        fresh_decoder.by_ref().take(4096).for_each(drop);
        wait_until(|| state.weights().is_some());
        drop(decoder);
        assert!(
            state.weights().is_some(),
            "old decoder Drop cannot erase a new generation"
        );
        state.cancel();
        assert_eq!(state.weights(), None);
    }
    #[test]
    fn unsupported_decoder_rate_retains_rms_fallback() {
        use bevy::audio::Decodable;
        let mut state = fixture_state();
        let meter = crate::metered_audio::SpeechMeter::default();
        let source = meter.buffered_with_visemes(
            AudioSource {
                bytes: sine_wave(22_050, 8192).into(),
            },
            state.begin_speech(),
        );
        let mut decoder = source.decoder();
        decoder.by_ref().take(4096).for_each(drop);
        wait_until(|| {
            state.poll();
            state.status.contains("unsupported")
        });
        assert_eq!(state.weights(), None);
        assert!(
            meter.level() > 0.1,
            "existing RMS remains available on unsupported analysis rate"
        );
    }
    #[test]
    fn worker_discards_overflow_and_requires_new_pcm_before_publishing() {
        let (send, receive) = mpsc::sync_channel(4);
        let (events, _receive_events) = mpsc::sync_channel(8);
        let generation = Arc::new(AtomicU64::new(1));
        let session = Arc::new(Session::new(generation, 1));
        session.rate.store(16_000, Ordering::Release);
        for _ in 0..PCM_CAPACITY + 100 {
            session.ring.push(0.3);
        }
        session.output.publish([0.8; 5], session.now(), 0);
        send.send(Command::Start {
            session: session.clone(),
            profile: fixture_profile(),
            settings: VisemeConfig {
                smoothness: 0.0,
                ..Default::default()
            }
            .settings()
            .unwrap(),
        })
        .unwrap();
        let worker = std::thread::spawn(move || worker(receive, events));
        wait_until(|| {
            session.ring.read.load(Ordering::Acquire) == session.ring.write.load(Ordering::Acquire)
        });
        wait_until(|| session.output.snapshot(session.now(), 0).is_none());
        for i in 0..4096 {
            session.ring.push((i as f32 * 0.1).sin() * 0.3);
        }
        wait_until(|| session.output.snapshot(session.now(), 0).is_some());
        session.finished.store(true, Ordering::Release);
        drop(send);
        worker.join().unwrap();
    }
    #[test]
    fn tuning_queue_failure_cancels_with_visible_rms_fallback() {
        let mut state = fixture_state();
        let expected = state.generation.load(Ordering::Acquire);
        let session = Arc::new(Session::new(state.generation.clone(), expected));
        session.output.publish([0.5; 5], session.now(), 0);
        state.session = Some(session);
        assert_eq!(state.weights(), Some([0.5; 5]));
        let (commands, _blocked_receiver) = mpsc::sync_channel(0);
        state.commands = commands;
        state.config.smoothness = 0.1;
        state.poll();
        assert_eq!(state.weights(), None);
        assert!(state.status.contains("RMS fallback"));
        assert!(state.status.contains("next speech"));
    }
    #[test]
    fn discarded_character_dialog_cannot_import_into_next_character() {
        let mut state = fixture_state();
        let (send, receive) = mpsc::sync_channel(1);
        state.dialog = Some(Mutex::new(receive));
        state.cancel_profile_dialog();
        assert!(
            send.send(Some(PathBuf::from("previous-character.json")))
                .is_err()
        );
        state.poll_dialog();
        assert_eq!(state.config.profile_path, None);
        assert!(state.vowel_coverage().is_some());
    }
    #[test]
    fn ring_is_bounded_fifo_and_overflow_is_explicit() {
        let ring = SampleRing::new();
        for i in 0..PCM_CAPACITY + 10 {
            ring.push(i as f32);
        }
        assert_eq!(ring.dropped.load(Ordering::Relaxed), 11);
        let mut out = [0.0; 4];
        assert_eq!(ring.pop(&mut out), 4);
        assert_eq!(out, [0.0, 1.0, 2.0, 3.0]);
        ring.clear();
        assert_eq!(ring.pop(&mut out), 0);
    }
    #[test]
    fn stale_generation_finish_and_output_age_cannot_leak_weights() {
        let generation = Arc::new(AtomicU64::new(1));
        let old = Arc::new(Session::new(generation.clone(), 1));
        old.output.publish([0.5; 5], 0, 0);
        assert_eq!(old.output.snapshot(1, 0), Some([0.5; 5]));
        assert_eq!(old.output.snapshot(OUTPUT_MAX_AGE_US + 1, 0), None);
        generation.store(2, Ordering::Release);
        let fresh = Arc::new(Session::new(generation.clone(), 2));
        let mut old_tap = VisemeTap {
            session: Some(old),
            ..VisemeTap::inactive()
        };
        old_tap.finish();
        assert!(fresh.current());
        fresh.output.publish([0.2; 5], 0, 0);
        assert_eq!(fresh.output.snapshot(1, 0), Some([0.2; 5]));
        assert_eq!(fresh.output.snapshot(1, 1), None);
    }
    #[test]
    fn callback_channel_selection_and_cancel_are_bounded() {
        let generation = Arc::new(AtomicU64::new(1));
        let session = Arc::new(Session::new(generation.clone(), 1));
        let mut tap = VisemeTap {
            session: Some(session.clone()),
            ..VisemeTap::inactive()
        };
        tap.configure(48_000, 2);
        for sample in [0.1, 0.9, 0.2, 0.8] {
            tap.push(sample);
        }
        let mut out = [0.0; 4];
        assert_eq!(session.ring.pop(&mut out), 2);
        assert_eq!(&out[..2], &[0.1, 0.2]);
        generation.store(2, Ordering::Release);
        tap.push(0.3);
        assert_eq!(session.ring.pop(&mut out), 0);
    }
    #[test]
    fn invalid_tuning_and_profiles_fail_without_needing_audio_hardware() {
        assert!(
            VisemeConfig {
                block_size: 0,
                ..Default::default()
            }
            .settings()
            .is_err()
        );
        assert!(
            VisemeConfig {
                min_volume: f32::NAN,
                ..Default::default()
            }
            .settings()
            .is_err()
        );
        let path = std::env::temp_dir().join(format!("bad-lipsync-{}.json", std::process::id()));
        std::fs::write(&path, b"{}").unwrap();
        assert!(load_profile(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
