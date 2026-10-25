//! Private host-to-extension call PCM. Parsing does not grant playback authority.
use super::*;

pub const MAX_OUTPUT_FRAMES: u32 = 65_536;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioOutput {
    Start {
        playback_id: Uuid,
        target: Target,
    },
    Frame {
        playback_id: Uuid,
        target: Target,
        frame_sequence: u32,
        pcm: Vec<i16>,
    },
    Finish {
        playback_id: Uuid,
        target: Target,
        frame_count: u32,
    },
    Cancel {
        playback_id: Uuid,
        target: Target,
    },
}
impl AudioOutput {
    pub fn target(&self) -> Target {
        match self {
            Self::Start { target, .. }
            | Self::Frame { target, .. }
            | Self::Finish { target, .. }
            | Self::Cancel { target, .. } => *target,
        }
    }
    pub fn validate(&self) -> Checked {
        let id = match self {
            Self::Start { playback_id, .. }
            | Self::Frame { playback_id, .. }
            | Self::Finish { playback_id, .. }
            | Self::Cancel { playback_id, .. } => *playback_id,
        };
        nonnil(id, "playback_id")?;
        self.target().validate()?;
        match self {
            Self::Frame {
                frame_sequence,
                pcm,
                ..
            } if *frame_sequence >= MAX_OUTPUT_FRAMES
                || pcm.len() != crate::call_pcm::CALL_FRAME_SAMPLES =>
            {
                invalid("audio output requires one bounded 20ms48kHz stereo PCM16 frame")
            }
            Self::Finish { frame_count, .. } if *frame_count > MAX_OUTPUT_FRAMES => {
                invalid("audio output frame count exceeds its limit")
            }
            _ => Ok(()),
        }
    }
}
/// Private worker feedback. Counts refer to adapter consumption, not audibility.
/// The host must correlate against its own route and sent-frame ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioPlaybackReceipt {
    Ready {
        playback_id: Uuid,
    },
    Consumed {
        playback_id: Uuid,
        frame_count: u32,
    },
    Finished {
        playback_id: Uuid,
        frame_count: u32,
        outcome: AudioPlaybackOutcome,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioPlaybackOutcome {
    Completed,
    Cancelled,
    Failed,
}
impl AudioPlaybackReceipt {
    pub fn playback_id(&self) -> Uuid {
        match self {
            Self::Ready { playback_id }
            | Self::Consumed { playback_id, .. }
            | Self::Finished { playback_id, .. } => *playback_id,
        }
    }
    pub fn validate(&self) -> Checked {
        nonnil(self.playback_id(), "playback_id")?;
        match self {
            Self::Consumed { frame_count, .. }
                if *frame_count == 0 || *frame_count > MAX_OUTPUT_FRAMES =>
            {
                invalid("Consumed frame count must be positive and bounded")
            }
            Self::Finished { frame_count, .. } if *frame_count > MAX_OUTPUT_FRAMES => {
                invalid("Finished frame count exceeds its limit")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target() -> Target {
        Target {
            uuid: Uuid::new_v4(),
            generation: 3,
        }
    }
    #[test]
    fn availability_is_strict_and_uses_the_separate_playback_grant() {
        let request = ActionEnvelope {
            api_version: API_VERSION,
            message_type: ActionEnvelopeType::Action,
            id: "available-1".into(),
            sequence: 1,
            session_id: Uuid::new_v4(),
            process_generation: 1,
            target: target(),
            action: Action::AudioPlaybackAvailable {},
        };
        assert_eq!(
            request.action.required_capability(),
            Capability::AudioPlayback
        );
        let line = encode_action_line(&request).unwrap();
        assert_eq!(parse_action_line(&line).unwrap(), request);
        let mut extra = serde_json::to_value(&request).unwrap();
        extra["action"]["ready"] = serde_json::json!(true);
        assert!(parse_action_line(&serde_json::to_vec(&extra).unwrap()).is_err());
    }
    #[test]
    fn playback_feedback_roundtrips_under_its_separate_grant() {
        let playback_id = Uuid::new_v4();
        for receipt in [
            AudioPlaybackReceipt::Ready { playback_id },
            AudioPlaybackReceipt::Consumed {
                playback_id,
                frame_count: 12,
            },
            AudioPlaybackReceipt::Finished {
                playback_id,
                frame_count: 12,
                outcome: AudioPlaybackOutcome::Completed,
            },
        ] {
            let action = Action::AudioPlaybackReceipt { receipt };
            assert_eq!(action.required_capability(), Capability::AudioPlayback);
            let request = ActionEnvelope {
                api_version: API_VERSION,
                message_type: ActionEnvelopeType::Action,
                id: "playback-1".into(),
                sequence: 1,
                session_id: Uuid::new_v4(),
                process_generation: 1,
                target: target(),
                action,
            };
            let wire = encode_action_line(&request).unwrap();
            assert_eq!(parse_action_line(&wire).unwrap(), request);
            let mut extra = serde_json::to_value(&request).unwrap();
            extra["action"]["receipt"]["filename"] = serde_json::json!("not-supported");
            assert!(parse_action_line(&serde_json::to_vec(&extra).unwrap()).is_err());
        }
    }
    #[test]
    fn playback_feedback_rejects_nil_ids_unbounded_counts_and_unknown_outcomes() {
        let playback_id = Uuid::new_v4();
        for receipt in [
            AudioPlaybackReceipt::Ready {
                playback_id: Uuid::nil(),
            },
            AudioPlaybackReceipt::Consumed {
                playback_id,
                frame_count: 0,
            },
            AudioPlaybackReceipt::Consumed {
                playback_id,
                frame_count: MAX_OUTPUT_FRAMES + 1,
            },
            AudioPlaybackReceipt::Finished {
                playback_id,
                frame_count: MAX_OUTPUT_FRAMES + 1,
                outcome: AudioPlaybackOutcome::Cancelled,
            },
        ] {
            assert!(receipt.validate().is_err());
        }
        assert!(
            AudioPlaybackReceipt::Finished {
                playback_id,
                frame_count: 0,
                outcome: AudioPlaybackOutcome::Cancelled
            }
            .validate()
            .is_ok()
        );
        assert!(
            serde_json::from_value::<AudioPlaybackReceipt>(serde_json::json!({
                "phase":"finished", "playback_id":playback_id, "frame_count":1, "outcome":"heard"
            }))
            .is_err()
        );
    }
    #[test]
    fn output_frame_roundtrip_is_bounded_and_strict() {
        let output = AudioOutput::Frame {
            playback_id: Uuid::new_v4(),
            target: target(),
            frame_sequence: 0,
            pcm: vec![i16::MIN; crate::call_pcm::CALL_FRAME_SAMPLES],
        };
        let envelope = HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: Uuid::new_v4(),
            sequence: 2,
            process_generation: 4,
            message: HostMessage::AudioOutput { output },
        };
        let wire = encode_host_line(&envelope).unwrap();
        assert!(wire.len() < MAX_LINE_BYTES);
        assert_eq!(parse_host_line(&wire).unwrap(), envelope);
        let mut value = serde_json::to_value(&envelope).unwrap();
        value["message"]["output"]["filename"] = serde_json::json!("not-a-file-api");
        assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    #[test]
    fn invalid_output_shapes_are_refused() {
        let id = Uuid::new_v4();
        let target = target();
        for output in [
            AudioOutput::Start {
                playback_id: Uuid::nil(),
                target,
            },
            AudioOutput::Frame {
                playback_id: id,
                target,
                frame_sequence: 0,
                pcm: vec![0; 320],
            },
            AudioOutput::Frame {
                playback_id: id,
                target,
                frame_sequence: MAX_OUTPUT_FRAMES,
                pcm: vec![0; crate::call_pcm::CALL_FRAME_SAMPLES],
            },
            AudioOutput::Finish {
                playback_id: id,
                target,
                frame_count: MAX_OUTPUT_FRAMES + 1,
            },
            AudioOutput::Cancel {
                playback_id: id,
                target: Target {
                    uuid: Uuid::nil(),
                    generation: 1,
                },
            },
        ] {
            assert!(output.validate().is_err());
        }
    }
}
