//! Transport-independent inbound utterance assembly. The adapter must decrypt,
//! decode and resample audio before admission. This is not a Discord voice client.
//! One collector belongs to one participant in one call/target generation.

pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME_SAMPLES: usize = 320;
pub const MAX_FRAMES: u64 = 1_500; // Thirty seconds at 20ms per frame.

#[derive(Debug, PartialEq, Eq)]
pub enum AudioError {
    StaleSession,
    WrongParticipant,
    OutOfOrder,
    InvalidFrame,
    TooLong,
    Empty,
    Stopped,
}

/// External speech, still pending host admission. Generation and participant
/// survive ASR; callers must never route this through the local user's composer.
#[derive(Debug, PartialEq, Eq)]
pub struct ParticipantTranscript {
    pub generation: u64,
    pub participant: u64,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TranscriptionError {
    Audio(AudioError),
    Cancelled,
    Provider(String),
}

/// Ordered, mono 16kHz PCM16 frames for a single externally attributed turn.
/// No mixer: overlapping participants need separate collectors and host arbitration.
pub struct Utterance {
    generation: u64,
    participant: u64,
    next: u64,
    samples: Vec<i16>,
    stopped: bool,
}

impl Utterance {
    pub fn new(generation: u64, participant: u64) -> Self {
        Self {
            generation,
            participant,
            next: 0,
            samples: Vec::new(),
            stopped: false,
        }
    }

    /// Reject gaps, duplicates and route mismatches without partially appending.
    /// The caller must cancel on transport loss; inserting silence is its policy.
    pub fn push(
        &mut self,
        generation: u64,
        participant: u64,
        sequence: u64,
        pcm: &[i16],
    ) -> Result<(), AudioError> {
        if self.stopped {
            return Err(AudioError::Stopped);
        }
        if generation != self.generation {
            return Err(AudioError::StaleSession);
        }
        if participant != self.participant {
            return Err(AudioError::WrongParticipant);
        }
        if sequence != self.next {
            return Err(AudioError::OutOfOrder);
        }
        if pcm.len() != FRAME_SAMPLES {
            return Err(AudioError::InvalidFrame);
        }
        if self.next == MAX_FRAMES {
            self.cancel();
            return Err(AudioError::TooLong);
        }
        self.samples.extend_from_slice(pcm);
        self.next += 1;
        Ok(())
    }

    /// Mute, disconnect, avatar replacement and cancellation discard the turn.
    /// A stopped collector cannot be revived by a late frame.
    pub fn cancel(&mut self) {
        self.stopped = true;
        self.samples.clear();
    }

