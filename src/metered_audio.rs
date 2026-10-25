//! Measures the samples consumed by Bevy's audio output, for buffered and
//! progressive speech alike. The added meter and viseme tap use arithmetic and
//! atomic operations only. The streaming source uses an atomic PCM queue;
//! analysis never runs in the callback or waits for the UI.
use crate::{expressions::AudioEnvelope, viseme_audio::VisemeTap, voice_stream::StreamingVoice};
use bevy::{
    audio::{AddAudioSource, Decodable},
    prelude::*,
};
use rodio::Source;
use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[path = "metered_audio_pcm.rs"]
mod pcm;
pub use pcm::{SpeechPcm, SpeechPcmChunk, SpeechPcmError, SpeechPcmState};

const MAX_AGE_MICROS: u64 = 250_000;
const ZERO: u64 = 0;
// Independent receipt bits: finishing must not erase past consumption.
const PLAYBACK_STARTED: u8 = 1;
const PLAYBACK_TERMINAL: u8 = 2;
const PLAYBACK_EOF: u8 = 4;
fn packed(generation: u32, level: f32) -> u64 {
    ((generation as u64) << 32) | level.to_bits() as u64
}
fn generation(value: u64) -> u32 {
    (value >> 32) as u32
}
fn bounded(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
struct MeterShared {
    value: AtomicU64,
    updated: AtomicU64,
    cancellation: AtomicU64,
    current_pcm: Mutex<Weak<pcm::Shared>>,
    epoch: Instant,
}
impl MeterShared {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
    fn begin(&self) -> u32 {
        let mut previous = self.value.load(Ordering::Acquire);
        loop {
            let next = generation(previous).wrapping_add(1);
            match self.value.compare_exchange_weak(
                previous,
                packed(next, 0.0),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.updated.store(self.now(), Ordering::Release);
                    return next;
                }
                Err(actual) => previous = actual,
            }
        }
    }
    fn publish(&self, expected: u32, level: f32) {
        let mut previous = self.value.load(Ordering::Acquire);
        while generation(previous) == expected {
            match self.value.compare_exchange_weak(
                previous,
                packed(expected, bounded(level)),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.updated.store(self.now(), Ordering::Release);
                    return;
                }
                Err(actual) => previous = actual,
            }
        }
    }
}
/// Starting or stopping speech atomically invalidates every old decoder.
/// Generation and level share one atomic word: an old decoder's EOF or Drop
/// cannot race a newer decoder and erase its mouth envelope.
#[derive(Resource, Clone)]
pub struct SpeechMeter(Arc<MeterShared>);
impl Default for SpeechMeter {
    fn default() -> Self {
        Self(Arc::new(MeterShared {
            value: AtomicU64::new(ZERO),
            updated: AtomicU64::new(0),
            cancellation: AtomicU64::new(0),
            current_pcm: Mutex::new(Weak::new()),
            epoch: Instant::now(),
        }))
    }
}
impl SpeechMeter {
    pub fn buffered(&self, source: AudioSource) -> MeteredSpeech {
        self.wrap(SpeechSource::Buffered(source), None)
    }
    pub fn streaming(&self, source: StreamingVoice) -> MeteredSpeech {
        self.wrap(SpeechSource::Streaming(source), None)
    }
    pub fn buffered_with_visemes(&self, source: AudioSource, tap: VisemeTap) -> MeteredSpeech {
        self.wrap(SpeechSource::Buffered(source), Some(tap))
    }
    pub fn streaming_with_visemes(&self, source: StreamingVoice, tap: VisemeTap) -> MeteredSpeech {
        self.wrap(SpeechSource::Streaming(source), Some(tap))
    }
    /// Route the existing decoded playback to one optional external consumer.
    /// Attach before decoder creation; ordinary playback pays no queue allocation.
    pub fn buffered_routed(
        &self,
        source: AudioSource,
        visemes: Option<VisemeTap>,
    ) -> (MeteredSpeech, SpeechPcm) {
        self.routed(SpeechSource::Buffered(source), visemes)
    }
    pub fn streaming_routed(
        &self,
        source: StreamingVoice,
        visemes: Option<VisemeTap>,
    ) -> (MeteredSpeech, SpeechPcm) {
        self.routed(SpeechSource::Streaming(source), visemes)
    }
    fn routed(
        &self,
        source: SpeechSource,
        visemes: Option<VisemeTap>,
    ) -> (MeteredSpeech, SpeechPcm) {
        let shared = pcm::Shared::new();
        let speech = self.wrap_with_pcm(source, visemes, Some(shared.clone()));
        let output = SpeechPcm::new(shared, speech.ticket.clone());
        (speech, output)
    }
    fn wrap(&self, source: SpeechSource, visemes: Option<VisemeTap>) -> MeteredSpeech {
        self.wrap_with_pcm(source, visemes, None)
    }
    fn wrap_with_pcm(
        &self,
        source: SpeechSource,
        visemes: Option<VisemeTap>,
        pcm: Option<Arc<pcm::Shared>>,
    ) -> MeteredSpeech {
        // This lock is used only during asset construction, never by next().
        // EOF and replacement linearize on the tap's own atomic state.
        let mut current_pcm = self
            .0
            .current_pcm
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = current_pcm.upgrade() {
            previous.replace();
        }
        *current_pcm = pcm.as_ref().map(Arc::downgrade).unwrap_or_default();
        let expected = self.0.begin();
        let pcm_owner = pcm.as_ref().map(|shared| pcm::Owner::new(shared.clone()));
        MeteredSpeech {
            source,
            pcm_owner,
            ticket: Arc::new(Ticket {
                shared: self.0.clone(),
                expected,
                cancellation: self.0.cancellation.load(Ordering::Acquire),
                claimed: AtomicBool::new(false),
                playback: AtomicU8::new(0),
                visemes,
                pcm,
            }),
        }
    }
    pub fn stop(&self) {
        // Serialize epoch and generation changes with asset construction.
        // Stop is a control-thread operation, never an audio callback.
        let _current_pcm = self
            .0
            .current_pcm
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.0.cancellation.fetch_add(1, Ordering::AcqRel);
        self.0.begin();
    }
    pub fn level(&self) -> f32 {
        if self
            .0
            .now()
            .saturating_sub(self.0.updated.load(Ordering::Acquire))
            > MAX_AGE_MICROS
        {
            return 0.0;
        }
        bounded(f32::from_bits(self.0.value.load(Ordering::Acquire) as u32))
    }
}
struct Ticket {
    shared: Arc<MeterShared>,
    expected: u32,
    cancellation: u64,
    claimed: AtomicBool,
    playback: AtomicU8,
    visemes: Option<VisemeTap>,
    pcm: Option<Arc<pcm::Shared>>,
}
impl Ticket {
    fn current(&self) -> bool {
        generation(self.shared.value.load(Ordering::Acquire)) == self.expected
    }
}
#[derive(Clone)]
enum SpeechSource {
    Buffered(AudioSource),
    Streaming(StreamingVoice),
}
#[derive(Asset, TypePath, Clone)]
pub struct MeteredSpeech {
    source: SpeechSource,
    pcm_owner: Option<Arc<pcm::Owner>>,
    ticket: Arc<Ticket>,
}

