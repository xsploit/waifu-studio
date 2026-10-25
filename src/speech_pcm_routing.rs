//! Bounded sentence FIFO from the existing single claimed decoder to ready
//! output routes. No provider requests, decoder creation or audio-thread locks.
use crate::{
    call_pcm::{CallPcm, CallPcmFrame},
    extension_protocol::{AudioPlaybackOutcome, ErrorCode, Target},
    extension_runtime::{AudioPlaybackRoute, ExtensionRuntime},
    extension_speech_output::SpeechOutputs,
    metered_audio::{MeteredSpeech, SpeechMeter, SpeechPcm, SpeechPcmState, SpeechPlayback},
    viseme_audio::VisemeTap,
    voice_stream::StreamingVoice,
};
use bevy::{audio::AudioSource, prelude::Resource};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
const MAX_SENTENCES: usize = 8;
const MAX_PENDING: usize = 64;
const START_WAIT: Duration = Duration::from_secs(5);
/// Routing settlement only, never a claim of remote audibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechRouteOutcome {
    Active,
    Completed,
    Partial,
    Stopped,
    SourceFailed,
    ProviderFailed,
}
#[derive(Clone)]
pub struct SpeechRouteReceipt(Arc<AtomicU8>);
impl SpeechRouteReceipt {
    pub fn outcome(&self) -> SpeechRouteOutcome {
        match self.0.load(Ordering::Acquire) {
            0 => SpeechRouteOutcome::Active,
            1 => SpeechRouteOutcome::Completed,
            2 => SpeechRouteOutcome::Partial,
            4 => SpeechRouteOutcome::SourceFailed,
            5 => SpeechRouteOutcome::ProviderFailed,
            _ => SpeechRouteOutcome::Stopped,
        }
    }
}
struct Destination {
    route: AudioPlaybackRoute,
    playback: Option<Uuid>,
    done: bool,
    finishing: bool,
    outcome: Option<AudioPlaybackOutcome>,
}
struct Frame {
    pcm: CallPcmFrame,
    delivered: u8,
}
struct Sentence {
    source: Uuid,
    playback: SpeechPlayback,
    settlement: SpeechRouteReceipt,
    target: Target,
    tap: SpeechPcm,
    stream: Option<StreamingVoice>,
    speed: f32,
    created: Instant,
    started: bool,
    source_stalled: bool,
    waiting: Option<Instant>,
    last_input: Instant,
    destinations: Vec<Destination>,
    converter: Option<CallPcm>,
    pending: VecDeque<Frame>,
    flushed: bool,
    drained: u64,
}
impl Drop for Sentence {
    fn drop(&mut self) {
        let _ = self
            .settlement
            .0
            .compare_exchange(0, 3, Ordering::AcqRel, Ordering::Acquire);
    }
}
#[derive(Resource, Default)]
pub struct SpeechPcmRouting {
    queue: VecDeque<Sentence>,
    awaiting: Vec<Sentence>,
    retired: Vec<Uuid>,
    notice: Option<&'static str>,
}
impl SpeechPcmRouting {
    pub fn playback_receipt(&self, playback: &SpeechPlayback) -> Option<SpeechRouteReceipt> {
        self.queue
            .iter()
            .chain(&self.awaiting)
            .find(|sentence| sentence.playback.same_source(playback))
            .map(|sentence| sentence.settlement.clone())
    }
    /// Includes remote-finish waits, excludes cancelled/retired sources.
    pub fn has_playback(&self, playback: &SpeechPlayback) -> bool {
        self.queue
            .iter()
            .chain(&self.awaiting)
            .any(|sentence| sentence.playback.same_source(playback))
    }

