//! Device-independent pacing of the existing speech decoder. This owns no sink,
//! provider request or route; callers must not also hand its asset to AudioPlayer.
use crate::metered_audio::MeteredSpeech;
use bevy::audio::Decodable;
use rodio::Source;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const MAX_CLOCKS: usize = 4;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(crate) static TEST_CLOCK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn next_deadline(previous: Instant, now: Instant, step: Duration) -> Instant {
    let next = previous + step;
    if next < now { now + step } else { next }
}
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Finish(Arc<AtomicBool>);
impl Drop for Finish {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum ClockError {
    InvalidSpeed,
    Busy,
    SpawnFailed,
}
/// Dropping cancels and wakes the worker without joining on the frame thread.
/// Finished means decoder consumption stopped, never remote audibility or EOF.
/// At most four decoder jobs retain their slot through asset/decoder teardown.
pub struct SpeechClock {
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}
impl SpeechClock {
    pub fn start(source: MeteredSpeech, speed: f32) -> Result<Self, ClockError> {
        if !speed.is_finite() || !(0.5..=2.0).contains(&speed) {
            return Err(ClockError::InvalidSpeed);
        }
        ACTIVE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_CLOCKS).then_some(count + 1)
            })
            .map_err(|_| ClockError::Busy)?;
        let permit = Permit;
        let cancelled = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let done = Finish(finished.clone());
        let worker = thread::Builder::new()
            .name("studio-speech-clock".into())
            .spawn(move || {
                // Reverse drop order: asset, permit, then finished publication.
                let (_done, _permit, source) = (done, permit, source);
                if stop.load(Ordering::Acquire) {
                    return;
                }
                let mut decoder = source.decoder();
                let receipt = source.playback();
                let channels = decoder.channels().get() as usize;
                let rate = decoder.sample_rate().get();
                if !(1..=2).contains(&channels) || !(4000..=384_000).contains(&rate) {
                    return;
                }
                // Match Rodio Speed's f32 multiplication and integer truncation.
                let effective = (rate as f32 * speed).max(1.0) as u32;
                let frames = effective.div_ceil(200); // at most about5 ms ahead
                let step = Duration::from_secs_f64(frames as f64 / effective as f64);
                let mut deadline = Instant::now();
                loop {
                    for _ in 0..frames {
                        if stop.load(Ordering::Acquire) || receipt.is_terminal() {
                            return;
                        }
                        for _ in 0..channels {
                            if decoder.next().is_none() {
                                return;
                            }
                        }
                    }
                    // Scheduler stalls never cause an unbounded catch-up burst.
                    deadline = next_deadline(deadline, Instant::now(), step);
                    loop {
                        if stop.load(Ordering::Acquire) || receipt.is_terminal() {
                            return;
                        }
                        let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                            break;
                        };
                        thread::park_timeout(wait);
                    }
                }
            })
            .map_err(|_| ClockError::SpawnFailed)?;
        Ok(Self {
            cancelled,
            finished,
            worker,
        })
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.worker.thread().unpark();
    }
}
impl Drop for SpeechClock {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metered_audio::{SpeechMeter, SpeechPcmState};
    fn wave(samples: usize) -> bevy::audio::AudioSource {
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
        for _ in 0..samples {
            bytes.extend(8192i16.to_le_bytes());
        }
        bevy::audio::AudioSource {
            bytes: bytes.into(),
        }
    }
    fn until(done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn steady_decoder_work_does_not_accumulate_clock_drift_or_stall_backlog() {
        let start = Instant::now();
        let step = Duration::from_millis(5);
        let mut deadline = start;
        for _ in 0..1000 {
            let after_work = deadline + Duration::from_micros(500);
            deadline = next_deadline(deadline, after_work, step);
        }
        assert_eq!(deadline.duration_since(start), Duration::from_secs(5));
        let after_stall = deadline + Duration::from_secs(1);
        assert_eq!(
            next_deadline(deadline, after_stall, step),
            after_stall + step
        );
    }
    #[test]
    fn clock_paces_one_decoder_without_device_and_bounds_cancelled_workers() {
        let _guard = TEST_CLOCK_LOCK.lock().unwrap();
        until(|| ACTIVE.load(Ordering::Acquire) == 0);
        for speed in [0.5, 1.0, 2.0] {
            let meter = SpeechMeter::default();
            let (asset, mut tap) = meter.buffered_routed(wave(800), None);
            let duplicate = asset.clone();
            let receipt = asset.playback();
            let start = Instant::now();
            let clock = SpeechClock::start(asset, speed).unwrap();
            until(|| receipt.has_started());
            assert_eq!(duplicate.decoder().next(), None);
            until(|| clock.is_finished());
            assert!(start.elapsed() >= Duration::from_secs_f32(0.09 / speed));
            let mut samples = Vec::new();
            while let Some(chunk) = tap.drain(4096).unwrap() {
                samples.extend(chunk.samples);
            }
            assert_eq!(samples, vec![0.25; 800]);
            assert_eq!(tap.state(), Ok(SpeechPcmState::Completed));
            until(|| ACTIVE.load(Ordering::Acquire) == 0);
        }
        // Completed progressive audio uses the same claimed decoder/tap path.
        let mut stream = None;
        crate::voice_stream::receive_wav(
            wave(800).bytes.as_ref(),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(1),
            |voice| stream = Some(voice),
        )
        .unwrap();
        let meter = SpeechMeter::default();
        let (asset, mut tap) = meter.streaming_routed(stream.unwrap(), None);
        let clock = SpeechClock::start(asset, 1.33).unwrap();
        until(|| clock.is_finished());
        let mut count = 0;
        while let Some(chunk) = tap.drain(4096).unwrap() {
            assert!(chunk.samples.iter().all(|sample| *sample == 0.25));
            count += chunk.samples.len();
        }
        assert_eq!(count, 800);
        assert_eq!(tap.state(), Ok(SpeechPcmState::Completed));
        until(|| ACTIVE.load(Ordering::Acquire) == 0);
        let (asset, tap) = meter.buffered_routed(wave(80_000), None);
        let receipt = asset.playback();
        let clock = SpeechClock::start(asset, 1.0).unwrap();
        until(|| receipt.has_started());
        let dropping = Instant::now();
        drop(clock);
        assert!(dropping.elapsed() < Duration::from_millis(100));
        until(|| ACTIVE.load(Ordering::Acquire) == 0);
        assert_eq!(tap.state(), Ok(SpeechPcmState::Abandoned));
        for speed in [0.0, -1.0, f32::NAN, f32::INFINITY, 0.49, 2.01] {
            assert!(matches!(
                SpeechClock::start(meter.buffered(wave(80)), speed),
                Err(ClockError::InvalidSpeed)
            ));
        }
        assert_eq!(ACTIVE.load(Ordering::Acquire), 0);
        let mut clocks = Vec::new();
        let mut meters = Vec::new();
        for _ in 0..MAX_CLOCKS {
            let meter = SpeechMeter::default();
            clocks.push(SpeechClock::start(meter.buffered(wave(80_000)), 1.0).unwrap());
            meters.push(meter);
        }
        let meter = SpeechMeter::default();
        assert!(matches!(
            SpeechClock::start(meter.buffered(wave(80)), 1.0),
            Err(ClockError::Busy)
        ));
        for clock in &clocks {
            clock.cancel();
        }
        until(|| clocks.iter().all(SpeechClock::is_finished));
        until(|| ACTIVE.load(Ordering::Acquire) == 0);
        let (asset, tap) = meter.buffered_routed(wave(80_000), None);
        let receipt = asset.playback();
        let clock = SpeechClock::start(asset, 1.0).unwrap();
        until(|| receipt.has_started());
        meter.stop();
        until(|| clock.is_finished());
        assert!(tap.state().is_err());
        drop(clock);
        until(|| ACTIVE.load(Ordering::Acquire) == 0);
        // Cancellation may win before decoder claim. Finished must still imply
        // the asset owner has published abandonment and released its job slot.
        for _ in 0..16 {
            let (asset, tap) = meter.buffered_routed(wave(16_000), None);
            let clock = SpeechClock::start(asset, 1.0).unwrap();
            let finished = clock.finished.clone();
            drop(clock);
            until(|| finished.load(Ordering::Acquire));
            assert_eq!(tap.state(), Ok(SpeechPcmState::Abandoned));
            assert_eq!(ACTIVE.load(Ordering::Acquire), 0);
        }
    }
}