/// Audio consumption receipt shared with the frame thread. Creating an asset
/// or decoder does not count as playback; the first consumed sample does,
/// including silent samples. Cancellation invalidates live playback immediately;
/// historical consumption remains observable after EOF, Drop or invalidation.
#[derive(Clone)]
pub struct SpeechPlayback(Arc<Ticket>);
impl SpeechPlayback {
    /// Identity of one decoder ticket, independent of current/terminal state.
    pub fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Historical natural decoder EOF; not remote completion or audibility.
    /// Drop/cancellation never sets this bit.
    pub fn reached_eof(&self) -> bool {
        self.0.playback.load(Ordering::Acquire) & PLAYBACK_EOF != 0
    }
    pub fn is_playing(&self) -> bool {
        self.0.current() && self.0.playback.load(Ordering::Acquire) == PLAYBACK_STARTED
    }
    /// Latched when the claimed decoder first returns a sample, including silence.
    /// Unlike is_playing, this remains true after completion or cancellation.
    pub fn has_started(&self) -> bool {
        self.0.playback.load(Ordering::Acquire) & PLAYBACK_STARTED != 0
    }
    /// True once the claimed decoder reaches EOF/is dropped, or this receipt is
    /// invalidated by stop or a replacement source. This does not distinguish
    /// natural EOF, cancellation and Drop, or confirm audio-device drain.
    /// An unclaimed asset's destruction alone is not observed by this receipt.
    pub fn is_terminal(&self) -> bool {
        !self.0.current() || self.0.playback.load(Ordering::Acquire) & PLAYBACK_TERMINAL != 0
    }
}
impl MeteredSpeech {
    pub fn playback(&self) -> SpeechPlayback {
        SpeechPlayback(self.ticket.clone())
    }
}

