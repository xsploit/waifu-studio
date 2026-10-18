//! Incremental RIFF/WAVE decoding off the audio thread; bounded progressive playback.
use bevy::{asset::Asset, audio::Decodable, reflect::TypePath};
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Production network budget: wall time spent on the network, excluding time
/// the producer is blocked on a full playback queue.
pub(crate) const NETWORK_BUDGET: Duration = Duration::from_secs(90);
/// A full queue that the consumer does not drain for this long fails the stream.
pub(crate) const STALL_LIMIT: Duration = Duration::from_secs(10);
/// Absolute wall-clock cap for one stream, playback waits included.
pub(crate) const ABSOLUTE_CAP: Duration = Duration::from_secs(20 * 60);

/// Shared time budget for one streaming speech worker. The WAV parser, SSE and
/// WebSocket readers all check the same budget; only `StreamingVoice::push`
/// records playback backpressure, so a long healthy utterance is not cut off
/// while a stalled network still fails after `network` of unblocked time.
#[derive(Clone, Debug)]
pub(crate) struct StreamBudget(Arc<BudgetState>);
#[derive(Debug)]
struct BudgetState {
    started: Instant,
    network: Duration,
    stall: Duration,
    cap: Duration,
    blocked_nanos: AtomicU64,
}
impl Default for StreamBudget {
    fn default() -> Self {
        Self::with_limits(NETWORK_BUDGET, STALL_LIMIT, ABSOLUTE_CAP)
    }
}
impl StreamBudget {
    pub(crate) fn with_limits(network: Duration, stall: Duration, cap: Duration) -> Self {
        Self(Arc::new(BudgetState {
            started: Instant::now(),
            network: network.min(cap),
            stall,
            cap,
            blocked_nanos: AtomicU64::new(0),
        }))
    }
    pub(crate) fn started(&self) -> Instant {
        self.0.started
    }
    fn blocked(&self) -> Duration {
        Duration::from_nanos(self.0.blocked_nanos.load(Ordering::Relaxed))
    }
    fn record_blocked(&self, waited: Duration) {
        let nanos = waited.as_nanos().min(u64::MAX as u128) as u64;
        self.0.blocked_nanos.fetch_add(nanos, Ordering::Relaxed);
    }
    /// Remaining time before `check` fails, for bounding a single blocking wait.
    pub(crate) fn remaining(&self) -> Duration {
        let wall = self.0.started.elapsed();
        let network = wall.saturating_sub(self.blocked());
        self.0
            .network
            .saturating_sub(network)
            .min(self.0.cap.saturating_sub(wall))
    }
    pub(crate) fn check(&self) -> Result<(), &'static str> {
        let wall = self.0.started.elapsed();
        if wall >= self.0.cap {
            return Err("Speech stream exceeded its absolute time limit");
        }
        if wall.saturating_sub(self.blocked()) >= self.0.network {
            return Err("Speech stream exceeded its network time budget");
        }
        Ok(())
    }
}

/// Only already-sanitized adapter errors may cross the generic WAV reader boundary.
#[derive(Debug)]
pub(crate) struct SafeStreamError(pub String);
impl std::fmt::Display for SafeStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SafeStreamError {}

/// One WAV worker writes complete PCM frames; only the claimed decoder reads.
/// Publishing indices after slot writes prevents partial stereo frames. Atomic
/// slots keep this safe without unsafe aliasing, locks or callback allocations.
#[derive(Debug)]
struct PcmQueue {
    slots: Box<[[AtomicU32; 2]]>,
    read: AtomicUsize,
    write: AtomicUsize,
}
impl PcmQueue {
    fn new(capacity: usize) -> Self {
        Self {
            slots: (0..=capacity)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                .collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
        }
    }
    fn len(&self) -> usize {
        let read = self.read.load(Ordering::Acquire);
        let write = self.write.load(Ordering::Acquire);
        (write + self.slots.len() - read) % self.slots.len()
    }
    fn push(&self, samples: &[f32], channels: usize) -> usize {
        let mut write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        let free = (read + self.slots.len() - write - 1) % self.slots.len();
        let frames = free.min(samples.len() / channels);
        for frame in samples[..frames * channels].chunks_exact(channels) {
            for (slot, sample) in self.slots[write].iter().zip(frame) {
                slot.store(sample.to_bits(), Ordering::Relaxed);
            }
            write = (write + 1) % self.slots.len();
        }
        self.write.store(write, Ordering::Release);
        frames * channels
    }
    fn pop(&self, frame: &mut [f32]) -> bool {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return false;
        }
        for (sample, slot) in frame.iter_mut().zip(&self.slots[read]) {
            *sample = f32::from_bits(slot.load(Ordering::Relaxed));
        }
        self.read
            .store((read + 1) % self.slots.len(), Ordering::Release);
        true
    }
}

