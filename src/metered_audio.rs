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
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_AGE_MICROS: u64 = 250_000;
const ZERO: u64 = 0;
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
    fn wrap(&self, source: SpeechSource, visemes: Option<VisemeTap>) -> MeteredSpeech {
        let expected = self.0.begin();
        MeteredSpeech {
            source,
            ticket: Arc::new(Ticket {
                shared: self.0.clone(),
                expected,
                claimed: AtomicBool::new(false),
                visemes,
            }),
        }
    }
    pub fn stop(&self) {
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
    claimed: AtomicBool,
    visemes: Option<VisemeTap>,
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
    ticket: Arc<Ticket>,
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
}
impl MeteredDecoder {
    fn new(inner: Option<Box<dyn Source + Send>>, ticket: Arc<Ticket>) -> Self {
        let channels = inner
            .as_ref()
            .map(|s| s.channels())
            .unwrap_or_else(|| 1.try_into().unwrap());
        let rate = inner
            .as_ref()
            .map(|s| s.sample_rate())
            .unwrap_or_else(|| 48_000.try_into().unwrap());
        let duration = inner.as_ref().and_then(|s| s.total_duration());
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
        }
    }
    fn finish(&mut self) {
        if self.active {
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
        MeteredDecoder::new(inner, self.ticket.clone())
    }
}
impl Iterator for MeteredDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.active {
            return None;
        }
        if !self.ticket.current() {
            self.finish();
            return None;
        }
        let Some(sample) = self.inner.as_mut().and_then(|s| s.next()) else {
            self.finish();
            return None;
        };
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if let Some(tap) = &mut self.visemes {
            tap.push(sample);
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
        self.finish();
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
        let source = meter.streaming(stream.unwrap());
        let mut decoder = source.decoder();
        assert_eq!(decoder.channels().get(), 1);
        assert_eq!(decoder.sample_rate().get(), 8_000);
        for _ in 0..160 {
            assert_eq!(decoder.next(), Some(0.5));
        }
        assert_eq!(meter.level(), 0.5);
        drop(decoder);
        assert_eq!(meter.level(), 0.0);
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