    pub fn pending_sentences(&self) -> usize {
        self.queue.len() + self.awaiting.len()
    }
    pub fn take_notice(&mut self) -> Option<&'static str> {
        self.notice.take()
    }
    fn can_attach(&mut self, routes: &[AudioPlaybackRoute]) -> bool {
        if routes.is_empty() {
            return false;
        }
        if self.pending_sentences() >= MAX_SENTENCES
            || routes.len() > 4
            || self.retired.len() >= MAX_SENTENCES
        {
            self.notice = Some("Call audio queue is full.");
            return false;
        }
        let target = routes[0].target;
        routes.iter().enumerate().all(|(i, r)| {
            r.target == target
                && !routes[..i]
                    .iter()
                    .any(|a| a.installation_id == r.installation_id)
        })
    }
    fn attach(
        &mut self,
        tap: SpeechPcm,
        playback: SpeechPlayback,
        routes: Vec<AudioPlaybackRoute>,
        speed: f32,
        now: Instant,
        stream: Option<StreamingVoice>,
    ) {
        self.queue.push_back(Sentence {
            source: Uuid::new_v4(),
            playback,
            settlement: SpeechRouteReceipt(Arc::new(AtomicU8::new(0))),
            target: routes[0].target,
            tap,
            stream,
            speed,
            created: now,
            started: self.queue.is_empty(),
            source_stalled: false,
            waiting: None,
            last_input: now,
            converter: None,
            pending: VecDeque::new(),
            flushed: false,
            drained: 0,
            destinations: routes
                .into_iter()
                .map(|route| Destination {
                    route,
                    playback: None,
                    done: false,
                    finishing: false,
                    outcome: None,
                })
                .collect(),
        });
    }
    pub fn buffered(
        &mut self,
        meter: &SpeechMeter,
        source: AudioSource,
        visemes: Option<VisemeTap>,
        routes: Vec<AudioPlaybackRoute>,
        speed: f32,
        now: Instant,
    ) -> MeteredSpeech {
        if !self.can_attach(&routes) {
            return match visemes {
                Some(v) => meter.buffered_with_visemes(source, v),
                None => meter.buffered(source),
            };
        }
        let (asset, tap) = meter.buffered_routed(source, visemes);
        self.attach(tap, asset.playback(), routes, speed, now, None);
        asset
    }
    pub fn streaming(
        &mut self,
        meter: &SpeechMeter,
        source: StreamingVoice,
        visemes: Option<VisemeTap>,
        routes: Vec<AudioPlaybackRoute>,
        speed: f32,
        now: Instant,
    ) -> MeteredSpeech {
        if !self.can_attach(&routes) {
            return match visemes {
                Some(v) => meter.streaming_with_visemes(source, v),
                None => meter.streaming(source),
            };
        }
        let status = source.clone();
        let (asset, tap) = meter.streaming_routed(source, visemes);
        self.attach(tap, asset.playback(), routes, speed, now, Some(status));
        asset
    }
    fn cancel_sentence(
        &mut self,
        sentence: Sentence,
        outputs: &mut SpeechOutputs,
        runtime: &ExtensionRuntime,
        now: Instant,
    ) {
        let state = sentence.tap.state();
        let provider_failed = sentence
            .stream
            .as_ref()
            .is_some_and(StreamingVoice::is_failed);
        let source_failed = sentence.source_stalled
            || matches!(state, Err(error) if error != crate::metered_audio::SpeechPcmError::Overflow)
            || state == Ok(SpeechPcmState::Abandoned);
        let natural = sentence.flushed
            && state == Ok(SpeechPcmState::Completed)
            && !sentence
                .stream
                .as_ref()
                .is_some_and(StreamingVoice::is_failed);
        let completed = sentence
            .destinations
            .iter()
            .filter(|destination| destination.outcome == Some(AudioPlaybackOutcome::Completed))
            .count();
        let empty = sentence.converter.is_none() && sentence.flushed;
        let outcome = if provider_failed {
            5
        } else if source_failed {
            4
        } else if natural && (empty || completed == sentence.destinations.len()) {
            1
        } else if natural && completed > 0 {
            2
        } else {
            3
        };
        sentence.settlement.0.store(outcome, Ordering::Release);
        if outcome == 2 {
            self.notice = Some("Some call outputs stopped before speech completed.");
        }
        for destination in &sentence.destinations {
            if let Some(id) = destination.playback {
                outputs.cancel(runtime, id, now);
            }
        }
        if outputs.has_source(sentence.source) {
            self.retired.push(sentence.source);
        }
        // Already-terminal outcomes still need draining when cancel retired it synchronously.
        while outputs.take_source_outcome(sentence.source).is_some() {}
    }
    /// Before owner.tick so source cancellation clears held PCM before any new send.
    pub fn tick(
        &mut self,
        outputs: &mut SpeechOutputs,
        runtime: &ExtensionRuntime,
        current: Option<Target>,
        now: Instant,
    ) {
        self.retired.retain(|source| {
            while outputs.take_source_outcome(*source).is_some() {}
            outputs.has_source(*source)
        });
        let awaiting = std::mem::take(&mut self.awaiting);
        for mut sentence in awaiting {
            Self::receive_outcomes(&mut sentence, outputs, &mut self.notice);
            let state = sentence.tap.state();
            if current != Some(sentence.target)
                || state.is_err()
                || state == Ok(SpeechPcmState::Abandoned)
                || sentence
                    .stream
                    .as_ref()
                    .is_some_and(StreamingVoice::is_failed)
                || sentence.destinations.iter().all(|d| d.done)
            {
                self.cancel_sentence(sentence, outputs, runtime, now);
            } else {
                self.awaiting.push(sentence);
            }
        }
        let mut kept = VecDeque::new();
        while let Some(sentence) = self.queue.pop_front() {
            let state = sentence.tap.state();
            if current != Some(sentence.target)
                || sentence
                    .stream
                    .as_ref()
                    .is_some_and(StreamingVoice::is_failed)
                || state.is_err()
                || state == Ok(SpeechPcmState::Abandoned)
            {
                if matches!(state, Err(crate::metered_audio::SpeechPcmError::Overflow)) {
                    self.notice = Some("Call audio fell behind.");
                }
                self.cancel_sentence(sentence, outputs, runtime, now);
            } else {
                kept.push_back(sentence);
            }
        }
        self.queue = kept;
        let Some(mut sentence) = self.queue.pop_front() else {
            return;
        };
        if !sentence.started {
            sentence.started = true;
            sentence.created = now;
            sentence.last_input = now;
        }
        Self::receive_outcomes(&mut sentence, outputs, &mut self.notice);
        let mut result = Ok(());
        // At most the tap's full bounded queue per frame, rather than one chunk
        // whose throughput would depend on render FPS at higher sample rates.
        for _ in 0..16 {
            let before = sentence.drained;
            result = Self::advance(&mut sentence, outputs, runtime, now);
            if result.is_err() || sentence.drained == before || sentence.flushed {
                break;
            }
        }
        if result.is_err() {
            self.notice = Some("Call audio could not continue.");
            self.cancel_sentence(sentence, outputs, runtime, now);
        } else if sentence.destinations.iter().all(|d| d.done) {
            self.cancel_sentence(sentence, outputs, runtime, now);
        } else if sentence.destinations.iter().all(|d| d.done || d.finishing) {
            // Finished media must not block draining the next local decoder
            // while the remote player acknowledges its tail.
            self.awaiting.push(sentence);
        } else {
            self.queue.push_front(sentence);
        }
    }
    fn receive_outcomes(
        sentence: &mut Sentence,
        outputs: &mut SpeechOutputs,
        notice: &mut Option<&'static str>,
    ) {
        while let Some(outcome) = outputs.take_source_outcome(sentence.source) {
            if outcome.outcome == AudioPlaybackOutcome::Failed {
                *notice = Some("Call audio stopped.");
            }
            for d in &mut sentence.destinations {
                if d.playback == Some(outcome.playback_id) {
                    d.done = true;
                    d.outcome = Some(outcome.outcome);
                }
            }
        }
    }
    fn advance(
        s: &mut Sentence,
        outputs: &mut SpeechOutputs,
        runtime: &ExtensionRuntime,
        now: Instant,
    ) -> Result<(), ()> {
        if now < s.created {
            return Err(());
        }
        let batch_bound = s
            .converter
            .as_ref()
            .map_or(56, |c| c.batch_frame_bound(2048));
        if !s.flushed
            && s.pending.len() + batch_bound > MAX_PENDING
            && let Some(front) = s.pending.front()
            && s.destinations
                .iter()
                .enumerate()
                .any(|(i, destination)| !destination.done && front.delivered & (1 << i) != 0)
        {
            // Preserve destinations that have progressed. A lagging destination
            // cannot retain the shared bounded window until the native tap overflows.
            for (i, destination) in s.destinations.iter_mut().enumerate() {
                if !destination.done && front.delivered & (1 << i) == 0 {
                    if let Some(id) = destination.playback {
                        outputs.cancel(runtime, id, now);
                    }
                    destination.done = true;
                }
            }
        }
        if s.pending.len() + batch_bound <= MAX_PENDING && !s.flushed {
            if let Some(chunk) = s.tap.drain(2048).map_err(|_| ())? {
                let converter = match &mut s.converter {
                    Some(c) => c,
                    None => s.converter.insert(
                        CallPcm::with_speed(chunk.sample_rate, chunk.channels, s.speed)
                            .map_err(|_| ())?,
                    ),
                };
                let frames = converter.push(&chunk).map_err(|_| ())?;
                s.drained += chunk.samples.len() as u64;
                if frames.len() > MAX_PENDING {
                    return Err(());
                }
                s.pending
                    .extend(frames.into_iter().map(|pcm| Frame { pcm, delivered: 0 }));
                s.waiting.get_or_insert(now);
                s.last_input = now;
            }
            if s.tap.state().map_err(|_| ())? == SpeechPcmState::Abandoned {
                return Err(());
            }
            if s.tap.is_finished().map_err(|_| ())? {
                if s.tap.state().map_err(|_| ())? != SpeechPcmState::Completed {
                    return Err(());
                }
                let frames = s.converter.as_mut().map(|c| c.finish()).unwrap_or_default();
                if s.pending.len() + frames.len() > MAX_PENDING {
                    return Err(());
                }
                s.pending
                    .extend(frames.into_iter().map(|pcm| Frame { pcm, delivered: 0 }));
                s.flushed = true;
            }
            if !s.flushed
                && s.pending.is_empty()
                && now.saturating_duration_since(s.last_input) >= START_WAIT
            {
                s.source_stalled = true;
                return Err(());
            }
        }
        if s.flushed && s.pending.is_empty() && s.converter.is_none() {
            for d in &mut s.destinations {
                d.done = true;
            }
            return Ok(());
        }
        if s.converter.is_none() && now.saturating_duration_since(s.created) >= START_WAIT {
            s.source_stalled = true;
            return Err(());
        }
        // Recheck cancellation after conversion before enqueueing any resulting samples.
        if s.stream.as_ref().is_some_and(StreamingVoice::is_failed) {
            return Err(());
        }
        if matches!(s.tap.state().map_err(|_| ())?, SpeechPcmState::Abandoned) {
            return Err(());
        }
        for d in &mut s.destinations {
            if d.done {
                continue;
            }
            if runtime
                .audio_playback_route(d.route.installation_id, d.route.target)
                .as_ref()
                != Some(&d.route)
            {
                if let Some(id) = d.playback {
                    outputs.cancel(runtime, id, now);
                }
                d.done = true;
                continue;
            }
            if d.playback.is_none() && !s.pending.is_empty() {
                match outputs.begin(
                    runtime,
                    d.route.installation_id,
                    d.route.target,
                    s.source,
                    now,
                ) {
                    Ok(id) => d.playback = Some(id),
                    Err((ErrorCode::Busy | ErrorCode::Unavailable, _)) => {
                        if s.waiting
                            .is_some_and(|t| now.saturating_duration_since(t) >= START_WAIT)
                        {
                            d.done = true;
                        }
                    }
                    Err(_) => d.done = true,
                }
            }
        }
        // Each destination advances independently; a blocked peer must not
        // prevent healthy destinations consuming later frames in order.
        for (i, destination) in s.destinations.iter_mut().enumerate() {
            if destination.done {
                continue;
            }
            let Some(id) = destination.playback else {
                continue;
            };
            let bit = 1 << i;
            for frame in &mut s.pending {
                if frame.delivered & bit != 0 {
                    continue;
                }
                match outputs.push_frame(id, &frame.pcm) {
                    Ok(()) => frame.delivered |= bit,
                    Err((ErrorCode::Busy, _)) => break,
                    Err(_) => {
                        outputs.cancel(runtime, id, now);
                        destination.done = true;
                        break;
                    }
                }
            }
        }
        while s.pending.front().is_some_and(|frame| {
            s.destinations
                .iter()
                .enumerate()
                .all(|(i, destination)| destination.done || frame.delivered & (1 << i) != 0)
        }) {
            s.pending.pop_front();
        }
        if s.flushed {
            for (i, d) in s.destinations.iter_mut().enumerate() {
                let delivered = s
                    .pending
                    .iter()
                    .all(|frame| frame.delivered & (1 << i) != 0);
                if !d.done && !d.finishing && delivered {
                    if let Some(id) = d.playback {
                        if outputs.finish(id).is_err() {
                            outputs.cancel(runtime, id, now);
                            d.done = true;
                        }
                        d.finishing = true;
                    } else {
                        d.done = true;
                    }
                }
            }
        }
        if s.flushed
            && s.destinations.iter().any(|destination| {
                (destination.finishing && !destination.done)
                    || destination.outcome == Some(AudioPlaybackOutcome::Completed)
            })
        {
            // Source EOF is the live fanout boundary: peers that still cannot
            // accept the complete source must not hold up subsequent speech.
            for destination in &mut s.destinations {
                if !destination.done && !destination.finishing {
                    if let Some(id) = destination.playback {
                        outputs.cancel(runtime, id, now);
                    }
                    destination.done = true;
                }
            }
        }
        Ok(())
    }
}