pub struct MeteredDecoder {
    inner: Option<Box<dyn Source + Send>>,
    ticket: Arc<Ticket>,
    active: bool,
    sum_squares: f64,
    count: usize,
    window: usize,
    channels: rodio::ChannelCount,
    rate: rodio::SampleRate,
    duration: Option<Duration>,
    visemes: Option<VisemeTap>,
    _pcm_owner: Option<Arc<pcm::Owner>>,
}
impl MeteredDecoder {
    fn new(
        inner: Option<Box<dyn Source + Send>>,
        ticket: Arc<Ticket>,
        pcm_owner: Option<Arc<pcm::Owner>>,
    ) -> Self {
        let channels = inner
            .as_ref()
            .map(|s| s.channels())
            .unwrap_or_else(|| 1.try_into().unwrap());
        let rate = inner
            .as_ref()
            .map(|s| s.sample_rate())
            .unwrap_or_else(|| 48_000.try_into().unwrap());
        let duration = inner.as_ref().and_then(|s| s.total_duration());
        if inner.is_some()
            && let Some(output) = &ticket.pcm
        {
            output.configure(rate.get(), channels.get());
        }
        let window = ((rate.get() as usize * channels.get() as usize) / 50).max(1);
        // Only the successful source claim can reach here with an inner decoder.
        // Duplicates must never finish or write the active viseme session.
        let mut visemes = inner.as_ref().and_then(|_| ticket.visemes.clone());
        if let Some(tap) = &mut visemes {
            tap.configure(rate.get(), channels.get() as usize);
        }
        Self {
            active: inner.is_some(),
            inner,
            ticket,
            sum_squares: 0.0,
            count: 0,
            window,
            channels,
            rate,
            duration,
            visemes,
            _pcm_owner: pcm_owner,
        }
    }
    fn finish(&mut self, eof: bool) {
        if self.active {
            // Publish the decoder receipt before releasing tap completion, so
            // routing retirement can never race ahead of the EOF distinction.
            self.ticket.playback.fetch_or(
                PLAYBACK_TERMINAL | if eof { PLAYBACK_EOF } else { 0 },
                Ordering::Release,
            );
            if let Some(output) = &self.ticket.pcm {
                output.finish(eof);
            }
            self.ticket.shared.publish(self.ticket.expected, 0.0);
            self.active = false;
            if let Some(tap) = &mut self.visemes {
                tap.finish();
            }
        }
    }
}
impl Decodable for MeteredSpeech {
    type Decoder = MeteredDecoder;
    fn decoder(&self) -> Self::Decoder {
        let claimed = !self.ticket.claimed.swap(true, Ordering::AcqRel);
        let inner: Option<Box<dyn Source + Send>> = if claimed && self.ticket.current() {
            Some(match &self.source {
                SpeechSource::Buffered(source) => Box::new(source.decoder()),
                SpeechSource::Streaming(source) => Box::new(source.decoder()),
            })
        } else {
            None
        };
        let pcm_owner = inner.as_ref().and(self.pcm_owner.clone());
        MeteredDecoder::new(inner, self.ticket.clone(), pcm_owner)
    }
}
impl Iterator for MeteredDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.active {
            return None;
        }
        if !self.ticket.current() {
            self.finish(false);
            return None;
        }
        let Some(sample) = self.inner.as_mut().and_then(|s| s.next()) else {
            self.finish(true);
            return None;
        };
        // Only the claimed decoder reaches this point. Avoid a write for every
        // sample once playback has started.
        if self.ticket.playback.load(Ordering::Relaxed) == 0 {
            self.ticket
                .playback
                .fetch_or(PLAYBACK_STARTED, Ordering::Release);
        }
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if let Some(tap) = &mut self.visemes {
            tap.push(sample);
        }
        if let Some(output) = &self.ticket.pcm {
            output.push(sample);
        }
        self.sum_squares += (sample as f64).powi(2);
        self.count += 1;
        if self.count >= self.window {
            self.ticket.shared.publish(
                self.ticket.expected,
                (self.sum_squares / self.count as f64).sqrt() as f32,
            );
            self.count = 0;
            self.sum_squares = 0.0;
        }
        Some(sample)
    }
}
impl Source for MeteredDecoder {
    fn current_span_len(&self) -> Option<usize> {
        if !self.active || !self.ticket.current() {
            Some(0)
        } else {
            self.inner.as_ref().and_then(|s| s.current_span_len())
        }
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}
impl Drop for MeteredDecoder {
    fn drop(&mut self) {
        self.finish(false);
    }
}

pub struct MeteredAudioPlugin;
impl Plugin for MeteredAudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioEnvelope>()
            .init_resource::<SpeechMeter>()
            .add_audio_source::<MeteredSpeech>()
            .add_systems(
                PostUpdate,
                sync_speech_envelope.before(crate::expressions::ExpressionSystems),
            );
    }
}
/// Runs on the frame thread, keeping the audio callback independent of Bevy
/// resources. Only the current decoder's atomic sample envelope reaches faces.
pub fn sync_speech_envelope(meter: Res<SpeechMeter>, envelope: Res<AudioEnvelope>) {
    envelope.set(meter.level());
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wave(samples: usize, value: i16) -> Vec<u8> {
        let data_size = samples as u32 * 2;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data_size).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8_000u32.to_le_bytes());
        bytes.extend(16_000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data_size.to_le_bytes());
        for _ in 0..samples {
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }
    fn audio(samples: usize, value: i16) -> AudioSource {
        AudioSource {
            bytes: wave(samples, value).into(),
        }
    }
    #[test]
    fn routed_state_distinguishes_eof_abandonment_and_stop_before_tail_drain() {
        let meter = SpeechMeter::default();
        let (source, mut pcm) = meter.buffered_routed(audio(320, 16384), None);
        let receipt = source.playback();
        assert!(!receipt.reached_eof());
        assert_eq!(pcm.state().unwrap(), SpeechPcmState::Open);
        assert_eq!(source.decoder().count(), 320);
        assert_eq!(pcm.state().unwrap(), SpeechPcmState::Completed);
        assert!(receipt.reached_eof());
        assert!(!pcm.is_finished().unwrap());
        assert_eq!(pcm.drain(4096).unwrap().unwrap().samples.len(), 320);
        assert!(pcm.is_finished().unwrap());
        meter.stop();
        assert_eq!(pcm.state(), Err(SpeechPcmError::Cancelled));
        let (source, pcm) = meter.buffered_routed(audio(320, 16384), None);
        let receipt = source.playback();
        let mut decoder = source.decoder();
        decoder.next();
        drop(decoder);
        assert!(!receipt.reached_eof());
        assert_eq!(pcm.state().unwrap(), SpeechPcmState::Abandoned);
        let (source, pcm) = meter.buffered_routed(audio(320, 16384), None);
        drop(source);
        assert_eq!(pcm.state().unwrap(), SpeechPcmState::Abandoned);
    }
    #[test]
    fn dropping_unclaimed_routed_asset_finishes_tap_even_with_retained_receipt() {
        let meter = SpeechMeter::default();
        let (source, mut output) = meter.buffered_routed(audio(320, 16_384), None);
        let receipt = source.playback();
        let clone = source.clone();
        drop(source);
        assert!(!output.is_finished().unwrap());
        drop(clone);
        assert!(output.is_finished().unwrap());
        assert!(output.drain(1).unwrap().is_none());
        assert!(!receipt.has_started());
    }
    #[test]
    fn routed_decoder_keeps_tap_live_after_asset_drop() {
        let meter = SpeechMeter::default();
        let (source, mut output) = meter.buffered_routed(audio(320, 16_384), None);
        let mut decoder = source.decoder();
        drop(source);
        assert!(!output.is_finished().unwrap());
        assert_eq!(decoder.next(), Some(0.5));
        assert_eq!(output.drain(1).unwrap().unwrap().samples, vec![0.5]);
        drop(decoder);
        assert!(output.is_finished().unwrap());
    }
    #[test]
    fn completed_routed_tail_survives_next_sentence_but_stop_cancels_all_tails() {
        let meter = SpeechMeter::default();
        let (first, mut tail) = meter.buffered_routed(audio(320, 16_384), None);
        assert_eq!(first.decoder().count(), 320);
        let (second, mut second_tail) = meter.buffered_routed(audio(160, 8192), None);
        assert_eq!(tail.drain(160).unwrap().unwrap().samples, vec![0.5; 160]);
        assert_eq!(second.decoder().count(), 160);
        meter.stop();
        assert_eq!(tail.drain(160), Err(SpeechPcmError::Cancelled));
        assert_eq!(second_tail.drain(160), Err(SpeechPcmError::Cancelled));
    }
    #[test]
    fn interrupted_routed_tail_cannot_become_completed_after_replacement() {
        let meter = SpeechMeter::default();
        let (first, mut tail) = meter.buffered_routed(audio(320, 16_384), None);
        let mut decoder = first.decoder();
        assert_eq!(decoder.next(), Some(0.5));
        let _next = meter.buffered(audio(160, 8192));
        assert_eq!(tail.drain(160), Err(SpeechPcmError::Cancelled));
        assert_eq!(decoder.next(), None);
        drop(decoder);
        assert_eq!(tail.drain(160), Err(SpeechPcmError::Cancelled));
    }
    #[test]
    fn dropped_decoder_tail_is_readable_only_until_sentence_replacement() {
        let meter = SpeechMeter::default();
        let (first, mut tail) = meter.buffered_routed(audio(320, 16_384), None);
        let mut decoder = first.decoder();
        for _ in 0..160 {
            decoder.next();
        }
        drop(decoder);
        assert_eq!(tail.drain(80).unwrap().unwrap().samples, vec![0.5; 80]);
        let _next = meter.buffered(audio(160, 8192));
        assert_eq!(tail.drain(160), Err(SpeechPcmError::Cancelled));
    }
    #[test]
    fn routed_pcm_matches_local_samples_and_duplicate_decoder_cannot_finish_it() {
        let meter = SpeechMeter::default();
        let (source, mut output) = meter.buffered_routed(audio(320, 16_384), None);
        assert!(output.drain(4096).unwrap().is_none());
        let mut decoder = source.decoder();
        assert_eq!(source.decoder().next(), None);
        assert!(!output.is_finished().unwrap());
        let local: Vec<_> = decoder.by_ref().collect();
        let remote = output.drain(4096).unwrap().unwrap();
        assert_eq!((remote.sample_rate, remote.channels), (8000, 1));
        assert_eq!(remote.samples, local);
        assert!(output.is_finished().unwrap());
        assert!(output.drain(4096).unwrap().is_none());
        assert!(source.playback().has_started());
    }
    #[test]
    fn routed_pcm_cancellation_invalidates_queued_samples_before_callback_runs_again() {
        let meter = SpeechMeter::default();
        let (source, mut output) = meter.buffered_routed(audio(320, 16_384), None);
        let mut decoder = source.decoder();
        decoder.next();
        meter.stop();
        assert_eq!(output.drain(10), Err(SpeechPcmError::Cancelled));
        assert_eq!(output.is_finished(), Err(SpeechPcmError::Cancelled));
        assert_eq!(decoder.next(), None);
    }
    #[test]
    fn routed_pcm_overflow_fails_tap_without_truncating_local_playback() {
        let meter = SpeechMeter::default();
        let (source, mut output) = meter.buffered_routed(audio(40_000, 16_384), None);
        assert_eq!(source.decoder().count(), 40_000);
        assert_eq!(output.drain(4096), Err(SpeechPcmError::Overflow));
        assert!(source.playback().is_terminal());
    }
    #[test]
    fn dropping_routed_pcm_consumer_does_not_stop_local_playback() {
        let meter = SpeechMeter::default();
        let (source, output) = meter.buffered_routed(audio(320, 16_384), None);
        drop(output);
        assert_eq!(source.decoder().count(), 320);
        assert!(source.playback().is_terminal());
    }
    #[test]
    fn routed_pcm_preserves_stereo_frames_and_finishes_after_partial_frame_drop() {
        let meter = SpeechMeter::default();
        let mut bytes = wave(8, 16_384);
        bytes[22..24].copy_from_slice(&2u16.to_le_bytes());
        bytes[28..32].copy_from_slice(&32_000u32.to_le_bytes());
        bytes[32..34].copy_from_slice(&4u16.to_le_bytes());
        let (source, mut output) = meter.buffered_routed(
            AudioSource {
                bytes: bytes.into(),
            },
            None,
        );
        let mut decoder = source.decoder();
        decoder.next();
        assert!(output.drain(1).unwrap().is_none());
        decoder.next();
        let chunk = output.drain(1).unwrap().unwrap();
        assert_eq!(chunk.channels, 2);
        assert_eq!(chunk.samples, vec![0.5, 0.5]);
        decoder.next();
        drop(decoder);
        assert!(output.drain(1).unwrap().is_none());
        assert!(output.is_finished().unwrap());
    }
    #[test]
    fn buffered_measures_consumed_samples_and_silences_at_eof() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(320, 16_384));
        assert_eq!(meter.level(), 0.0);
        let mut decoder = source.decoder();
        for _ in 0..159 {
            assert_eq!(decoder.next(), Some(0.5));
        }
        assert_eq!(meter.level(), 0.0);
        assert_eq!(decoder.next(), Some(0.5));
        assert!((meter.level() - 0.5).abs() < 0.0001);
        for _ in 0..160 {
            decoder.next();
        }
        assert_eq!(decoder.next(), None);
        assert_eq!(meter.level(), 0.0);
    }
    #[test]
    fn playback_receipt_tracks_consumption_even_when_silent_and_ignores_duplicates() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(2, 0));
        let playback = source.playback();
        assert!(!playback.is_playing(), "queued is not playing");
        let mut decoder = source.decoder();
        assert!(!playback.is_playing(), "decoder creation is not playback");
        assert_eq!(decoder.next(), Some(0.0));
        assert!(playback.is_playing(), "silence is still consumed audio");
        assert_eq!(source.decoder().next(), None);
        assert!(
            playback.is_playing(),
            "a duplicate cannot finish the source"
        );
        assert!(playback.has_started());
        assert!(!playback.is_terminal(), "duplicate EOF is not source EOF");
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(decoder.next(), None);
        assert!(!playback.is_playing());

        let source = meter.buffered(audio(2, 0));
        let playback = source.playback();
        let mut decoder = source.decoder();
        decoder.next();
        assert!(playback.is_playing());
        meter.stop();
        assert!(
            !playback.is_playing(),
            "stop invalidates before audio cleanup"
        );
        assert_eq!(decoder.next(), None);
        let source = meter.buffered(audio(2, 0));
        let playback = source.playback();
        let mut decoder = source.decoder();
        decoder.next();
        drop(decoder);
        assert!(
            !playback.is_playing(),
            "dropping the consumer ends playback"
        );
    }
    #[test]
    fn short_clip_eof_before_polling_preserves_both_receipt_latches() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(1, 0));
        let playback = source.playback();
        let retained = playback.clone();
        let mut decoder = source.decoder();
        // The frame thread does not inspect the receipt between these calls.
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(decoder.next(), None);
        assert!(playback.has_started());
        assert!(playback.is_terminal());
        assert!(!playback.is_playing());
        drop(decoder);
        drop(source);
        assert!(retained.has_started());
        assert!(retained.is_terminal());
    }
    #[test]
    fn dropped_unconsumed_decoder_is_terminal_without_false_start() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(1, 16_384));
        let playback = source.playback();
        assert!(!playback.has_started());
        assert!(!playback.is_terminal());
        let decoder = source.decoder();
        assert!(!playback.has_started(), "claiming is not consumption");
        assert!(!playback.is_terminal());
        drop(decoder);
        drop(source);
        assert!(!playback.has_started());
        assert!(playback.is_terminal());
        assert!(!playback.is_playing());
    }
    #[test]
    fn cancellation_and_replacement_preserve_consumption_history() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(2, 0));
        let playback = source.playback();
        let mut decoder = source.decoder();
        assert_eq!(decoder.next(), Some(0.0));
        meter.stop();
        assert!(
            playback.is_terminal(),
            "invalidation precedes decoder cleanup"
        );
        assert!(playback.has_started());
        assert!(!playback.is_playing());
        assert_eq!(decoder.next(), None);
        assert!(playback.has_started());

        let unconsumed = meter.buffered(audio(1, 0));
        let unconsumed_playback = unconsumed.playback();
        let mut unconsumed_decoder = unconsumed.decoder();
        meter.stop();
        assert!(unconsumed_playback.is_terminal());
        assert!(!unconsumed_playback.has_started());
        assert_eq!(unconsumed_decoder.next(), None);
        assert!(!unconsumed_playback.has_started());

        let superseded = meter.buffered(audio(1, 0));
        let superseded_playback = superseded.playback();
        let current = meter.buffered(audio(1, 0));
        let current_playback = current.playback();
        assert!(superseded_playback.is_terminal());
        assert!(!superseded_playback.has_started());
        assert!(!current_playback.is_terminal());
        assert!(!current_playback.has_started());
    }
    #[test]
    fn stale_eof_and_drop_cannot_silence_new_generation() {
        let meter = SpeechMeter::default();
        let old_source = meter.buffered(audio(320, 16_384));
        let mut old = old_source.decoder();
        for _ in 0..160 {
            old.next();
        }
        let new_source = meter.buffered(audio(320, 24_576));
        let mut new = new_source.decoder();
        for _ in 0..160 {
            new.next();
        }
        assert_eq!(meter.level(), 0.75);
        assert_eq!(old.next(), None);
        drop(old);
        assert_eq!(meter.level(), 0.75);
        drop(old_source.decoder());
        assert_eq!(meter.level(), 0.75);
        meter.stop();
        assert_eq!(meter.level(), 0.0);
        assert_eq!(new.next(), None);
    }
    #[test]
    fn duplicate_decoder_never_claims_or_silences_playing_source() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(320, 16_384));
        let mut first = source.decoder();
        for _ in 0..160 {
            first.next();
        }
        assert_eq!(meter.level(), 0.5);
        let mut duplicate = source.decoder();
        assert_eq!(duplicate.next(), None);
        drop(duplicate);
        assert_eq!(meter.level(), 0.5);
        drop(first);
        assert_eq!(meter.level(), 0.0);
    }
    #[test]
    fn streaming_pcm_uses_same_envelope_and_preserves_source_metadata() {
        let meter = SpeechMeter::default();
        let mut stream = None;
        crate::voice_stream::receive_wav(
            &wave(320, 16_384)[..],
            Arc::new(AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(1),
            |voice| stream = Some(voice),
        )
        .unwrap();
        let (source, mut output) = meter.streaming_routed(stream.unwrap(), None);
        let mut decoder = source.decoder();
        assert_eq!(decoder.channels().get(), 1);
        assert_eq!(decoder.sample_rate().get(), 8_000);
        for _ in 0..160 {
            assert_eq!(decoder.next(), Some(0.5));
        }
        assert_eq!(meter.level(), 0.5);
        let chunk = output.drain(160).unwrap().unwrap();
        assert_eq!(chunk.samples, vec![0.5; 160]);
        assert_eq!((chunk.sample_rate, chunk.channels), (8000, 1));
        drop(decoder);
        assert!(output.is_finished().unwrap());
        assert_eq!(meter.level(), 0.0);
    }
    #[test]
    fn routed_pcm_wraps_queue_repeatedly_without_loss_or_reordering() {
        let meter = SpeechMeter::default();
        let mut bytes = wave(80_000, 0);
        for (i, sample) in bytes[44..].as_chunks_mut::<2>().0.iter_mut().enumerate() {
            sample.copy_from_slice(&((i % 30_000) as i16 - 15_000).to_le_bytes());
        }
        let (source, mut output) = meter.buffered_routed(
            AudioSource {
                bytes: bytes.into(),
            },
            None,
        );
        let mut decoder = source.decoder();
        let mut count = 0;
        loop {
            let local: Vec<_> = decoder.by_ref().take(1000).collect();
            if local.is_empty() {
                break;
            }
            let remote = output.drain(1000).unwrap().unwrap();
            assert_eq!(local, remote.samples);
            count += local.len();
        }
        assert_eq!(count, 80_000);
        assert!(output.is_finished().unwrap());
    }
    #[test]
    fn concurrent_stale_publications_cannot_erase_new_speech() {
        let meter = SpeechMeter::default();
        let old = meter.0.begin();
        let start = Arc::new(std::sync::Barrier::new(2));
        let worker_meter = meter.clone();
        let worker_start = start.clone();
        let worker = std::thread::spawn(move || {
            worker_start.wait();
            for _ in 0..10_000 {
                worker_meter.0.publish(old, 0.0);
            }
        });
        let current = meter.0.begin();
        meter.0.publish(current, 0.75);
        start.wait();
        worker.join().unwrap();
        assert_eq!(meter.level(), 0.75);
    }
    #[test]
    fn superseded_atomic_publication_cannot_replace_current_level() {
        let meter = SpeechMeter::default();
        let previous = meter.0.begin();
        meter.0.publish(previous, 0.5);
        let current = meter.0.begin();
        meter.0.publish(current, 0.8);
        meter.0.publish(previous, 0.0);
        meter.0.publish(previous, 1.0);
        assert_eq!(meter.level(), 0.8);
    }
}
