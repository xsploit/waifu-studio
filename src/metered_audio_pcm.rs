//! Single-reader PCM tap fed only by the existing claimed playback decoder.
use super::*;
use std::sync::atomic::{AtomicU32, AtomicUsize};
const CAPACITY: usize = 32_768;
const OPEN: u8 = 0;
const FINISHED: u8 = 1;
const OVERFLOW: u8 = 2;
const CLOSED: u8 = 3;
const UNSUPPORTED: u8 = 4;
const ABANDONED: u8 = 5;
const CANCELLED: u8 = 6;

pub(super) struct Shared {
    slots: Box<[AtomicU32]>,
    read: AtomicUsize,
    write: AtomicUsize,
    state: AtomicU8,
    format: AtomicU64,
}
impl Shared {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: (0..=CAPACITY).map(|_| AtomicU32::new(0)).collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            state: AtomicU8::new(OPEN),
            format: AtomicU64::new(0),
        })
    }
    pub(super) fn configure(&self, rate: u32, channels: u16) {
        if rate == 0 || !(1..=2).contains(&channels) {
            let _ =
                self.state
                    .compare_exchange(OPEN, UNSUPPORTED, Ordering::AcqRel, Ordering::Acquire);
        } else {
            self.format
                .store(((rate as u64) << 32) | channels as u64, Ordering::Release);
        }
    }
    pub(super) fn push(&self, sample: f32) {
        if self.state.load(Ordering::Acquire) != OPEN {
            return;
        }
        let write = self.write.load(Ordering::Relaxed);
        let next = (write + 1) % self.slots.len();
        if next == self.read.load(Ordering::Acquire) {
            let _ =
                self.state
                    .compare_exchange(OPEN, OVERFLOW, Ordering::AcqRel, Ordering::Acquire);
            return;
        }
        self.slots[write].store(sample.to_bits(), Ordering::Relaxed);
        self.write.store(next, Ordering::Release);
    }
    pub(super) fn replace(&self) {
        let mut state = self.state.load(Ordering::Acquire);
        while matches!(state, OPEN | ABANDONED) {
            match self.state.compare_exchange_weak(
                state,
                CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }
    }
    pub(super) fn finish(&self, eof: bool) {
        let _ = self.state.compare_exchange(
            OPEN,
            if eof { FINISHED } else { ABANDONED },
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Owned only by playback assets and their claimed decoder, never receipts.
pub(super) struct Owner(Arc<Shared>);
impl Owner {
    pub(super) fn new(shared: Arc<Shared>) -> Arc<Self> {
        Arc::new(Self(shared))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.finish(false);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPcmError {
    Cancelled,
    Overflow,
    UnsupportedFormat,
}
/// Producer termination is distinct from buffered-tail exhaustion. Only natural
/// EOF permits flushing a routed resampler; abandonment cancels the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPcmState {
    Open,
    Completed,
    Abandoned,
}
/// Native decoded format, interleaved whole frames. No resampling or encoding.
#[derive(Debug, PartialEq)]
pub struct SpeechPcmChunk {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}
/// One bounded consumer, deliberately not Clone. Drop detaches the tap without
/// stopping playback. Queueing is not evidence of remote delivery or playback.
pub struct SpeechPcm {
    shared: Arc<Shared>,
    ticket: Arc<Ticket>,
}
impl SpeechPcm {
    pub(super) fn new(shared: Arc<Shared>, ticket: Arc<Ticket>) -> Self {
        Self { shared, ticket }
    }
    fn check(&self) -> Result<(), SpeechPcmError> {
        if self.ticket.shared.cancellation.load(Ordering::Acquire) != self.ticket.cancellation {
            return Err(SpeechPcmError::Cancelled);
        }
        match self.shared.state.load(Ordering::Acquire) {
            CANCELLED => Err(SpeechPcmError::Cancelled),
            OVERFLOW => Err(SpeechPcmError::Overflow),
            UNSUPPORTED => Err(SpeechPcmError::UnsupportedFormat),
            _ => Ok(()),
        }
    }
    /// Called off the audio callback. Allocation is capped at 4096 whole frames.
    /// A slow consumer fails closed on overflow; local audio never waits for it.
    pub fn drain(&mut self, max_frames: usize) -> Result<Option<SpeechPcmChunk>, SpeechPcmError> {
        self.drain_before_commit(max_frames, || {})
    }
    fn drain_before_commit(
        &mut self,
        max_frames: usize,
        before_commit: impl FnOnce(),
    ) -> Result<Option<SpeechPcmChunk>, SpeechPcmError> {
        self.check()?;
        let format = self.shared.format.load(Ordering::Acquire);
        let channels = format as u16;
        if channels == 0 || max_frames == 0 {
            return Ok(None);
        }
        let read = self.shared.read.load(Ordering::Relaxed);
        let write = self.shared.write.load(Ordering::Acquire);
        let available = (write + self.shared.slots.len() - read) % self.shared.slots.len();
        let count = (available / channels as usize).min(max_frames.min(4096)) * channels as usize;
        if count == 0 {
            return Ok(None);
        }
        let samples = (0..count)
            .map(|i| {
                f32::from_bits(
                    self.shared.slots[(read + i) % self.shared.slots.len()].load(Ordering::Relaxed),
                )
            })
            .collect();
        before_commit();
        self.check()?;
        self.shared
            .read
            .store((read + count) % self.shared.slots.len(), Ordering::Release);
        Ok(Some(SpeechPcmChunk {
            sample_rate: (format >> 32) as u32,
            channels,
            samples,
        }))
    }
    pub fn state(&self) -> Result<SpeechPcmState, SpeechPcmError> {
        self.check()?;
        let state = self.shared.state.load(Ordering::Acquire);
        self.check()?;
        Ok(match state {
            FINISHED => SpeechPcmState::Completed,
            ABANDONED => SpeechPcmState::Abandoned,
            _ => SpeechPcmState::Open,
        })
    }
    pub fn is_finished(&self) -> Result<bool, SpeechPcmError> {
        self.check()?;
        // Acquire completion before reading indices so all final producer
        // writes are visible when deciding whether the queue has drained.
        let state = self.shared.state.load(Ordering::Acquire);
        let channels = self.shared.format.load(Ordering::Acquire) as u16;
        let read = self.shared.read.load(Ordering::Acquire);
        let write = self.shared.write.load(Ordering::Acquire);
        // Dropping between stereo samples discards the incomplete final frame.
        let remaining = (write + self.shared.slots.len() - read) % self.shared.slots.len();
        Ok(matches!(state, FINISHED | ABANDONED)
            && (channels == 0 || remaining < channels as usize))
    }
}
impl Drop for SpeechPcm {
    fn drop(&mut self) {
        self.shared.state.store(CLOSED, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_state_survives_late_format_configuration() {
        let shared = Shared::new();
        shared.replace();
        shared.configure(48000, 3);
        assert_eq!(shared.state.load(Ordering::Acquire), CANCELLED);
    }
    #[test]
    fn stop_waits_for_asset_construction_control_lock() {
        let meter = SpeechMeter::default();
        let guard = meter.0.current_pcm.lock().unwrap();
        let other = meter.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            other.stop();
            done_tx.send(()).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err());
        assert_eq!(meter.0.cancellation.load(Ordering::Acquire), 0);
        drop(guard);
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert_eq!(meter.0.cancellation.load(Ordering::Acquire), 1);
    }
    #[test]
    fn cancellation_during_copy_does_not_commit_the_read_position() {
        let meter = SpeechMeter::default();
        let (_source, mut output) = meter.buffered_routed(
            AudioSource {
                bytes: Vec::new().into(),
            },
            None,
        );
        output.shared.configure(8000, 1);
        output.shared.push(0.5);
        let read = output.shared.read.load(Ordering::Acquire);
        assert_eq!(
            output.drain_before_commit(1, || meter.stop()),
            Err(SpeechPcmError::Cancelled)
        );
        assert_eq!(output.shared.read.load(Ordering::Acquire), read);
    }
    #[test]
    fn eof_and_replacement_share_one_terminal_decision() {
        let completed = Shared::new();
        completed.finish(true);
        completed.replace();
        assert_eq!(completed.state.load(Ordering::Acquire), FINISHED);
        let interrupted = Shared::new();
        interrupted.replace();
        interrupted.finish(true);
        assert_eq!(interrupted.state.load(Ordering::Acquire), CANCELLED);
        let dropped = Shared::new();
        dropped.finish(false);
        dropped.replace();
        assert_eq!(dropped.state.load(Ordering::Acquire), CANCELLED);
    }
}