    /// Blocking worker operation using the app's existing ASR configuration and
    /// session credential. Never call on the UI or audio callback thread.
    /// `cancelled` must cover call, target, permission and service revisions.
    /// Cancellation rejects late results; it does not abort a blocked HTTP read.
    pub fn transcribe(
        self,
        generation: u64,
        config: &crate::listen::ListenConfig,
        session_key: Option<&str>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<ParticipantTranscript, TranscriptionError> {
        self.transcribe_using(generation, cancelled, |wav| {
            crate::listen::transcribe_wav(config, wav, session_key, cancelled)
        })
    }

    fn transcribe_using(
        self,
        generation: u64,
        cancelled: &impl Fn() -> bool,
        transcribe: impl FnOnce(&[u8]) -> Result<String, String>,
    ) -> Result<ParticipantTranscript, TranscriptionError> {
        if cancelled() {
            return Err(TranscriptionError::Cancelled);
        }
        let participant = self.participant;
        let wav = self.finish(generation).map_err(TranscriptionError::Audio)?;
        // Recheck after assembly before provider work, and after either provider
        // outcome so a revoked job cannot publish a late success or error.
        if cancelled() {
            return Err(TranscriptionError::Cancelled);
        }
        let result = transcribe(&wav);
        if cancelled() {
            return Err(TranscriptionError::Cancelled);
        }
        Ok(ParticipantTranscript {
            generation,
            participant,
            text: result.map_err(TranscriptionError::Provider)?,
        })
    }

    /// Consume into a WAV accepted by `listen::transcribe_wav`; this performs no
    /// provider calls and does not turn external speech into authored-user memory.
    pub fn finish(self, generation: u64) -> Result<Vec<u8>, AudioError> {
        if self.stopped {
            return Err(AudioError::Stopped);
        }
        if generation != self.generation {
            return Err(AudioError::StaleSession);
        }
        if self.samples.is_empty() {
            return Err(AudioError::Empty);
        }
        let bytes = (self.samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(bytes as usize + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(bytes + 36).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // Mono
        wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&bytes.to_le_bytes());
        for sample in self.samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(wav)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded_turn() -> Utterance {
        let mut turn = Utterance::new(7, 8);
        turn.push(7, 8, 0, &[1; FRAME_SAMPLES]).unwrap();
        turn
    }

    #[test]
    fn asr_retains_external_participant_and_exact_text() {
        let result = recorded_turn()
            .transcribe_using(7, &|| false, |wav| {
                assert_eq!(wav.len(), 684);
                Ok("Exact spoken words.".into())
            })
            .unwrap();
        assert_eq!(
            result,
            ParticipantTranscript {
                generation: 7,
                participant: 8,
                text: "Exact spoken words.".into()
            }
        );
    }

    #[test]
    fn cancellation_and_stale_generation_do_not_start_asr() {
        assert_eq!(
            recorded_turn().transcribe_using(7, &|| true, |_| panic!("cancelled ASR started")),
            Err(TranscriptionError::Cancelled)
        );
        assert_eq!(
            recorded_turn().transcribe_using(8, &|| false, |_| panic!("stale ASR started")),
            Err(TranscriptionError::Audio(AudioError::StaleSession))
        );
    }

    #[test]
    fn revoked_job_discards_late_provider_success_and_failure() {
        use std::cell::Cell;
        for response in [Ok("late text".into()), Err("late failure".into())] {
            let cancelled = Cell::new(false);
            let result = recorded_turn().transcribe_using(7, &|| cancelled.get(), |_| {
                cancelled.set(true);
                response
            });
            assert_eq!(result, Err(TranscriptionError::Cancelled));
        }
    }

    #[test]
    fn exact_pcm_and_duration_survive_wav_assembly() {
        let mut turn = Utterance::new(7, 8);
        let mut frame = [0; FRAME_SAMPLES];
        frame[..3].copy_from_slice(&[i16::MIN, 0, i16::MAX]);
        turn.push(7, 8, 0, &frame).unwrap();
        let wav = turn.finish(7).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 640);
        assert_eq!(wav.len(), 684);
        assert_eq!(&wav[44..50], &[0, 128, 0, 0, 255, 127]);
    }

    #[test]
    fn rejected_frames_do_not_advance_or_mix_participants() {
        let mut turn = Utterance::new(7, 8);
        let frame = [1; FRAME_SAMPLES];
        assert_eq!(turn.push(6, 8, 0, &frame), Err(AudioError::StaleSession));
        assert_eq!(
            turn.push(7, 9, 0, &frame),
            Err(AudioError::WrongParticipant)
        );
        assert_eq!(turn.push(7, 8, 1, &frame), Err(AudioError::OutOfOrder));
        assert_eq!(
            turn.push(7, 8, 0, &frame[..319]),
            Err(AudioError::InvalidFrame)
        );
        turn.push(7, 8, 0, &frame).unwrap();
        assert_eq!(turn.push(7, 8, 0, &frame), Err(AudioError::OutOfOrder));
        assert_eq!(turn.finish(7).unwrap().len(), 684);
    }

    #[test]
    fn overflow_discards_instead_of_transcribing_a_truncated_turn() {
        let mut turn = Utterance::new(7, 8);
        for sequence in 0..MAX_FRAMES {
            turn.push(7, 8, sequence, &[0; FRAME_SAMPLES]).unwrap();
        }
        assert_eq!(
            turn.push(7, 8, MAX_FRAMES, &[0; FRAME_SAMPLES]),
            Err(AudioError::TooLong)
        );
        assert!(turn.samples.is_empty());
        assert_eq!(turn.finish(7), Err(AudioError::Stopped));
    }

    #[test]
    fn cancel_and_target_change_reject_late_audio() {
        let mut turn = Utterance::new(7, 8);
        turn.push(7, 8, 0, &[0; FRAME_SAMPLES]).unwrap();
        turn.cancel();
        assert_eq!(
            turn.push(7, 8, 1, &[0; FRAME_SAMPLES]),
            Err(AudioError::Stopped)
        );
        assert_eq!(turn.finish(7), Err(AudioError::Stopped));
        assert_eq!(Utterance::new(7, 8).finish(7), Err(AudioError::Empty));
        let mut turn = Utterance::new(7, 8);
        turn.push(7, 8, 0, &[0; FRAME_SAMPLES]).unwrap();
        assert_eq!(turn.finish(8), Err(AudioError::StaleSession));
    }
}
