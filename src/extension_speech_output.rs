//! Main-thread speech output ownership. Never called from an audio callback.
//! Receipt intake is lock-free with respect to ExtensionRuntime: frame holds
//! that runtime's lock while invoking the sink. Transport work happens in tick.
use crate::{
    call_pcm::CallPcmFrame,
    extension_playback_ledger::PlaybackLedger,
    extension_protocol::{
        AudioOutput, AudioPlaybackOutcome, AudioPlaybackReceipt, ErrorCode, IntegrationOrigin,
        MAX_OUTPUT_FRAMES, Target,
    },
    extension_runtime::{AudioPlaybackRoute, ExtensionRuntime},
};
use bevy::prelude::Resource;
use std::{
    collections::{BTreeMap, VecDeque},
    time::Instant,
};
use uuid::Uuid;
const MAX_ROUTES: usize = 4;
const MAX_BUFFERED_FRAMES: usize = 64;
const MAX_OUTCOMES: usize = 32;
const MAX_RECENT: usize = 64;
type Failure = (ErrorCode, String);
struct Output {
    source: Uuid,
    ledger: PlaybackLedger,
    frames: VecDeque<CallPcmFrame>,
    eof: bool,
    cancelled: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechOutputOutcome {
    pub source: Uuid,
    pub playback_id: Uuid,
    pub route: AudioPlaybackRoute,
    pub outcome: AudioPlaybackOutcome,
}
#[derive(Resource)]
pub struct SpeechOutputs {
    epoch: Instant,
    active: BTreeMap<Uuid, Output>,
    ready: BTreeMap<Uuid, (AudioPlaybackRoute, bool)>,
    outcomes: VecDeque<SpeechOutputOutcome>,
    recent: VecDeque<(Uuid, Uuid)>,
}
impl Default for SpeechOutputs {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}
fn error(code: ErrorCode, message: &str) -> Failure {
    (code, message.into())
}
impl SpeechOutputs {
    pub fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            active: BTreeMap::new(),
            ready: BTreeMap::new(),
            outcomes: VecDeque::new(),
            recent: VecDeque::new(),
        }
    }
    pub fn is_ready(&self, installation: Uuid, target: Target) -> bool {
        self.ready
            .get(&installation)
            .is_some_and(|(route, ready)| *ready && route.target == target)
    }
    pub fn ready_routes(&self, target: Target) -> Vec<AudioPlaybackRoute> {
        self.ready
            .values()
            .filter(|(route, ready)| *ready && route.target == target)
            .map(|(route, _)| route.clone())
            .collect()
    }
    /// Called under the runtime frame lock: use only the earlier host snapshot.
    pub fn receive_available(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        session: Uuid,
        generation: u64,
    ) -> Result<(), Failure> {
        let (_, ready) = self
            .ready
            .values_mut()
            .find(|(route, _)| {
                route.target == target
                    && &route.origin == origin
                    && route.session == session
                    && route.process_generation == generation
            })
            .ok_or_else(|| {
                error(
                    ErrorCode::InvalidRequest,
                    "Call readiness does not match an eligible route",
                )
            })?;
        *ready = true;
        Ok(())
    }
    pub fn ready_routes_for_character(&self, character: Uuid) -> Vec<AudioPlaybackRoute> {
        self.ready
            .values()
            .filter(|(route, ready)| *ready && route.target.uuid == character)
            .map(|(route, _)| route.clone())
            .collect()
    }
    /// Read-only UI snapshot: cached readiness alone must not survive a
    /// revocation, target replacement or process restart before the next tick.
    pub fn current_ready_routes_for_character(
        &self,
        runtime: &ExtensionRuntime,
        character: Uuid,
    ) -> Vec<AudioPlaybackRoute> {
        let current = runtime.current_audio_playback_routes(character);
        self.ready_routes_for_character(character)
            .into_iter()
            .filter(|route| current.contains(route))
            .collect()
    }
    pub fn has_source(&self, source: Uuid) -> bool {
        self.active.values().any(|o| o.source == source)
    }
    pub fn take_source_outcome(&mut self, source: Uuid) -> Option<SpeechOutputOutcome> {
        let index = self.outcomes.iter().position(|o| o.source == source)?;
        self.outcomes.remove(index)
    }
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
    pub fn buffered_frames(&self) -> usize {
        self.active.values().map(|o| o.frames.len()).sum()
    }
    pub fn take_outcome(&mut self) -> Option<SpeechOutputOutcome> {
        self.outcomes.pop_front()
    }
    pub fn begin(
        &mut self,
        runtime: &ExtensionRuntime,
        installation: Uuid,
        target: Target,
        source: Uuid,
        now: Instant,
    ) -> Result<Uuid, Failure> {
        let time = now
            .checked_duration_since(self.epoch)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "Invalid playback clock"))?;
        if source.is_nil() {
            return Err(error(ErrorCode::InvalidRequest, "Invalid speech source"));
        }
        // Reserve a terminal-outcome slot at admission; retain every admitted
        // outcome until consumed instead of silently overwriting an earlier one.
        if self.active.len() >= MAX_ROUTES
            || self.active.contains_key(&installation)
            || self.active.len() + self.outcomes.len() >= MAX_OUTCOMES
        {
            return Err(error(ErrorCode::Busy, "Speech output is busy"));
        }
        if self.recent.contains(&(installation, source)) {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Speech source was already routed",
            ));
        }
        let route = runtime
            .audio_playback_route(installation, target)
            .ok_or_else(|| {
                error(
                    ErrorCode::PermissionDenied,
                    "Speech output route is unavailable",
                )
            })?;
        if !self
            .ready
            .get(&installation)
            .is_some_and(|(candidate, ready)| *ready && candidate == &route)
        {
            return Err(error(ErrorCode::Unavailable, "Call audio is not ready"));
        }
        let mut ledger = PlaybackLedger::new(route.clone(), time);
        let id = ledger.playback_id();
        let output = AudioOutput::Start {
            playback_id: id,
            target,
        };
        self.recent.push_back((installation, source));
        if self.recent.len() > MAX_RECENT {
            self.recent.pop_front();
        }
        if ledger
            .send(output, time, |route, output| {
                runtime.publish_audio_output(route, Some(target), output)
            })
            .is_err()
        {
            runtime.abort_audio_playback(&route);
            return Err(error(
                ErrorCode::Unavailable,
                "Speech output could not start",
            ));
        }
        self.active.insert(
            installation,
            Output {
                source,
                ledger,
                frames: VecDeque::new(),
                eof: false,
                cancelled: false,
            },
        );
        Ok(id)
    }
    pub fn push_frame(&mut self, id: Uuid, frame: &CallPcmFrame) -> Result<(), Failure> {
        let output = self
            .active
            .values_mut()
            .find(|o| o.ledger.playback_id() == id)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "Unknown speech output"))?;
        if output.eof
            || output.ledger.outcome().is_some()
            || output.ledger.sent_frames() as usize + output.frames.len()
                >= MAX_OUTPUT_FRAMES as usize
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Speech output no longer accepts frames",
            ));
        }
        if output.frames.len() >= MAX_BUFFERED_FRAMES {
            return Err(error(ErrorCode::Busy, "Speech output buffer is full"));
        }
        output.frames.push_back(*frame);
        Ok(())
    }
    pub fn finish(&mut self, id: Uuid) -> Result<(), Failure> {
        let output = self
            .active
            .values_mut()
            .find(|o| o.ledger.playback_id() == id)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "Unknown speech output"))?;
        if output.eof || output.ledger.outcome().is_some() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Speech output already finished",
            ));
        }
        output.eof = true;
        Ok(())
    }
    /// Host supplies target/origin/session/process after normal action admission.
    /// No runtime call here: avoid reacquiring its frame lock from the sink.
    pub fn receive(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        session: Uuid,
        generation: u64,
        receipt: &AudioPlaybackReceipt,
        now: Instant,
    ) -> Result<(), Failure> {
        let time = now
            .checked_duration_since(self.epoch)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "Invalid playback clock"))?;
        let output = self
            .active
            .values_mut()
            .find(|o| {
                let route = o.ledger.route();
                route.session == session
                    && route.process_generation == generation
                    && route.target == target
                    && &route.origin == origin
                    && o.ledger.playback_id() == receipt.playback_id()
            })
            .ok_or_else(|| {
                error(
                    ErrorCode::InvalidRequest,
                    "Playback feedback does not match its source",
                )
            })?;
        let route = output.ledger.route().clone();
        output
            .ledger
            .receive(&route, receipt, time)
            .map_err(|_| error(ErrorCode::InvalidRequest, "Playback feedback was rejected"))
    }
    fn retire(&mut self, installation: Uuid, outcome: AudioPlaybackOutcome) {
        if let Some(output) = self.active.remove(&installation) {
            self.outcomes.push_back(SpeechOutputOutcome {
                source: output.source,
                playback_id: output.ledger.playback_id(),
                route: output.ledger.route().clone(),
                outcome,
            });
        }
    }
    /// Explicit source stop never flushes queued PCM. Cancel remains correlated
    /// until terminal feedback, unless authority/clock/transport already failed.
    pub fn cancel(&mut self, runtime: &ExtensionRuntime, id: Uuid, now: Instant) {
        let Some((installation, output)) = self
            .active
            .iter_mut()
            .find(|(_, o)| o.ledger.playback_id() == id)
        else {
            return;
        };
        let installation = *installation;
        if let Some(outcome) = output.ledger.outcome() {
            if outcome == AudioPlaybackOutcome::Failed {
                runtime.abort_audio_playback(output.ledger.route());
            }
            self.retire(installation, outcome);
            return;
        }
        if output.cancelled {
            return;
        }
        output.cancelled = true;
        output.frames.clear();
        output.eof = true;
        let route = output.ledger.route().clone();
        let result = now.checked_duration_since(self.epoch).and_then(|time| {
            output
                .ledger
                .send(
                    AudioOutput::Cancel {
                        playback_id: id,
                        target: route.target,
                    },
                    time,
                    |route, _| runtime.cancel_audio_playback(route, id),
                )
                .ok()
        });
        if result.is_none() {
            runtime.abort_audio_playback(&route);
            self.retire(installation, AudioPlaybackOutcome::Cancelled);
        }
    }
    /// Call outside runtime.frame, even without new receipts or source frames.
    pub fn tick(&mut self, runtime: &ExtensionRuntime, current: Option<Target>, now: Instant) {
        let candidates = current
            .map(|target| runtime.audio_playback_routes(target))
            .unwrap_or_default();
        self.ready
            .retain(|_, (route, _)| candidates.contains(route));
        for route in candidates {
            self.ready
                .entry(route.installation_id)
                .or_insert((route, false));
        }
        let ids: Vec<_> = self.active.keys().copied().collect();
        for installation in ids {
            let output = self
                .active
                .get_mut(&installation)
                .expect("captured active output");
            let route = output.ledger.route().clone();
            if let Some(outcome) = output.ledger.outcome() {
                if outcome == AudioPlaybackOutcome::Failed {
                    runtime.abort_audio_playback(&route);
                }
                self.retire(installation, outcome);
                continue;
            }
            if current != Some(route.target) {
                if !runtime.cancel_audio_playback(&route, output.ledger.playback_id()) {
                    runtime.abort_audio_playback(&route);
                }
                self.retire(installation, AudioPlaybackOutcome::Cancelled);
                continue;
            }
            if runtime
                .audio_playback_route(installation, route.target)
                .as_ref()
                != Some(&route)
            {
                runtime.abort_audio_playback(&route);
                self.retire(installation, AudioPlaybackOutcome::Cancelled);
                continue;
            }
            let Some(time) = now.checked_duration_since(self.epoch) else {
                runtime.abort_audio_playback(&route);
                self.retire(installation, AudioPlaybackOutcome::Failed);
                continue;
            };
            if output.ledger.tick(time).is_err() {
                runtime.abort_audio_playback(&route);
                self.retire(installation, AudioPlaybackOutcome::Failed);
                continue;
            }
            let mut failed = false;
            while output.ledger.has_frame_credit() {
                let Some(frame) = output.frames.front() else {
                    break;
                };
                let message = AudioOutput::Frame {
                    playback_id: output.ledger.playback_id(),
                    target: route.target,
                    frame_sequence: output.ledger.sent_frames(),
                    pcm: frame.to_vec(),
                };
                if output
                    .ledger
                    .send(message, time, |r, m| {
                        runtime.publish_audio_output(r, current, m)
                    })
                    .is_err()
                {
                    failed = true;
                    break;
                }
                output.frames.pop_front();
            }
            if !failed && output.eof && output.frames.is_empty() && output.ledger.ready_for_frames()
            {
                let message = AudioOutput::Finish {
                    playback_id: output.ledger.playback_id(),
                    target: route.target,
                    frame_count: output.ledger.sent_frames(),
                };
                failed = output
                    .ledger
                    .send(message, time, |r, m| {
                        runtime.publish_audio_output(r, current, m)
                    })
                    .is_err();
            }
            if failed {
                runtime.abort_audio_playback(&route);
                self.retire(installation, AudioPlaybackOutcome::Failed);
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        animation::AnimationState,
        behavior::BehaviorController,
        extension_actions::{ActionCatalog, ActionControllers, ExternalMessageSink},
        extension_process::ProcessSpec,
        extension_protocol::Capability,
    };
    use std::{fs, path::PathBuf, time::Duration};
    struct Fixture {
        runtime: ExtensionRuntime,
        root: PathBuf,
        id: Uuid,
        target: Target,
    }
    const OWNED_CHILD_SCRIPT: &str = r#"import json,os,sys
from collections import deque
from pathlib import Path
root=Path(os.environ['WAIFU_EXTENSION_DATA'])
h=json.loads(sys.stdin.readline()); mode=sys.argv[1]
(root/'ready').write_text('ready')
seq=0; pending=None; events=deque(); count=0
for line in sys.stdin:
 r=json.loads(line); m=r['message']
 if m['type']=='event' and m['event']['type']=='target.changed' and m['event']['target'] is not None:
  events.append((m['event']['target'],None))
 elif m['type']=='audio.output':
  o=m['output']; phase=o['phase']; target=o['target']; pid=o['playback_id']
  if phase=='start':
   count=0
   if mode!='silent': events.append((target,dict(phase='ready',playback_id=pid)))
  elif phase=='frame':
   assert o['frame_sequence']==count
   assert len(o['pcm'])==1920 and (mode.startswith('converted') or all(v==123 for v in o['pcm']))
   if mode.startswith('converted'):
    import struct
    with (root/'pcm').open('ab') as pcm: pcm.write(struct.pack('<1920h',*o['pcm']))
   count+=1
   if mode not in ('silent','ready_only'):
    events.append((target,dict(phase='consumed',playback_id=pid,frame_count=99 if mode=='invalid' else count)))
   p=root/'frames.tmp'; p.write_text(str(count)); p.replace(root/'frames')
  elif phase=='finish':
   assert o['frame_count']==count
   if mode=='converted_delay':
    import time
    (root/'finishing').write_text('1'); time.sleep(1)
   events.append((target,dict(phase='finished',playback_id=pid,frame_count=count,outcome='completed')))
  elif phase=='cancel': events.append((target,dict(phase='finished',playback_id=pid,frame_count=count,outcome='cancelled')))
 elif m['type']=='result' and m['id']==pending: pending=None
 if pending is None and events:
  target,receipt=events.popleft(); seq+=1; pending='playback-'+str(seq)
  print(json.dumps(dict(api_version=1,type='action',id=pending,sequence=seq,session_id=h['session_id'],
   process_generation=h['process_generation'],target=target,action=(dict(type='audio.playback.available') if receipt is None else dict(type='audio.playback.receipt',receipt=receipt)))),flush=True)
"#;
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.runtime.shutdown(Duration::from_secs(2));
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            Self::new_with_grants(
                mode,
                [Capability::AudioPlayback, Capability::LifecycleObserve],
            )
        }
        fn new_with_grants(mode: &str, grants: impl IntoIterator<Item = Capability>) -> Self {
            let root = std::env::temp_dir().join(format!("speech-owner-{}", Uuid::new_v4()));
            fs::create_dir_all(root.join("data")).unwrap();
            let fixture = Self {
                runtime: ExtensionRuntime::default(),
                root,
                id: Uuid::new_v4(),
                target: Target {
                    uuid: Uuid::new_v4(),
                    generation: 0,
                },
            };

            fixture
                .runtime
                .start(
                    fixture.id,
                    ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec![
                            "-u".into(),
                            "-c".into(),
                            OWNED_CHILD_SCRIPT.into(),
                            mode.into(),
                        ],
                        working_directory: fixture.root.clone(),
                        data_directory: fixture.root.join("data"),
                        lifetime: Some(Duration::from_secs(12)),
                    },
                    IntegrationOrigin {
                        integration_id: "org.example.speech-fixture".into(),
                        display_name: "Owned PCM fixture".into(),
                    },
                    grants,
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(4);
            while !fixture.root.join("data/ready").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            fixture
        }
        fn add_ready_blocker(&self, outputs: &mut SpeechOutputs) -> Uuid {
            let id = Uuid::new_v4();
            let data = self.root.join("busy-data");
            fs::create_dir_all(&data).unwrap();
            self.runtime
                .start(
                    id,
                    ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec![
                            "-u".into(),
                            "-c".into(),
                            OWNED_CHILD_SCRIPT.into(),
                            "ready_only".into(),
                        ],
                        working_directory: self.root.clone(),
                        data_directory: data,
                        lifetime: Some(Duration::from_secs(12)),
                    },
                    IntegrationOrigin {
                        integration_id: "org.example.busy-fixture".into(),
                        display_name: "Owned blocked output".into(),
                    },
                    [Capability::AudioPlayback, Capability::LifecycleObserve],
                )
                .unwrap();
            self.until(outputs, |o| o.is_ready(id, self.target));
            id
        }
        fn frames(&self) -> usize {
            fs::read_to_string(self.root.join("data/frames"))
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        }
        fn frame(&self, outputs: &mut SpeechOutputs) {
            struct Sink<'a>(&'a mut SpeechOutputs);
            impl ExternalMessageSink for Sink<'_> {
                fn audio_playback_available(
                    &mut self,
                    target: Target,
                    origin: &IntegrationOrigin,
                    session: Uuid,
                    generation: u64,
                ) -> Result<(), Failure> {
                    self.0
                        .receive_available(target, origin, session, generation)
                }

                fn deliver(
                    &mut self,
                    _: Target,
                    _: &IntegrationOrigin,
                    _: &str,
                ) -> Result<Uuid, Failure> {
                    panic!("receipt became text")
                }
                fn audio_playback_receipt(
                    &mut self,
                    target: Target,
                    origin: &IntegrationOrigin,
                    session: Uuid,
                    generation: u64,
                    receipt: &AudioPlaybackReceipt,
                ) -> Result<(), Failure> {
                    self.0
                        .receive(target, origin, session, generation, receipt, Instant::now())
                }
            }
            outputs.tick(&self.runtime, Some(self.target), Instant::now());
            self.runtime.frame(
                &[],
                Some(self.target),
                false,
                &ActionCatalog::from_packs([], &[]),
                ActionControllers {
                    animation: &mut AnimationState::default(),
                    behavior: &mut BehaviorController::default(),
                    external_messages: Some(&mut Sink(outputs)),
                },
            );
            outputs.tick(&self.runtime, Some(self.target), Instant::now());
        }
        fn prepare(&self, outputs: &mut SpeechOutputs) {
            self.until(outputs, |o| o.is_ready(self.id, self.target));
        }
        fn until(&self, outputs: &mut SpeechOutputs, done: impl Fn(&SpeechOutputs) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                self.frame(outputs);
                if done(outputs) {
                    break;
                }
                assert!(Instant::now() < deadline, "{:?}", self.runtime.views());
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    fn source_wave(samples: usize, value: i16) -> bevy::audio::AudioSource {
        source_wave_rate(samples, value, 8000)
    }
    fn source_wave_rate(samples: usize, value: i16, rate: u32) -> bevy::audio::AudioSource {
        let size = (samples * 2) as u32;
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
        for _ in 0..samples {
            bytes.extend(value.to_le_bytes());
        }
        bevy::audio::AudioSource {
            bytes: bytes.into(),
        }
    }
    fn expected_pcm(samples: &[f32], speed: f32) -> Vec<u8> {
        expected_pcm_rate(samples, speed, 8000)
    }
    fn expected_pcm_rate(samples: &[f32], speed: f32, rate: u32) -> Vec<u8> {
        let mut converter = crate::call_pcm::CallPcm::with_speed(rate, 1, speed).unwrap();
        let mut frames = Vec::new();
        for part in samples.chunks(4096) {
            frames.extend(
                converter
                    .push(&crate::metered_audio::SpeechPcmChunk {
                        sample_rate: rate,
                        channels: 1,
                        samples: part.to_vec(),
                    })
                    .unwrap(),
            );
        }
        frames.extend(converter.finish());
        frames
            .iter()
            .flat_map(|f| f.iter().flat_map(|s| s.to_le_bytes()))
            .collect()
    }
    fn route_until(
        f: &Fixture,
        outputs: &mut SpeechOutputs,
        routing: &mut crate::speech_pcm_routing::SpeechPcmRouting,
        done: impl Fn(&SpeechOutputs, &crate::speech_pcm_routing::SpeechPcmRouting) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            routing.tick(outputs, &f.runtime, Some(f.target), Instant::now());
            f.frame(outputs);
            if done(outputs, routing) {
                routing.tick(outputs, &f.runtime, Some(f.target), Instant::now());
                break;
            }
            assert!(Instant::now() < deadline, "{:?}", f.runtime.views());
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn readiness_snapshot_rechecks_authority_without_waiting_for_owner_tick() {
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        assert_eq!(
            outputs
                .current_ready_routes_for_character(&f.runtime, f.target.uuid)
                .len(),
            1
        );
        assert!(
            outputs
                .current_ready_routes_for_character(&f.runtime, Uuid::new_v4())
                .is_empty()
        );
        let replacement = Target {
            uuid: f.target.uuid,
            generation: f.target.generation + 1,
        };
        f.runtime.frame(
            &[],
            Some(replacement),
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                animation: &mut AnimationState::default(),
                behavior: &mut BehaviorController::default(),
                external_messages: None,
            },
        );
        assert_eq!(outputs.ready_routes_for_character(f.target.uuid).len(), 1);
        assert!(
            outputs
                .current_ready_routes_for_character(&f.runtime, f.target.uuid)
                .is_empty()
        );
        f.runtime.stop(f.id);
        assert_eq!(outputs.ready_routes_for_character(f.target.uuid).len(), 1); // stale cache
        assert!(
            outputs
                .current_ready_routes_for_character(&f.runtime, f.target.uuid)
                .is_empty()
        );
    }
    #[test]
    fn shared_pcm_single_decoder_routes_completed_sentences_fifo_and_matches_bytes() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let routes = outputs.ready_routes(f.target);
        let first = routing.buffered(
            &meter,
            source_wave(800, 16384),
            None,
            routes.clone(),
            1.0,
            Instant::now(),
        );
        let local_first: Vec<_> = first.decoder().collect();
        assert_eq!(first.decoder().next(), None);
        let second = routing.buffered(
            &meter,
            source_wave(400, -8192),
            None,
            routes,
            1.0,
            Instant::now(),
        );
        let local_second: Vec<_> = second.decoder().collect();
        assert_eq!(routing.pending_sentences(), 2);
        route_until(&f, &mut outputs, &mut routing, |o, r| {
            o.active_count() == 0 && r.pending_sentences() == 0
        });
        let mut expected = expected_pcm(&local_first, 1.0);
        expected.extend(expected_pcm(&local_second, 1.0));
        assert_eq!(fs::read(f.root.join("data/pcm")).unwrap(), expected);
        assert!(outputs.take_outcome().is_none());
        assert!(routing.take_notice().is_none());
    }
    #[test]
    fn shared_pcm_remote_finish_wait_does_not_starve_next_native_tap() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted_delay");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let routes = outputs.ready_routes(f.target);
        let first = routing.buffered(
            &meter,
            source_wave(80, 1000),
            None,
            routes.clone(),
            1.0,
            Instant::now(),
        );
        first.decoder().for_each(drop);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !f.root.join("data/finishing").exists() {
            routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
            f.frame(&mut outputs);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let second = routing.buffered(
            &meter,
            source_wave_rate(40_000, 2000, 48_000),
            None,
            routes,
            1.0,
            Instant::now(),
        );
        let mut decoder = second.decoder();
        // More than the tap's 32768 samples, while first Finish is unacknowledged.
        // Bounded converted frames fit in the pending queue during this delay.
        for _ in 0..5 {
            assert_eq!(decoder.by_ref().take(8000).count(), 8000);
            routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
            f.frame(&mut outputs);
        }
        assert_eq!(decoder.next(), None);
        route_until(&f, &mut outputs, &mut routing, |o, r| {
            o.active_count() == 0 && r.pending_sentences() == 0
        });
        assert!(routing.take_notice().is_none());
        let mut expected = expected_pcm(&vec![1000.0 / 32768.0; 80], 1.0);
        expected.extend(
            (0..40_000)
                .flat_map(|_| [2000i16, 2000i16])
                .flat_map(i16::to_le_bytes),
        );
        // 40000 mono frames need silence padding to a complete 20 ms packet.
        expected.extend(vec![0; (960 - 40_000 % 960) * 4]);
        assert_eq!(fs::read(f.root.join("data/pcm")).unwrap(), expected);
    }
    #[test]
    fn shared_pcm_queued_source_gets_start_budget_when_it_reaches_front() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let t = Instant::now();
        let routes = outputs.ready_routes(f.target);
        let first = routing.buffered(&meter, source_wave(80, 1000), None, routes.clone(), 1.0, t);
        first.decoder().for_each(drop);
        let second = routing.buffered(&meter, source_wave(80, 2000), None, routes, 1.0, t);
        routing.tick(&mut outputs, &f.runtime, Some(f.target), t);
        routing.tick(
            &mut outputs,
            &f.runtime,
            Some(f.target),
            t + Duration::from_secs(6),
        );
        assert_eq!(routing.pending_sentences(), 2);
        assert!(routing.take_notice().is_none());
        // Waiting has not claimed or cancelled its ordinary native decoder.
        assert_eq!(second.decoder().count(), 80);
        meter.stop();
        routing.tick(
            &mut outputs,
            &f.runtime,
            Some(f.target),
            t + Duration::from_secs(6),
        );
        assert_eq!(routing.pending_sentences(), 0);
    }
    #[test]
    fn shared_pcm_background_clock_routes_without_local_device_or_second_decoder() {
        let _guard = crate::speech_clock::TEST_CLOCK_LOCK.lock().unwrap();
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let asset = routing.buffered(
            &meter,
            source_wave(1600, 8192),
            None,
            outputs.ready_routes(f.target),
            1.33,
            Instant::now(),
        );
        let receipt = asset.playback();
        assert!(routing.has_playback(&receipt));
        let settlement = routing.playback_receipt(&receipt).unwrap();
        let clock = crate::speech_clock::SpeechClock::start(asset, 1.33).unwrap();
        route_until(&f, &mut outputs, &mut routing, |o, r| {
            clock.is_finished() && o.active_count() == 0 && r.pending_sentences() == 0
        });
        assert!(receipt.has_started() && receipt.is_terminal());
        assert_eq!(
            fs::read(f.root.join("data/pcm")).unwrap(),
            expected_pcm(&vec![0.25; 1600], 1.33)
        );
        assert!(routing.take_notice().is_none());
        assert!(outputs.take_outcome().is_none());
        assert!(!routing.has_playback(&receipt));
        assert_eq!(
            settlement.outcome(),
            crate::speech_pcm_routing::SpeechRouteOutcome::Completed
        );
    }
    #[test]
    fn shared_pcm_settlement_keeps_healthy_route_and_reports_partial_loss() {
        use crate::speech_pcm_routing::SpeechRouteOutcome;
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routes = outputs.ready_routes(f.target);
        let mut lost = routes[0].clone();
        lost.installation_id = Uuid::new_v4();
        routes.push(lost);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let source = routing.buffered(
            &meter,
            source_wave(800, 8192),
            None,
            routes,
            1.0,
            Instant::now(),
        );
        let settlement = routing.playback_receipt(&source.playback()).unwrap();
        let local: Vec<_> = source.decoder().collect();
        route_until(&f, &mut outputs, &mut routing, |o, r| {
            o.active_count() == 0 && r.pending_sentences() == 0
        });
        assert_eq!(settlement.outcome(), SpeechRouteOutcome::Partial);
        assert_eq!(
            fs::read(f.root.join("data/pcm")).unwrap(),
            expected_pcm(&local, 1.0)
        );
        assert!(outputs.take_outcome().is_none());
    }
    #[test]
    fn shared_pcm_busy_destination_cannot_starve_healthy_call() {
        use crate::speech_pcm_routing::SpeechRouteOutcome;
        let _guard = crate::speech_clock::TEST_CLOCK_LOCK.lock().unwrap();
        for samples in [9600, 96_000] {
            let f = Fixture::new("converted");
            let mut outputs = SpeechOutputs::default();
            f.prepare(&mut outputs);
            let blocked = f.add_ready_blocker(&mut outputs);
            let held = outputs
                .begin(
                    &f.runtime,
                    blocked,
                    f.target,
                    Uuid::new_v4(),
                    Instant::now(),
                )
                .unwrap();
            let routes = outputs.ready_routes(f.target);
            assert_eq!(routes.len(), 2);
            let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
            let meter = crate::metered_audio::SpeechMeter::default();
            let source = routing.buffered(
                &meter,
                source_wave_rate(samples, 8192, 48_000),
                None,
                routes,
                1.0,
                Instant::now(),
            );
            let settlement = routing.playback_receipt(&source.playback()).unwrap();
            let clock = crate::speech_clock::SpeechClock::start(source, 1.0).unwrap();
            let early = Instant::now() + Duration::from_secs(1);
            while f.frames() < 10 {
                routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
                f.frame(&mut outputs);
                assert!(
                    Instant::now() < early,
                    "Healthy destination was blocked by Busy peer"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            route_until(&f, &mut outputs, &mut routing, |_, r| {
                clock.is_finished() && r.pending_sentences() == 0
            });
            assert_eq!(settlement.outcome(), SpeechRouteOutcome::Partial);
            assert_eq!(
                fs::read(f.root.join("data/pcm")).unwrap(),
                expected_pcm_rate(&vec![0.25; samples], 1.0, 48_000)
            );
            outputs.cancel(&f.runtime, held, Instant::now());
            f.until(&mut outputs, |o| o.active_count() == 0);
            while outputs.take_outcome().is_some() {}
        }
    }
    #[test]
    fn shared_pcm_source_failure_settles_separately_from_call_route_loss() {
        use crate::speech_pcm_routing::SpeechRouteOutcome;
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let source = routing.buffered(
            &meter,
            source_wave(800, 8192),
            None,
            outputs.ready_routes(f.target),
            1.0,
            Instant::now(),
        );
        let settlement = routing.playback_receipt(&source.playback()).unwrap();
        assert_eq!(source.decoder().take(10).count(), 10); // abandoned prefix
        routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
        assert_eq!(settlement.outcome(), SpeechRouteOutcome::SourceFailed);
        assert_eq!(outputs.active_count(), 0);
    }
    #[test]
    fn shared_streaming_pcm_routes_once_and_failed_production_cannot_flush_as_eof() {
        use bevy::audio::Decodable;
        use std::sync::{Arc, atomic::AtomicU64};
        for failed in [false, true] {
            let f = Fixture::new("converted");
            let mut outputs = SpeechOutputs::default();
            f.prepare(&mut outputs);
            let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
            let meter = crate::metered_audio::SpeechMeter::default();
            let wave = source_wave(800, 16384);
            let bytes = &wave.bytes[..if failed { 44 + 800 } else { wave.bytes.len() }];
            let mut voice = None;
            let result = crate::voice_stream::receive_wav(
                bytes,
                Arc::new(AtomicU64::new(1)),
                1,
                Instant::now() + Duration::from_secs(1),
                |v| voice = Some(v),
            );
            assert_eq!(result.is_err(), failed);
            let voice = voice.unwrap();
            assert_eq!(voice.is_failed(), failed);
            let asset = routing.streaming(
                &meter,
                voice,
                None,
                outputs.ready_routes(f.target),
                1.0,
                Instant::now(),
            );
            let settlement = routing.playback_receipt(&asset.playback()).unwrap();
            let local: Vec<_> = asset.decoder().collect();
            assert_eq!(asset.decoder().next(), None);
            route_until(&f, &mut outputs, &mut routing, |o, r| {
                o.active_count() == 0 && r.pending_sentences() == 0
            });
            if failed {
                assert_eq!(f.frames(), 0);
                assert!(!f.root.join("data/pcm").exists());
            } else {
                assert_eq!(
                    fs::read(f.root.join("data/pcm")).unwrap(),
                    expected_pcm(&local, 1.0)
                );
            }
            assert_eq!(
                settlement.outcome(),
                if failed {
                    crate::speech_pcm_routing::SpeechRouteOutcome::ProviderFailed
                } else {
                    crate::speech_pcm_routing::SpeechRouteOutcome::Completed
                }
            );
        }
    }
    #[test]
    fn shared_pcm_routing_uses_captured_local_playback_speed() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let asset = routing.buffered(
            &meter,
            source_wave(800, 16384),
            None,
            outputs.ready_routes(f.target),
            1.33,
            Instant::now(),
        );
        let local: Vec<_> = asset.decoder().collect();
        route_until(&f, &mut outputs, &mut routing, |o, r| {
            o.active_count() == 0 && r.pending_sentences() == 0
        });
        assert_eq!(
            fs::read(f.root.join("data/pcm")).unwrap(),
            expected_pcm(&local, 1.33)
        );
    }
    #[test]
    fn shared_pcm_abandonment_never_flushes_and_optional_timeout_preserves_local_source() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let asset = routing.buffered(
            &meter,
            source_wave(800, 16384),
            None,
            outputs.ready_routes(f.target),
            1.0,
            Instant::now(),
        );
        assert_eq!(asset.decoder().take(100).count(), 100);
        routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
        assert_eq!(routing.pending_sentences(), 0);
        assert_eq!(outputs.active_count(), 0);
        assert_eq!(f.frames(), 0);
        let t = Instant::now();
        let asset = routing.buffered(
            &meter,
            source_wave(800, 16384),
            None,
            outputs.ready_routes(f.target),
            1.0,
            t,
        );
        let settlement = routing.playback_receipt(&asset.playback()).unwrap();
        routing.tick(
            &mut outputs,
            &f.runtime,
            Some(f.target),
            t + Duration::from_secs(5),
        );
        assert_eq!(routing.pending_sentences(), 0);
        assert_eq!(
            settlement.outcome(),
            crate::speech_pcm_routing::SpeechRouteOutcome::SourceFailed
        );
        assert_eq!(asset.decoder().count(), 800);
        assert_eq!(f.frames(), 0);
    }
    #[test]
    fn shared_pcm_stop_cancels_prefix_without_flushing_more_frames_or_leaking_outcomes() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let asset = routing.buffered(
            &meter,
            source_wave(8000, 16384),
            None,
            outputs.ready_routes(f.target),
            1.0,
            Instant::now(),
        );
        let mut decoder = asset.decoder();
        for _ in 0..1200 {
            assert!(decoder.next().is_some());
        }
        route_until(&f, &mut outputs, &mut routing, |o, _| {
            o.active.values().any(|o| o.ledger.sent_frames() > 0)
        });
        let sent = outputs
            .active
            .values()
            .map(|o| o.ledger.sent_frames() as usize)
            .sum::<usize>();
        meter.stop();
        route_until(&f, &mut outputs, &mut routing, |o, r| {
            o.active_count() == 0 && r.pending_sentences() == 0
        });
        assert_eq!(f.frames(), sent);
        assert!(outputs.take_outcome().is_none());
        assert_eq!(decoder.next(), None);
    }
    #[test]
    fn shared_pcm_captured_route_cannot_replay_speech_into_a_replacement() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let old = f.runtime.audio_playback_route(f.id, f.target).unwrap();
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let asset = routing.buffered(
            &meter,
            source_wave(800, 16384),
            None,
            outputs.ready_routes(f.target),
            1.0,
            Instant::now(),
        );
        asset.decoder().count();
        f.runtime.restart(f.id);
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            f.frame(&mut outputs);
            if f.runtime
                .audio_playback_route(f.id, f.target)
                .is_some_and(|r| r.session != old.session)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        f.prepare(&mut outputs);
        routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
        assert_eq!(routing.pending_sentences(), 0);
        assert_eq!(outputs.active_count(), 0);
        assert_eq!(f.frames(), 0);
    }
    #[test]
    fn shared_pcm_sentence_budget_falls_back_without_breaking_local_decoder() {
        use bevy::audio::Decodable;
        let f = Fixture::new("converted");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let mut routing = crate::speech_pcm_routing::SpeechPcmRouting::default();
        let meter = crate::metered_audio::SpeechMeter::default();
        let mut assets = Vec::new();
        for _ in 0..8 {
            assets.push(routing.buffered(
                &meter,
                source_wave(80, 16384),
                None,
                outputs.ready_routes(f.target),
                1.0,
                Instant::now(),
            ));
        }
        let fallback = routing.buffered(
            &meter,
            source_wave(80, 16384),
            None,
            outputs.ready_routes(f.target),
            1.0,
            Instant::now(),
        );
        assert_eq!(routing.pending_sentences(), 8);
        assert!(routing.take_notice().is_some());
        assert_eq!(fallback.decoder().count(), 80);
        routing.tick(&mut outputs, &f.runtime, Some(f.target), Instant::now());
        assert_eq!(routing.pending_sentences(), 0);
        assert_eq!(outputs.active_count(), 0);
    }
    #[test]
    fn output_only_grant_receives_private_target_control_without_lifecycle_subscription() {
        let f = Fixture::new_with_grants("normal", [Capability::AudioPlayback]);
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let id = outputs
            .begin(&f.runtime, f.id, f.target, Uuid::new_v4(), Instant::now())
            .unwrap();
        outputs.push_frame(id, &[123; 1920]).unwrap();
        outputs.finish(id).unwrap();
        f.until(&mut outputs, |o| o.active_count() == 0);
        assert_eq!(
            outputs.take_outcome().unwrap().outcome,
            AudioPlaybackOutcome::Completed
        );
        assert_eq!(f.frames(), 1);
    }
    #[test]
    fn owner_requires_current_call_readiness_and_drops_it_after_authority_loss() {
        let f = Fixture::new("normal");
        let mut outputs = SpeechOutputs::default();
        assert_eq!(
            outputs
                .begin(&f.runtime, f.id, f.target, Uuid::new_v4(), Instant::now())
                .unwrap_err()
                .0,
            ErrorCode::Unavailable
        );
        outputs.tick(&f.runtime, Some(f.target), Instant::now());
        let route = f.runtime.audio_playback_route(f.id, f.target).unwrap();
        assert!(
            outputs
                .receive_available(
                    route.target,
                    &route.origin,
                    Uuid::new_v4(),
                    route.process_generation
                )
                .is_err()
        );
        assert!(!outputs.is_ready(f.id, f.target));
        f.prepare(&mut outputs);
        assert!(outputs.is_ready(f.id, f.target));
        f.runtime.stop(f.id);
        outputs.tick(&f.runtime, Some(f.target), Instant::now());
        assert!(!outputs.is_ready(f.id, f.target));
        assert!(
            outputs
                .receive_available(
                    route.target,
                    &route.origin,
                    route.session,
                    route.process_generation
                )
                .is_err()
        );
        assert!(outputs.ready_routes(f.target).is_empty());
    }
    #[test]
    fn owner_actual_child_feedback_drives_pcm_and_terminal_outcome_without_runtime_deadlock() {
        let f = Fixture::new("normal");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let source = Uuid::new_v4();
        let id = outputs
            .begin(&f.runtime, f.id, f.target, source, Instant::now())
            .unwrap();
        for _ in 0..9 {
            outputs.push_frame(id, &[123; 1920]).unwrap();
        }
        outputs.finish(id).unwrap();
        f.until(&mut outputs, |o| o.active_count() == 0);
        let done = outputs.take_outcome().unwrap();
        assert_eq!(done.playback_id, id);
        assert_eq!(done.source, source);
        assert_eq!(done.outcome, AudioPlaybackOutcome::Completed);
        assert_eq!(f.frames(), 9);
        assert!(outputs.take_outcome().is_none());
        assert!(f.runtime.audio_playback_route(f.id, f.target).is_some());
    }
    #[test]
    fn owner_four_frame_window_and_local_buffer_are_bounded_without_dropping_source() {
        let f = Fixture::new("ready_only");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let id = outputs
            .begin(&f.runtime, f.id, f.target, Uuid::new_v4(), Instant::now())
            .unwrap();
        for _ in 0..64 {
            outputs.push_frame(id, &[123; 1920]).unwrap();
        }
        assert_eq!(
            outputs.push_frame(id, &[123; 1920]).unwrap_err().0,
            ErrorCode::Busy
        );
        f.until(&mut outputs, |o| {
            o.active[&f.id].ledger.sent_frames() == 4 && f.frames() == 4
        });
        for _ in 0..8 {
            f.frame(&mut outputs);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(f.frames(), 4);
        assert_eq!(outputs.buffered_frames(), 60);
        outputs.finish(id).unwrap();
        assert_eq!(
            outputs.push_frame(id, &[123; 1920]).unwrap_err().0,
            ErrorCode::InvalidRequest
        );
    }
    #[test]
    fn owner_invalid_correlated_feedback_aborts_original_process() {
        let f = Fixture::new("invalid");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let id = outputs
            .begin(&f.runtime, f.id, f.target, Uuid::new_v4(), Instant::now())
            .unwrap();
        outputs.push_frame(id, &[123; 1920]).unwrap();
        f.until(&mut outputs, |o| o.active_count() == 0);
        assert_eq!(
            outputs.take_outcome().unwrap().outcome,
            AudioPlaybackOutcome::Failed
        );
        assert!(f.runtime.audio_playback_route(f.id, f.target).is_none());
    }
    #[test]
    fn owner_cancel_is_idempotent_discards_pcm_and_prevents_source_replay() {
        let f = Fixture::new("normal");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let source = Uuid::new_v4();
        let id = outputs
            .begin(&f.runtime, f.id, f.target, source, Instant::now())
            .unwrap();
        outputs.push_frame(id, &[123; 1920]).unwrap();
        outputs.cancel(&f.runtime, id, Instant::now());
        outputs.cancel(&f.runtime, id, Instant::now());
        assert_eq!(outputs.buffered_frames(), 0);
        f.until(&mut outputs, |o| o.active_count() == 0);
        assert_eq!(
            outputs.take_outcome().unwrap().outcome,
            AudioPlaybackOutcome::Cancelled
        );
        assert_eq!(f.frames(), 0);
        assert_eq!(
            outputs
                .begin(&f.runtime, f.id, f.target, source, Instant::now())
                .unwrap_err()
                .0,
            ErrorCode::InvalidRequest
        );
        assert!(f.runtime.audio_playback_route(f.id, f.target).is_some());
    }
    #[test]
    fn owner_authority_and_target_loss_cancel_without_stopping_replacement() {
        let f = Fixture::new("normal");
        let mut outputs = SpeechOutputs::default();
        f.prepare(&mut outputs);
        let id = outputs
            .begin(&f.runtime, f.id, f.target, Uuid::new_v4(), Instant::now())
            .unwrap();
        let old = outputs.active[&f.id].ledger.route().clone();
        outputs.tick(&f.runtime, None, Instant::now());
        assert_eq!(
            outputs.take_outcome().unwrap().outcome,
            AudioPlaybackOutcome::Cancelled
        );
        assert_eq!(
            outputs.push_frame(id, &[123; 1920]).unwrap_err().0,
            ErrorCode::InvalidRequest
        );
        assert!(f.runtime.audio_playback_route(f.id, f.target).is_some());
        f.runtime.restart(f.id);
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            f.frame(&mut outputs);
            if f.runtime
                .audio_playback_route(f.id, f.target)
                .is_some_and(|r| r.session != old.session)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!f.runtime.abort_audio_playback(&old));
        assert!(f.runtime.audio_playback_route(f.id, f.target).is_some());
    }
    #[test]
    fn owner_foreign_feedback_cannot_release_credit_and_stall_aborts() {
        let f = Fixture::new("silent");
        let t = Instant::now();
        let mut outputs = SpeechOutputs::new(t);
        f.prepare(&mut outputs);
        let id = outputs
            .begin(&f.runtime, f.id, f.target, Uuid::new_v4(), t)
            .unwrap();
        let route = outputs.active[&f.id].ledger.route().clone();
        assert!(
            outputs
                .receive(
                    route.target,
                    &route.origin,
                    Uuid::new_v4(),
                    route.process_generation,
                    &AudioPlaybackReceipt::Ready { playback_id: id },
                    t
                )
                .is_err()
        );
        assert!(!outputs.active[&f.id].ledger.ready_for_frames());
        outputs.tick(&f.runtime, Some(f.target), t + Duration::from_secs(5));
        assert_eq!(
            outputs.take_outcome().unwrap().outcome,
            AudioPlaybackOutcome::Failed
        );
        assert!(f.runtime.audio_playback_route(f.id, f.target).is_none());
    }
}
