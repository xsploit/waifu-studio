//! Off-callback FIR conversion to complete 20 ms 48 kHz stereo PCM16 frames.
//! This module neither claims a decoder nor sends/plays audio.
use crate::{listen::resample::Polyphase, metered_audio::SpeechPcmChunk};

pub const CALL_SAMPLE_RATE: u32 = 48_000;
pub const CALL_FRAME_SAMPLES: usize = 1920;
const MAX_INPUT_FRAMES: usize = 4096;
type Resampler = Polyphase<48_000, 24>;
pub type CallPcmFrame = [i16; CALL_FRAME_SAMPLES];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPcmError {
    InvalidFormat,
    InvalidChunk,
    Finished,
}

pub struct CallPcm {
    rate: u32,
    channels: u16,
    left: Resampler,
    right: Option<Resampler>,
    pending: CallPcmFrame,
    used: usize,
    has_input: bool,
    finished: bool,
}
impl CallPcm {
    /// Includes existing partial packet and the possible final filter tail.
    pub(crate) fn batch_frame_bound(&self, input: usize) -> usize {
        self.left.output_bound_with_tail(input).div_ceil(960) + 1
    }
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, CallPcmError> {
        Self::with_speed(sample_rate, channels, 1.0)
    }
    /// Match the local sink's fixed playback speed without synthesizing/decoding
    /// twice. Input chunks retain their original native format identity.
    pub fn with_speed(sample_rate: u32, channels: u16, speed: f32) -> Result<Self, CallPcmError> {
        let effective = (sample_rate as f32 * speed).max(1.0) as u32;
        if !speed.is_finite()
            || !(0.5..=2.0).contains(&speed)
            || !(2000..=768_000).contains(&effective)
            || !(4000..=384_000).contains(&sample_rate)
            || !(1..=2).contains(&channels)
        {
            return Err(CallPcmError::InvalidFormat);
        }
        Ok(Self {
            rate: sample_rate,
            channels,
            left: Resampler::for_playback(effective),
            right: (channels == 2).then(|| Resampler::for_playback(effective)),
            pending: [0; CALL_FRAME_SAMPLES],
            used: 0,
            has_input: false,
            finished: false,
        })
    }
    /// Validate the whole chunk before changing filter or framing state.
    /// One call accepts at most 4096 native frames; format cannot change mid-turn.
    pub fn push(&mut self, chunk: &SpeechPcmChunk) -> Result<Vec<CallPcmFrame>, CallPcmError> {
        if self.finished {
            return Err(CallPcmError::Finished);
        }
        if chunk.sample_rate != self.rate || chunk.channels != self.channels {
            return Err(CallPcmError::InvalidFormat);
        }
        if !chunk.samples.len().is_multiple_of(self.channels as usize)
            || chunk.samples.len() / self.channels as usize > MAX_INPUT_FRAMES
            || chunk
                .samples
                .iter()
                .any(|s| !s.is_finite() || !(-1.0..=1.0).contains(s))
        {
            return Err(CallPcmError::InvalidChunk);
        }
        let mut output = Vec::new();
        for frame in chunk.samples.chunks_exact(self.channels as usize) {
            self.has_input = true;
            self.feed(
                frame[0],
                if self.channels == 2 {
                    frame[1]
                } else {
                    frame[0]
                },
                &mut output,
            );
        }
        Ok(output)
    }
    fn feed(&mut self, left: f32, right: f32, output: &mut Vec<CallPcmFrame>) {
        let (left, count) = self.left.push(left);
        let right = if let Some(filter) = &mut self.right {
            let (right, other_count) = filter.push(right);
            debug_assert_eq!(count, other_count);
            right
        } else {
            left
        };
        for i in 0..count {
            for sample in [left[i], right[i]] {
                self.pending[self.used] = (sample.clamp(-1.0, 1.0) * 32768.0)
                    .round()
                    .clamp(-32768.0, 32767.0) as i16;
                self.used += 1;
            }
            if self.used == CALL_FRAME_SAMPLES {
                output.push(self.pending);
                self.used = 0;
            }
        }
    }
    /// Preserve the FIR tail, then silence-pad the final partial 20 ms frame.
    /// Idempotent: subsequent finish calls produce no duplicate audio.
    /// For cancellation, drop the converter instead of flushing queued speech.
    pub fn finish(&mut self) -> Vec<CallPcmFrame> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut output = Vec::new();
        if !self.has_input {
            return output;
        }
        for _ in 0..self.left.tail_input_frames() {
            self.feed(0.0, 0.0, &mut output);
        }
        if self.used > 0 {
            self.pending[self.used..].fill(0);
            output.push(self.pending);
            self.used = 0;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn convert(
        rate: u32,
        channels: u16,
        samples: &[f32],
        frames_per_chunk: usize,
    ) -> Vec<CallPcmFrame> {
        let mut converter = CallPcm::new(rate, channels).unwrap();
        let mut frames = Vec::new();
        for samples in samples.chunks(frames_per_chunk * channels as usize) {
            frames.extend(
                converter
                    .push(&SpeechPcmChunk {
                        sample_rate: rate,
                        channels,
                        samples: samples.to_vec(),
                    })
                    .unwrap(),
            );
        }
        frames.extend(converter.finish());
        assert!(converter.finish().is_empty());
        frames
    }
    #[test]
    fn native_stereo_preserves_channel_order_and_pcm16_scale() {
        let samples: Vec<_> = (0..960).flat_map(|_| [0.25, -0.5]).collect();
        let frames = convert(48_000, 2, &samples, 127);
        assert_eq!(frames.len(), 1);
        assert!(
            frames[0]
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| *frame == [8192, -16384])
        );
    }
    #[test]
    fn native_mono_duplicates_without_resampling_or_extra_eof_frame() {
        let frames = convert(48_000, 1, &vec![0.5; 960], 101);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].iter().all(|s| *s == 16384));
    }
    #[test]
    fn invalid_chunk_is_rejected_before_filter_or_frame_state_changes() {
        let mut converter = CallPcm::new(48_000, 2).unwrap();
        for samples in [
            vec![0.5],
            vec![0.0; 8194],
            vec![f32::NAN, 0.0],
            vec![2.0, 0.0],
        ] {
            assert_eq!(
                converter.push(&SpeechPcmChunk {
                    sample_rate: 48_000,
                    channels: 2,
                    samples
                }),
                Err(CallPcmError::InvalidChunk)
            );
        }
        let chunk = SpeechPcmChunk {
            sample_rate: 48_000,
            channels: 2,
            samples: vec![0.25; 1920],
        };
        assert_eq!(
            converter.push(&chunk).unwrap(),
            convert(48_000, 2, &chunk.samples, 960)
        );
        assert!(converter.finish().is_empty());
        assert_eq!(converter.push(&chunk), Err(CallPcmError::Finished));
        assert!(matches!(
            CallPcm::new(0, 1),
            Err(CallPcmError::InvalidFormat)
        ));
        assert!(matches!(
            CallPcm::new(48_000, 3),
            Err(CallPcmError::InvalidFormat)
        ));
    }
    #[test]
    fn playback_speed_matches_the_local_rodio_rate_and_native_input_identity() {
        use rodio::Source;
        let rate = 22050;
        let samples: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.1).sin() * 0.5).collect();
        for speed in [0.5, 1.0, 1.33, 2.0] {
            let local = rodio::buffer::SamplesBuffer::new(
                1.try_into().unwrap(),
                rate.try_into().unwrap(),
                samples.clone(),
            )
            .speed(speed);
            let expected = convert(local.sample_rate().get(), 1, &samples, 73);
            let mut remote = CallPcm::with_speed(rate, 1, speed).unwrap();
            let mut actual = Vec::new();
            for part in samples.chunks(73) {
                actual.extend(
                    remote
                        .push(&SpeechPcmChunk {
                            sample_rate: rate,
                            channels: 1,
                            samples: part.to_vec(),
                        })
                        .unwrap(),
                );
            }
            actual.extend(remote.finish());
            assert_eq!(actual, expected, "speed={speed}");
        }
        for speed in [0.0, -1.0, f32::NAN, f32::INFINITY, 0.001, 1000.0] {
            assert!(matches!(
                CallPcm::with_speed(rate, 1, speed),
                Err(CallPcmError::InvalidFormat)
            ));
        }
    }
    #[test]
    fn ui_speed_extremes_cover_the_full_supported_native_rate_range() {
        for (rate, speed) in [(4000, 0.5), (384000, 2.0)] {
            let mut converter = CallPcm::with_speed(rate, 1, speed).unwrap();
            let effective = (rate as f32 * speed).max(1.0) as u64;
            let input_with_tail = 2048 + converter.left.tail_input_frames() as u64;
            let expected = (input_with_tail * 48_000 / effective).div_ceil(960);
            let mut frames = converter
                .push(&SpeechPcmChunk {
                    sample_rate: rate,
                    channels: 1,
                    samples: vec![0.0; 2048],
                })
                .unwrap();
            frames.extend(converter.finish());
            assert!(!frames.is_empty());
            assert_eq!(frames.len() as u64, expected);
            assert!(frames.len() <= 64);
            assert!(frames.iter().flatten().all(|s| *s == 0));
        }
    }
    #[test]
    fn filter_history_and_fractional_phase_survive_arbitrary_chunk_boundaries() {
        let samples: Vec<_> = (0..8000)
            .flat_map(|i| {
                let angle = i as f32 * std::f32::consts::TAU * 1000.0 / 22_050.0;
                [angle.sin() * 0.5, angle.cos() * 0.25]
            })
            .collect();
        assert_eq!(
            convert(22_050, 2, &samples, 4096),
            convert(22_050, 2, &samples, 73)
        );
    }
    #[test]
    fn finishing_flushes_filter_tail_and_zero_pads_once() {
        let frames = convert(24_000, 1, &[1.0], 1);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].iter().any(|sample| sample.abs() > 1000));
        assert!(frames[0][1000..].iter().all(|sample| *sample == 0));
        let mut empty = CallPcm::new(24_000, 1).unwrap();
        assert!(empty.finish().is_empty());
    }
    #[test]
    fn downsampling_preserves_speech_and_rejects_above_nyquist_alias() {
        let rms = |frequency: f32| {
            let samples: Vec<_> = (0..19_200)
                .map(|i| (i as f32 * std::f32::consts::TAU * frequency / 96_000.0).sin() * 0.5)
                .collect();
            let frames = convert(96_000, 1, &samples, 997);
            let output: Vec<_> = frames
                .iter()
                .flat_map(|frame| {
                    frame
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| pair[0] as f64 / 32768.0)
                })
                .collect();
            (output[2000..6000]
                .iter()
                .map(|sample| sample * sample)
                .sum::<f64>()
                / 4000.0)
                .sqrt()
        };
        let pass = rms(1000.0);
        let stop = rms(30_000.0);
        assert!(pass > 0.30, "speech amplitude lost: {pass}");
        assert!(
            stop < pass * 0.02,
            "out-of-band alias: pass={pass}, stop={stop}"
        );
    }
}