#[derive(Debug)]
struct Shared {
    samples: PcmQueue,
    failed: AtomicBool,
    done: AtomicBool,
    claimed: AtomicBool,
    generation: Arc<AtomicU64>,
    expected: u64,
    channels: u16,
    rate: u32,
}
#[derive(Asset, TypePath, Clone, Debug)]
pub struct StreamingVoice {
    shared: Arc<Shared>,
}
pub struct StreamingDecoder {
    shared: Arc<Shared>,
    active: bool,
    frame: [f32; 2],
    next_channel: usize,
}
impl StreamingVoice {
    /// Complete frames currently available, excluding synthesized underrun silence.
    pub fn buffered_frames(&self) -> usize {
        self.shared.samples.len()
    }
    /// Decoder production has ended; queued frames may still be available.
    pub fn is_finished(&self) -> bool {
        self.shared.done.load(Ordering::Acquire)
    }
    pub fn is_failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire) || self.cancelled()
    }
    fn new(channels: u16, rate: u32, generation: Arc<AtomicU64>, expected: u64) -> Self {
        let capacity = rate as usize * 2;
        Self {
            shared: Arc::new(Shared {
                samples: PcmQueue::new(capacity),
                failed: AtomicBool::new(false),
                done: AtomicBool::new(false),
                claimed: AtomicBool::new(false),
                generation,
                expected,
                channels,
                rate,
            }),
        }
    }
    fn cancelled(&self) -> bool {
        self.shared.generation.load(Ordering::Acquire) != self.shared.expected
    }
    fn push(&self, samples: &[f32], budget: &StreamBudget) -> Result<(), String> {
        let mut offset = 0;
        let mut progress = Instant::now();
        let mut observed = usize::MAX;
        while offset < samples.len() {
            if self.cancelled() {
                return Err("Speech cancelled".into());
            }
            budget.check()?;
            let queued = self.shared.samples.len();
            if queued < observed {
                progress = Instant::now();
            }
            let n = self
                .shared
                .samples
                .push(&samples[offset..], self.shared.channels as usize);
            if n > 0 {
                progress = Instant::now();
            }
            offset += n;
            observed = self.shared.samples.len();
            if offset < samples.len() {
                if progress.elapsed() >= budget.0.stall {
                    return Err("Speech playback stalled".into());
                }
                // Waiting for playback to drain is not network time.
                let waiting = Instant::now();
                std::thread::sleep(Duration::from_millis(2));
                budget.record_blocked(waiting.elapsed());
            }
        }
        Ok(())
    }
    fn buffered(&self) -> usize {
        self.shared.samples.len() * self.shared.channels as usize
    }
    fn finish(&self) {
        self.shared.done.store(true, Ordering::Release);
    }
}
impl Decodable for StreamingVoice {
    type Decoder = StreamingDecoder;
    fn decoder(&self) -> Self::Decoder {
        StreamingDecoder {
            shared: self.shared.clone(),
            active: !self.shared.claimed.swap(true, Ordering::AcqRel),
            frame: [0.0; 2],
            next_channel: self.shared.channels as usize,
        }
    }
}
impl Iterator for StreamingDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.active
            || self.shared.failed.load(Ordering::Acquire)
            || self.shared.generation.load(Ordering::Acquire) != self.shared.expected
        {
            self.active = false;
            return None;
        }
        let channels = self.shared.channels as usize;
        if self.next_channel == channels {
            // Producer publication cannot force silence while frames are queued.
            // Only a genuine empty queue emits a complete silent frame.
            self.frame.fill(0.0);
            if !self.shared.samples.pop(&mut self.frame[..channels])
                && self.shared.done.load(Ordering::Acquire)
                // Finish may have been published just after the first empty read.
                // Recheck after acquiring it so the final PCM frame isn't lost.
                && !self.shared.samples.pop(&mut self.frame[..channels])
            {
                self.active = false;
                return None;
            }
            self.next_channel = 0;
        }
        let sample = self.frame[self.next_channel];
        self.next_channel += 1;
        Some(sample)
    }
}
impl rodio::Source for StreamingDecoder {
    fn current_span_len(&self) -> Option<usize> {
        if !self.active
            || self.shared.failed.load(Ordering::Acquire)
            || self.shared.generation.load(Ordering::Acquire) != self.shared.expected
        {
            Some(0)
        } else {
            None
        }
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.shared
            .channels
            .try_into()
            .expect("validated channel count")
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.shared.rate.try_into().expect("validated sample rate")
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct Input<R> {
    reader: R,
    bytes: usize,
    budget: StreamBudget,
    generation: Arc<AtomicU64>,
    expected: u64,
}
impl<R: Read> Input<R> {
    fn read(&mut self, target: &mut [u8]) -> Result<usize, String> {
        if self.generation.load(Ordering::Acquire) != self.expected {
            return Err("Speech cancelled".into());
        }
        self.budget.check()?;
        let n = self.reader.read(target).map_err(|error| {
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<SafeStreamError>())
                .map(|safe| safe.0.clone())
                .unwrap_or_else(|| "Could not receive speech stream".into())
        })?;
        self.bytes += n;
        if self.bytes > 32 * 1024 * 1024 {
            return Err("Speech audio exceeds 32 MiB".into());
        }
        Ok(n)
    }
    fn exact(&mut self, target: &mut [u8]) -> Result<(), String> {
        let mut pos = 0;
        while pos < target.len() {
            let n = self.read(&mut target[pos..])?;
            if n == 0 {
                return Err("Truncated speech WAV".into());
            }
            pos += n;
        }
        Ok(())
    }
    fn skip(&mut self, mut size: usize) -> Result<(), String> {
        let mut scratch = [0; 4096];
        while size > 0 {
            let n = size.min(scratch.len());
            self.exact(&mut scratch[..n])?;
            size -= n;
        }
        Ok(())
    }
}
fn u16le(v: &[u8]) -> u16 {
    u16::from_le_bytes(v[..2].try_into().unwrap())
}
fn u32le(v: &[u8]) -> u32 {
    u32::from_le_bytes(v[..4].try_into().unwrap())
}

/// Observed Fish encoder sentinel, shared by HTTP, timestamp SSE and WS.
/// Only the exact pair in the standard 44-byte PCM header means streaming.
pub(crate) fn fish_unknown_wav_length(riff: u32, data: u32, header_bytes: usize) -> bool {
    riff == 0xffff_ff24 && data == 0xffff_ff00 && header_bytes == 44
}

/// `receive_wav_with_budget` with a network budget ending at `deadline` and
/// the production stall watchdog and absolute cap.
#[cfg(test)]
pub(crate) fn receive_wav<R: Read>(
    reader: R,
    generation: Arc<AtomicU64>,
    expected: u64,
    deadline: Instant,
    ready: impl FnMut(StreamingVoice),
) -> Result<(), String> {
    let network = deadline.saturating_duration_since(Instant::now());
    let budget = StreamBudget::with_limits(network, STALL_LIMIT, ABSOLUTE_CAP);
    receive_wav_with_budget(reader, generation, expected, &budget, ready)
}

/// Callback fires once after 180 ms of PCM is buffered, or at EOF for short clips.
/// Only standard little-endian RIFF PCM16 / IEEE float32 are accepted.
pub(crate) fn receive_wav_with_budget<R: Read>(
    reader: R,
    generation: Arc<AtomicU64>,
    expected: u64,
    budget: &StreamBudget,
    mut ready: impl FnMut(StreamingVoice),
) -> Result<(), String> {
    let mut input = Input {
        reader,
        bytes: 0,
        budget: budget.clone(),
        generation: generation.clone(),
        expected,
    };
    let mut riff = [0; 12];
    input.exact(&mut riff)?;
    if &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
        return Err("Streaming speech requires RIFF WAV".into());
    }
    let mut format = None;
    let length = loop {
        let mut header = [0; 8];
        input.exact(&mut header)?;
        let size = u32le(&header[4..]);
        match &header[..4] {
            b"fmt " => {
                if !(16..=4096).contains(&size) {
                    return Err("Unsupported WAV format header".into());
                }
                let mut fmt = vec![0; size as usize];
                input.exact(&mut fmt)?;
                let (code, channels, rate, bits) = (
                    u16le(&fmt),
                    u16le(&fmt[2..]),
                    u32le(&fmt[4..]),
                    u16le(&fmt[14..]),
                );
                if !matches!((code, bits), (1, 16) | (3, 32))
                    || !(1..=2).contains(&channels)
                    || !(8000..=192000).contains(&rate)
                    || u16le(&fmt[12..]) != channels * (bits / 8)
                    || u32le(&fmt[8..]) != rate * channels as u32 * (bits as u32 / 8)
                {
                    return Err("Streaming WAV supports mono/stereo PCM16 or float32 only".into());
                }
                format = Some((code, channels, rate, bits));
                if !size.is_multiple_of(2) {
                    input.skip(1)?;
                }
            }
            b"data" => break size,
            _ => input.skip(size as usize + (size % 2) as usize)?,
        }
    };
    let (code, channels, rate, bits) = format.ok_or("WAV data precedes format")?;
    let audio = StreamingVoice::new(channels, rate, generation, expected);
    let result = (|| {
        let width = (bits / 8) as usize;
        let frame = width * channels as usize;
        let mut remaining = if length == 0
            || length == u32::MAX
            || fish_unknown_wav_length(u32le(&riff[4..]), length, input.bytes)
        {
            None
        } else {
            Some(length as usize)
        };
        let mut carry = Vec::new();
        let mut buffer = [0; 4096];
        let mut emitted = false;
        let mut total = 0usize;
        loop {
            if remaining == Some(0) {
                break;
            }
            let limit = remaining.unwrap_or(buffer.len()).min(buffer.len());
            let n = input.read(&mut buffer[..limit])?;
            if n == 0 {
                if remaining.is_some_and(|n| n > 0) {
                    return Err(format!(
                        "Truncated speech WAV data (declared {length} bytes [0x{length:08x}], received {} bytes)",
                        length as usize - remaining.unwrap()
                    ));
                }
                break;
            }
            if let Some(nleft) = remaining.as_mut() {
                *nleft -= n;
            }
            carry.extend_from_slice(&buffer[..n]);
            let usable = carry.len() / frame * frame;
            let samples: Vec<f32> = carry[..usable]
                .chunks_exact(width)
                .map(|b| {
                    if code == 1 {
                        i16::from_le_bytes(b.try_into().unwrap()) as f32 / 32768.0
                    } else {
                        let v = f32::from_le_bytes(b.try_into().unwrap());
                        if v.is_finite() {
                            v.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        }
                    }
                })
                .collect();
            total += samples.len();
            audio.push(&samples, budget)?;
            carry.drain(..usable);
            if !emitted && audio.buffered() >= rate as usize * channels as usize * 180 / 1000 {
                ready(audio.clone());
                emitted = true;
            }
        }
        if !carry.is_empty() {
            return Err("Truncated WAV sample frame".into());
        }
        if total == 0 {
            return Err("Speech WAV contains no samples".into());
        }
        if !emitted {
            ready(audio.clone());
        }
        Ok(())
    })();
    // Invalidate without a producer-side reset of the consumer's read index.
    if result.is_err() {
        audio.shared.failed.store(true, Ordering::Release);
    }
    audio.finish();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav() -> Vec<u8> {
        let mut b = b"RIFF".to_vec();
        b.extend(40u32.to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(8000u32.to_le_bytes());
        b.extend(16000u32.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(u32::MAX.to_le_bytes());
        b.extend(16384i16.to_le_bytes());
        b.extend((-16384i16).to_le_bytes());
        b
    }
    struct Fragmented(std::io::Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            let n = b.len().min(1);
            self.0.read(&mut b[..n])
        }
    }
    #[test]
    fn fragmented_unknown_length_wav() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut clip = None;
        receive_wav(
            Fragmented(std::io::Cursor::new(wav())),
            generation,
            1,
            Instant::now() + Duration::from_secs(1),
            |a| clip = Some(a),
        )
        .unwrap();
        let mut decoder = clip.unwrap().decoder();
        assert_eq!(decoder.next(), Some(0.5));
        assert_eq!(decoder.next(), Some(-0.5));
        assert_eq!(decoder.next(), None);
    }
    #[test]
    fn queue_bounds_underrun_cancel_and_single_consumer() {
        let generation = Arc::new(AtomicU64::new(1));
        let audio = StreamingVoice::new(1, 8000, generation.clone(), 1);
        let mut decoder = audio.decoder();
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(audio.decoder().next(), None);
        audio
            .push(&vec![0.5; 16000], &StreamBudget::default())
            .unwrap();
        let expired = StreamBudget::with_limits(Duration::ZERO, STALL_LIMIT, ABSOLUTE_CAP);
        assert!(audio.push(&[1.0], &expired).is_err());
        assert_eq!(audio.buffered(), 16000);
        generation.store(2, Ordering::Release);
        assert_eq!(decoder.next(), None);
    }
    #[test]
    fn stereo_publication_and_underrun_preserve_frame_alignment() {
        let audio = StreamingVoice::new(2, 8000, Arc::new(AtomicU64::new(1)), 1);
        let mut decoder = audio.decoder();
        let budget = StreamBudget::default();
        audio.push(&[0.1, 0.2, 0.3, 0.4], &budget).unwrap();
        assert_eq!(decoder.next(), Some(0.1));
        // Publishing more audio between channels cannot interrupt queued frames.
        audio.push(&[0.7, 0.8], &budget).unwrap();
        assert_eq!(decoder.next(), Some(0.2));
        assert_eq!(decoder.next(), Some(0.3));
        assert_eq!(decoder.next(), Some(0.4));
        assert_eq!(decoder.next(), Some(0.7));
        assert_eq!(decoder.next(), Some(0.8));
        // Data arriving halfway through an underrun must await the next frame.
        assert_eq!(decoder.next(), Some(0.0));
        audio.push(&[0.5, 0.6], &budget).unwrap();
        audio.finish();
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(decoder.next(), Some(0.5));
        assert_eq!(decoder.next(), Some(0.6));
        assert_eq!(decoder.next(), None);
    }
    #[test]
    fn concurrent_pcm_publication_preserves_every_sample_without_gaps() {
        let queue = Arc::new(PcmQueue::new(127));
        let producer = queue.clone();
        let worker = std::thread::spawn(move || {
            for n in 1..=50_000 {
                let frame = [n as f32, -(n as f32)];
                while producer.push(&frame, 2) == 0 {
                    std::thread::yield_now();
                }
            }
        });
        for n in 1..=50_000 {
            let mut frame = [0.0; 2];
            while !queue.pop(&mut frame) {
                std::thread::yield_now();
            }
            assert_eq!(frame, [n as f32, -(n as f32)]);
        }
        worker.join().unwrap();
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn completion_published_during_playback_keeps_the_final_frame() {
        for _ in 0..32 {
            let audio = StreamingVoice::new(2, 8000, Arc::new(AtomicU64::new(1)), 1);
            let mut decoder = audio.decoder();
            let worker = std::thread::spawn(move || {
                let budget = StreamBudget::default();
                for n in 1..=2048 {
                    audio.push(&[n as f32, -(n as f32)], &budget).unwrap();
                }
                audio.finish();
            });
            let mut received = 0;
            while let Some(left) = decoder.next() {
                let right = decoder.next().expect("complete stereo frame");
                if left == 0.0 {
                    assert_eq!(right, 0.0);
                    std::thread::yield_now();
                } else {
                    received += 1;
                    assert_eq!([left, right], [received as f32, -(received as f32)]);
                }
            }
            worker.join().unwrap();
            assert_eq!(received, 2048);
        }
    }

    #[test]
    fn float_stereo_known_length_and_nonfinite_samples() {
        let mut bytes = wav();
        bytes[20..22].copy_from_slice(&3u16.to_le_bytes());
        bytes[22..24].copy_from_slice(&2u16.to_le_bytes());
        bytes[28..32].copy_from_slice(&64000u32.to_le_bytes());
        bytes[32..34].copy_from_slice(&8u16.to_le_bytes());
        bytes[34..36].copy_from_slice(&32u16.to_le_bytes());
        bytes[40..44].copy_from_slice(&8u32.to_le_bytes());
        bytes.truncate(44);
        bytes.extend(0.75f32.to_le_bytes());
        bytes.extend(f32::NAN.to_le_bytes());
        bytes.extend(b"trailing metadata");
        let mut clip = None;
        receive_wav(
            &bytes[..],
            Arc::new(AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(1),
            |a| clip = Some(a),
        )
        .unwrap();
        let mut decoder = clip.unwrap().decoder();
        assert_eq!(decoder.next(), Some(0.75));
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(decoder.next(), None);
    }
    /// Known-length 8 kHz mono PCM16; 32000 samples is twice the queue capacity.
    fn long_wav(samples: usize) -> Vec<u8> {
        let mut b = wav();
        b.truncate(40);
        b.extend((samples as u32 * 2).to_le_bytes());
        b.extend(std::iter::repeat_n(8192i16.to_le_bytes(), samples).flatten());
        b
    }
    /// Delivers one byte per read after a fixed network delay.
    struct Slow(std::io::Cursor<Vec<u8>>, Duration);
    impl Read for Slow {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.1);
            let n = b.len().min(1);
            self.0.read(&mut b[..n])
        }
    }
    fn short(network: u64, stall: u64, cap: u64) -> StreamBudget {
        StreamBudget::with_limits(
            Duration::from_millis(network),
            Duration::from_millis(stall),
            Duration::from_millis(cap),
        )
    }
    #[test]
    fn healthy_playback_longer_than_network_budget_succeeds() {
        // Draining 16000 samples at ~100 per 2 ms keeps the producer blocked on a
        // full queue for >= 320 ms, well past the 200 ms network budget.
        let budget = short(200, 2_000, 20_000);
        let (send, recv) = std::sync::mpsc::channel::<StreamingVoice>();
        let consumer = std::thread::spawn(move || {
            let mut decoder = recv.recv().unwrap().decoder();
            let mut played = 0usize;
            loop {
                for _ in 0..100 {
                    match decoder.next() {
                        Some(_) => played += 1,
                        None => return played,
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        let started = Instant::now();
        receive_wav_with_budget(
            &long_wav(32_000)[..],
            Arc::new(AtomicU64::new(1)),
            1,
            &budget,
            |audio| send.send(audio).unwrap(),
        )
        .unwrap();
        assert!(started.elapsed() > Duration::from_millis(200));
        // Underrun silence may add frames; every real sample is still delivered.
        assert!(consumer.join().unwrap() >= 32_000);
    }
    #[test]
    fn stalled_network_still_fails_within_its_budget() {
        let started = Instant::now();
        let error = receive_wav_with_budget(
            Slow(
                std::io::Cursor::new(long_wav(32_000)),
                Duration::from_millis(5),
            ),
            Arc::new(AtomicU64::new(1)),
            1,
            &short(100, 10_000, 20_000),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error, "Speech stream exceeded its network time budget");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn undrained_consumer_terminates_and_invalidates_playback() {
        let mut clip = None;
        let started = Instant::now();
        let error = receive_wav_with_budget(
            &long_wav(32_000)[..],
            Arc::new(AtomicU64::new(1)),
            1,
            &short(10_000, 100, 20_000),
            |audio| clip = Some(audio),
        )
        .unwrap_err();
        assert_eq!(error, "Speech playback stalled");
        assert!(started.elapsed() < Duration::from_secs(2));
        let mut decoder = clip.unwrap().decoder();
        assert_eq!(decoder.next(), None);
    }
    #[test]
    fn slow_but_live_consumer_hits_absolute_cap() {
        // One frame per 10 ms resets the stall watchdog but cannot outlast the cap.
        let audio = StreamingVoice::new(1, 8000, Arc::new(AtomicU64::new(1)), 1);
        let mut decoder = audio.decoder();
        audio
            .push(&vec![0.5; 16000], &StreamBudget::default())
            .unwrap();
        let drain = std::thread::spawn(move || {
            for _ in 0..40 {
                decoder.next();
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let started = Instant::now();
        let error = audio
            .push(&vec![0.5; 1000], &short(10_000, 1_000, 150))
            .unwrap_err();
        assert_eq!(error, "Speech stream exceeded its absolute time limit");
        assert!(started.elapsed() < Duration::from_secs(2));
        drain.join().unwrap();
    }
    #[test]
    fn cancellation_is_prompt_while_blocked_on_playback() {
        let generation = Arc::new(AtomicU64::new(1));
        let cancel = generation.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            cancel.store(2, Ordering::Release);
        });
        let started = Instant::now();
        let error = receive_wav_with_budget(
            &long_wav(32_000)[..],
            generation,
            1,
            &StreamBudget::default(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error, "Speech cancelled");
        assert!(started.elapsed() < Duration::from_secs(2));
        canceller.join().unwrap();
    }
    #[test]
    fn rejects_truncated_and_unsupported_wav() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut bytes = wav();
        bytes.pop();
        assert!(
            receive_wav(
                &bytes[..],
                generation.clone(),
                1,
                Instant::now() + Duration::from_secs(1),
                |_| {}
            )
            .is_err()
        );
        let mut bytes = wav();
        bytes[20] = 6;
        assert!(
            receive_wav(
                &bytes[..],
                generation,
                1,
                Instant::now() + Duration::from_secs(1),
                |_| {}
            )
            .is_err()
        );
    }
    #[test]
    fn finite_data_truncation_reports_declared_and_received_bytes() {
        // A large finite size is not evidence that a provider intended an
        // unknown-length stream. Keep failing closed and report its exact size.
        for declared in [8u32, 0x7fff_ffff] {
            let mut bytes = wav();
            bytes[40..44].copy_from_slice(&declared.to_le_bytes());
            let error = receive_wav(
                Fragmented(std::io::Cursor::new(bytes)),
                Arc::new(AtomicU64::new(1)),
                1,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("A short incomplete clip must not start playback"),
            )
            .unwrap_err();
            assert_eq!(
                error,
                format!(
                    "Truncated speech WAV data (declared {declared} bytes [0x{declared:08x}], received 4 bytes)"
                )
            );
        }
    }

    #[test]
    fn fish_streaming_header_works_in_the_shared_http_and_sse_decoder() {
        let mut bytes = wav();
        bytes[4..8].copy_from_slice(&0xffff_ff24u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&0xffff_ff00u32.to_le_bytes());
        let mut ready = false;
        receive_wav(
            Fragmented(std::io::Cursor::new(bytes.clone())),
            Arc::new(AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(1),
            |_| ready = true,
        )
        .unwrap();
        assert!(ready);
        bytes.pop();
        assert!(
            receive_wav(
                &bytes[..],
                Arc::new(AtomicU64::new(1)),
                1,
                Instant::now() + Duration::from_secs(1),
                |_| {}
            )
            .unwrap_err()
            .contains("sample frame")
        );
        assert!(!fish_unknown_wav_length(0xffff_ff25, 0xffff_ff00, 44));
        assert!(!fish_unknown_wav_length(0xffff_ff24, 0xffff_ff00, 46));
        assert!(!fish_unknown_wav_length(0xffff_ff24, 0xffff_fefe, 44));
    }
}
