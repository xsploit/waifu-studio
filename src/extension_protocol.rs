//! Pure API-v1 JSONL descriptions and validation; no transport or live extension API.
//! Integration checkpoint schema; not a stable public API.
//!
//! Use parse_*_line / encode_*_line at protocol boundaries. Direct serde decoding
//! checks shape but does not replace semantic validation. A record is at most
//! 64 KiB excluding an optional LF/CRLF terminator; encoded records end in LF.
//! Example extension request (shown on multiple lines only for readability):
//! ```json
//! {"api_version":1,"type":"action","id":"request-1","sequence":1,
//!  "session_id":"00000000-0000-0000-0000-000000000001",
//!  "process_generation":2,
//!  "target":{"uuid":"00000000-0000-0000-0000-000000000002","generation":3},
//!  "action":{"type":"motion.play","motion_id":"org.example.pack/nod","mode":"one_shot"}}
//! ```
//! Session identity, host sequence, process generation and target generation are
//! distinct. This module preserves them, but owns no current-state/replay checks.
//! The host must authenticate the process/session, reject stale targets, enforce
//! granted capabilities and event namespace ownership, and look up motion IDs
//! in its registered catalog (never treat an ID as a path). It must check that
//! an expression exists on the target. Successful parsing authorizes no action.
//!
//! IntegrationOrigin occurs only in host messages. The host supplies attribution
//! from its installation identity, never from a request or an arbitrary chat role.

use crate::reply_affect::AffectEmotion;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, fmt};
use uuid::Uuid;

#[path = "extension_audio_output_protocol.rs"]
mod audio_output;
pub use audio_output::{
    AudioOutput, AudioPlaybackOutcome, AudioPlaybackReceipt, MAX_OUTPUT_FRAMES,
};

pub const API_VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 64 * 1024;
pub const MAX_ID_BYTES: usize = 128;
/// A manifest package ID (128), slash, and local content ID (64).
pub const MAX_REGISTERED_ID_BYTES: usize = 193;
pub const MAX_EVENT_NAME_BYTES: usize = 256;
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_CHAT_TEXT_BYTES: usize = 2048;
pub const MAX_REPLY_CHUNK_BYTES: usize = 4 * 1024;
pub const MAX_CAPABILITIES: usize = 9;
pub const MAX_EVENT_DATA_BYTES: usize = 8 * 1024;
pub const MAX_EVENT_DATA_DEPTH: usize = 16;
pub const MAX_EVENT_DATA_NODES: usize = 1024;
pub const MAX_EXPRESSION_DURATION_MS: u32 = 60_000;
pub const MAX_CATALOG_ENTRIES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    LineTooLong,
    InvalidJson(String),
    InvalidField(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineTooLong => write!(f, "JSONL record exceeds {MAX_LINE_BYTES} bytes"),
            Self::InvalidJson(message) => write!(f, "Invalid protocol JSON: {message}"),
            Self::InvalidField(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProtocolError {}

type Checked<T = ()> = Result<T, ProtocolError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub uuid: Uuid,
    /// Avatar/character incarnation, independent of the extension process.
    pub generation: u64,
}

impl Target {
    pub fn validate(&self) -> Checked {
        nonnil(self.uuid, "target.uuid")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "audio.playback")]
    AudioPlayback,
    #[serde(rename = "audio.transcribe")]
    AudioTranscribe,
    #[serde(rename = "motion.play")]
    MotionPlay,
    #[serde(rename = "expression.apply")]
    ExpressionApply,
    #[serde(rename = "message.deliver")]
    MessageDeliver,
    #[serde(rename = "event.emit")]
    EventEmit,
    #[serde(rename = "catalog.get")]
    CatalogGet,
    #[serde(rename = "lifecycle.observe")]
    LifecycleObserve,
    #[serde(rename = "reply.read")]
    ReplyRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionEnvelopeType {
    #[serde(rename = "action")]
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostEnvelopeType {
    #[serde(rename = "host")]
    Host,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionEnvelope {
    pub api_version: u32,
    #[serde(rename = "type")]
    pub message_type: ActionEnvelopeType,
    pub id: String,
    /// Positive monotonic request sequence within this session/process incarnation.
    /// Runtime replay tracking uses this sequence, not an unbounded set of IDs.
    pub sequence: u64,
    pub session_id: Uuid,
    pub process_generation: u64,
    /// Required even for catalog.get; there is no implicit current target.
    pub target: Target,
    pub action: Action,
}

/// Structured external chat candidate. Acceptance is not persistence or delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatMessage {
    /// Opaque source message identity, including Twitch chat IDs.
    pub source_id: String,
    pub author_id: String,
    pub author_name: String,
    pub text: String,
}

impl ChatMessage {
    pub fn validate(&self) -> Checked {
        identifier(&self.source_id, "source_id")?;
        identifier(&self.author_id, "author_id")?;
        text_field(&self.author_name, "author_name", MAX_ID_BYTES)?;
        if self.author_name.chars().any(char::is_control) {
            return invalid("chat author_name must contain no control characters");
        }
        if self.text.trim().is_empty()
            || self.text.len() > MAX_CHAT_TEXT_BYTES
            || self
                .text
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            return invalid(
                "chat text must be nonempty, at most 2048 bytes, without controls except newline, CR and tab",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Action {
    #[serde(rename = "audio.playback.available")]
    AudioPlaybackAvailable {},
    #[serde(rename = "audio.playback.receipt")]
    AudioPlaybackReceipt { receipt: AudioPlaybackReceipt },
    #[serde(rename = "audio.input")]
    AudioInput { input: AudioInput },
    #[serde(rename = "motion.play")]
    MotionPlay {
        /// Opaque registered catalog key, not a filename or URL.
        motion_id: String,
        mode: PlaybackMode,
    },
    #[serde(rename = "expression.apply")]
    ExpressionApply {
        name: String,
        strength: f32,
        duration_ms: u32,
    },
    #[serde(rename = "message.deliver")]
    MessageDeliver { text: String },
    #[serde(rename = "chat.offer")]
    ChatOffer { message: ChatMessage },
    #[serde(rename = "event.emit")]
    EventEmit { name: String, data: Value },
    #[serde(rename = "catalog.get")]
    CatalogGet {},
}

/// Bounded decoded participant audio, not arbitrary files, URLs or device input.
/// Outer action sequence/session/target checks apply to every record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", deny_unknown_fields)]
pub enum AudioInput {
    #[serde(rename = "start")]
    Start {
        turn_id: String,
        participant_id: String,
        participant_name: String,
    },
    #[serde(rename = "frame")]
    Frame {
        turn_id: String,
        frame_sequence: u64,
        pcm: Vec<i16>,
    },
    #[serde(rename = "finish")]
    Finish { turn_id: String },
    #[serde(rename = "cancel")]
    Cancel { turn_id: String },
}

impl AudioInput {
    pub fn validate(&self) -> Checked {
        let turn_id = match self {
            Self::Start {
                turn_id,
                participant_id,
                participant_name,
            } => {
                ChatMessage {
                    source_id: turn_id.clone(),
                    author_id: participant_id.clone(),
                    author_name: participant_name.clone(),
                    text: "validation".into(),
                }
                .validate()?;
                turn_id
            }
            Self::Frame {
                turn_id,
                frame_sequence,
                pcm,
            } => {
                if *frame_sequence >= crate::discord_audio::MAX_FRAMES
                    || pcm.len() != crate::discord_audio::FRAME_SAMPLES
                {
                    return invalid(
                        "audio frame must contain 320 mono16kHz PCM16 samples within thirty seconds",
                    );
                }
                turn_id
            }
            Self::Finish { turn_id } | Self::Cancel { turn_id } => turn_id,
        };
        identifier(turn_id, "turn_id")
    }
}

#[cfg(test)]
mod audio_tests {
    use super::*;

    #[test]
    fn audio_frame_roundtrip_stays_inside_existing_jsonl_limit() {
        let input = AudioInput::Frame {
            turn_id: "turn".into(),
            frame_sequence: 1499,
            pcm: vec![i16::MIN; 320],
        };
        input.validate().unwrap();
        let action = Action::AudioInput { input };
        let json = serde_json::to_vec(&action).unwrap();
        assert!(json.len() < 4096);
        let decoded: Action = serde_json::from_slice(&json).unwrap();
        assert_eq!(action, decoded);
        assert_eq!(decoded.required_capability(), Capability::AudioTranscribe);
    }

    #[test]
    fn malformed_audio_and_unknown_fields_are_rejected() {
        for (sequence, count) in [(0, 319), (0, 321), (1500, 320)] {
            assert!(
                AudioInput::Frame {
                    turn_id: "turn".into(),
                    frame_sequence: sequence,
                    pcm: vec![0; count]
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            AudioInput::Start {
                turn_id: "turn".into(),
                participant_id: "viewer".into(),
                participant_name: "bad\nname".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            AudioInput::Cancel { turn_id: "".into() }
                .validate()
                .is_err()
        );
        assert!(
            serde_json::from_value::<AudioInput>(
                serde_json::json!({"phase":"cancel","turn_id":"turn","filename":"/tmp/audio.wav"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<AudioInput>(serde_json::json!({"phase":"frame","turn_id":"turn","frame_sequence":0,"pcm":[32768]})).is_err());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackMode {
    #[serde(rename = "one_shot")]
    OneShot,
}

impl Action {
    pub const fn required_capability(&self) -> Capability {
        match self {
            Self::AudioPlaybackAvailable {} | Self::AudioPlaybackReceipt { .. } => {
                Capability::AudioPlayback
            }
            Self::AudioInput { .. } => Capability::AudioTranscribe,
            Self::MotionPlay { .. } => Capability::MotionPlay,
            Self::ExpressionApply { .. } => Capability::ExpressionApply,
            Self::MessageDeliver { .. } | Self::ChatOffer { .. } => Capability::MessageDeliver,
            Self::EventEmit { .. } => Capability::EventEmit,
            Self::CatalogGet {} => Capability::CatalogGet,
        }
    }

    pub fn validate(&self) -> Checked {
        match self {
            Self::AudioPlaybackAvailable {} => Ok(()),
            Self::AudioPlaybackReceipt { receipt } => receipt.validate(),
            Self::AudioInput { input } => input.validate(),
            Self::MotionPlay { motion_id, .. } => {
                identifier_limit(motion_id, "motion_id", MAX_REGISTERED_ID_BYTES)
            }
            Self::ExpressionApply {
                name,
                strength,
                duration_ms,
            } => {
                text_field(name, "expression name", MAX_ID_BYTES)?;
                if !strength.is_finite() || !(0.0..=1.0).contains(strength) {
                    return invalid("strength must be finite and between 0 and 1");
                }
                if !(1..=MAX_EXPRESSION_DURATION_MS).contains(duration_ms) {
                    return invalid("duration_ms must be between 1 and 60000");
                }
                Ok(())
            }
            Self::MessageDeliver { text } => text_field(text, "text", MAX_TEXT_BYTES),
            Self::ChatOffer { message } => message.validate(),
            Self::EventEmit { name, data } => {
                event_name(name)?;
                event_data(data)
            }
            Self::CatalogGet {} => Ok(()),
        }
    }
}

impl ActionEnvelope {
    pub fn validate(&self) -> Checked {
        version(self.api_version)?;
        identifier(&self.id, "id")?;
        if self.sequence == 0 {
            return invalid("sequence must be greater than zero");
        }
        nonnil(self.session_id, "session_id")?;
        self.target.validate()?;
        self.action.validate()
    }
}

/// Host-authored attribution. This type is never part of an extension action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrigin {
    pub integration_id: String,
    pub display_name: String,
}

impl IntegrationOrigin {
    pub fn validate(&self) -> Checked {
        identifier(&self.integration_id, "integration_id")?;
        text_field(&self.display_name, "integration display_name", MAX_ID_BYTES)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEnvelope {
    pub api_version: u32,
    #[serde(rename = "type")]
    pub message_type: HostEnvelopeType,
    pub session_id: Uuid,
    /// Host-assigned sequence within the session. Ordering/replay is host policy.
    pub sequence: u64,
    /// Extension process incarnation, not a target/avatar generation.
    pub process_generation: u64,
    pub message: HostMessage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum HostMessage {
    #[serde(rename = "handshake")]
    Handshake {
        origin: IntegrationOrigin,
        #[serde(default)]
        settings: crate::extension_settings::Settings,
        /// Capabilities actually granted by the host, not requested by a child.
        capabilities: Vec<Capability>,
    },
    #[serde(rename = "result")]
    Result {
        id: String,
        target: Target,
        result: ActionResult,
    },
    #[serde(rename = "event")]
    Event { event: HostEvent },
    #[serde(rename = "audio.output")]
    AudioOutput { output: AudioOutput },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", deny_unknown_fields)]
pub enum ActionResult {
    #[serde(rename = "ok")]
    Ok { value: ResultValue },
    #[serde(rename = "error")]
    Error { code: ErrorCode, message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ResultValue {
    /// Acknowledgement only; not evidence of visual playback/delivery completion.
    #[serde(rename = "accepted")]
    Accepted {},
    #[serde(rename = "catalog")]
    Catalog {
        motions: Vec<RegisteredMotion>,
        expressions: Vec<RegisteredExpression>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredMotion {
    pub motion_id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredExpression {
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    PermissionDenied,
    StaleSession,
    StaleProcess,
    StaleTarget,
    UnknownMotion,
    UnknownExpression,
    Busy,
    Unavailable,
    InternalError,
}

/// Observed reply terminal state, not an inference from avatar selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// Retained terminal playback receipt for a correlated host action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionOutcome {
    Completed,
    Cancelled,
    Failed,
}

/// Queued is external-candidate admission, not history delivery or a spoken reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioInputOutcome {
    Queued,
    Failed,
    Cancelled,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechStopReason {
    ObservedTerminal,
    Cancelled,
}

/// Original affect owner, independent of the selected or loaded avatar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmotionOwner {
    Studio,
    SceneChat,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum HostEvent {
    #[serde(rename = "audio.input.finished")]
    AudioInputFinished {
        target: Target,
        turn_id: String,
        outcome: AudioInputOutcome,
    },
    #[serde(rename = "motion.playback.started")]
    MotionPlaybackStarted {
        playback_id: Uuid,
        source_target: Option<Target>,
        motion_id: Option<String>,
        source: MotionPlaybackSource,
    },
    #[serde(rename = "motion.playback.ended")]
    MotionPlaybackEnded {
        playback_id: Uuid,
        source_target: Option<Target>,
        motion_id: Option<String>,
        source: MotionPlaybackSource,
        outcome: MotionOutcome,
    },

    #[serde(rename = "extension.stopping")]
    ExtensionStopping {
        installation_id: Uuid,
        session_id: Uuid,
        process_generation: u64,
        reason: ExtensionStopReason,
    },
    #[serde(rename = "extension.stopped")]
    ExtensionStopped {
        installation_id: Uuid,
        session_id: Uuid,
        process_generation: u64,
        exit_code: Option<i32>,
        requested_reason: Option<ExtensionStopReason>,
    },
    #[serde(rename = "extension.started")]
    ExtensionStarted {
        installation_id: Uuid,
        session_id: Uuid,
        process_generation: u64,
    },
    /// Best-effort teardown metadata for an existing persistent observer only.
    #[serde(rename = "app.stopping")]
    AppStopping { app_run_id: Uuid },
    #[serde(rename = "app.ready")]
    AppReady { app_run_id: Uuid },
    /// Source selection revision; neither UUID grants loaded-avatar authority.
    #[serde(rename = "character.selected")]
    CharacterSelected {
        previous: Option<Uuid>,
        new: Option<Uuid>,
        selection_revision: u64,
    },
    /// Source ownership only; character is never the current target incarnation.
    #[serde(rename = "reply.finished")]
    ReplyFinished {
        reply_id: Uuid,
        character: Option<Uuid>,
        outcome: ReplyOutcome,
    },
    #[serde(rename = "reply.text")]
    ReplyText {
        reply_id: Uuid,
        character: Option<Uuid>,
        /// UTF-8 byte offset in the original full reply text.
        offset: u64,
        text: String,
    },
    #[serde(rename = "speech.started")]
    SpeechStarted {
        playback_id: Uuid,
        character: Option<Uuid>,
        voice_generation: u64,
        reply_id: Option<Uuid>,
    },
    #[serde(rename = "speech.stopped")]
    SpeechStopped {
        playback_id: Uuid,
        character: Option<Uuid>,
        voice_generation: u64,
        reply_id: Option<Uuid>,
        reason: SpeechStopReason,
    },
    /// Source identity only; this event does not supply a current action target.
    #[serde(rename = "emotion.changed")]
    EmotionChanged {
        character: Uuid,
        owner: EmotionOwner,
        previous: AffectEmotion,
        emotion: AffectEmotion,
        reply_id: Option<Uuid>,
    },
    #[serde(rename = "target.changed")]
    TargetChanged { target: Option<Target> },
    /// Original action target and correlation; never inferred from the current
    /// selected avatar or from playback no longer being active.
    #[serde(rename = "motion.finished")]
    MotionFinished {
        target: Target,
        motion_id: String,
        action_id: String,
        outcome: MotionOutcome,
    },
    #[serde(rename = "message.delivered")]
    MessageDelivered {
        target: Target,
        text: String,
        origin: IntegrationOrigin,
    },
    #[serde(rename = "event.emitted")]
    Emitted {
        target: Target,
        name: String,
        data: Value,
        origin: IntegrationOrigin,
    },
}

impl HostEnvelope {
    pub fn validate(&self) -> Checked {
        version(self.api_version)?;
        nonnil(self.session_id, "session_id")?;
        match &self.message {
            HostMessage::Handshake {
                origin,
                capabilities,
                settings,
            } => {
                crate::extension_settings::validate_wire_values(settings)
                    .map_err(ProtocolError::InvalidField)?;
                origin.validate()?;
                if capabilities.len() > MAX_CAPABILITIES {
                    return invalid("handshake exceeds the supported capability limit");
                }
                let unique: BTreeSet<_> = capabilities.iter().collect();
                if unique.len() != capabilities.len() {
                    return invalid("handshake capabilities must be unique");
                }
                Ok(())
            }
            HostMessage::Result { id, target, result } => {
                identifier(id, "result id")?;
                target.validate()?;
                result.validate()
            }
            HostMessage::Event { event } => event.validate(),
            HostMessage::AudioOutput { output } => output.validate(),
        }
    }
}

impl ActionResult {
    pub fn validate(&self) -> Checked {
        match self {
            Self::Error { message, .. } => text_field(message, "error message", MAX_TEXT_BYTES),
            Self::Ok { value } => match value {
                ResultValue::Accepted {} => Ok(()),
                ResultValue::Catalog {
                    motions,
                    expressions,
                } => {
                    if motions.len().saturating_add(expressions.len()) > MAX_CATALOG_ENTRIES {
                        return invalid(
                            "catalog exceeds 128 total entries; host must bound its view",
                        );
                    }
                    let mut motion_ids = BTreeSet::new();
                    for motion in motions {
                        identifier_limit(
                            &motion.motion_id,
                            "catalog motion_id",
                            MAX_REGISTERED_ID_BYTES,
                        )?;
                        text_field(&motion.name, "catalog motion name", MAX_ID_BYTES)?;
                        if !motion_ids.insert(&motion.motion_id) {
                            return invalid("duplicate catalog motion_id");
                        }
                    }
                    let mut expression_names = BTreeSet::new();
                    for expression in expressions {
                        text_field(&expression.name, "catalog expression name", MAX_ID_BYTES)?;
                        if !expression_names.insert(&expression.name) {
                            return invalid("duplicate catalog expression name");
                        }
                    }
                    Ok(())
                }
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionStopReason {
    Disabled,
    Removed,
    Restarted,
    HostShutdown,
    Reconfigured,
}

/// Host-authenticated attribution captured at playback admission; never a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MotionPlaybackSource {
    Host,
    Extension {
        integration_id: String,
        session_id: Uuid,
        process_generation: u64,
        action_id: String,
    },
}

impl HostEvent {
    pub fn validate(&self) -> Checked {
        match self {
            Self::AudioInputFinished {
                target, turn_id, ..
            } => {
                target.validate()?;
                identifier(turn_id, "turn_id")
            }
            Self::MotionPlaybackStarted {
                playback_id,
                source_target,
                motion_id,
                source,
            }
            | Self::MotionPlaybackEnded {
                playback_id,
                source_target,
                motion_id,
                source,
                ..
            } => {
                nonnil(*playback_id, "playback_id")?;
                if let Some(target) = source_target {
                    target.validate()?;
                }
                if let Some(id) = motion_id {
                    identifier_limit(id, "motion_id", MAX_REGISTERED_ID_BYTES)?;
                }
                if let MotionPlaybackSource::Extension {
                    integration_id,
                    session_id,
                    process_generation,
                    action_id,
                } = source
                {
                    identifier(integration_id, "integration_id")?;
                    nonnil(*session_id, "session_id")?;
                    if *process_generation == 0 {
                        return invalid("process_generation must be positive");
                    }
                    identifier(action_id, "action_id")?;
                }
                Ok(())
            }
            Self::ExtensionStarted {
                installation_id,
                session_id,
                process_generation,
                ..
            }
            | Self::ExtensionStopping {
                installation_id,
                session_id,
                process_generation,
                ..
            }
            | Self::ExtensionStopped {
                installation_id,
                session_id,
                process_generation,
                ..
            } => {
                nonnil(*installation_id, "installation_id")?;
                nonnil(*session_id, "session_id")?;
                if *process_generation == 0 {
                    return invalid("process_generation must be positive");
                }
                Ok(())
            }
            Self::AppReady { app_run_id } | Self::AppStopping { app_run_id } => {
                nonnil(*app_run_id, "app_run_id")
            }
            Self::CharacterSelected {
                previous,
                new,
                selection_revision,
            } => {
                optional_uuid(*previous, "previous")?;
                optional_uuid(*new, "new")?;
                if previous == new || *selection_revision == 0 {
                    return invalid("selection must change identity and have a positive revision");
                }
                Ok(())
            }
            Self::ReplyFinished {
                reply_id,
                character,
                ..
            } => {
                nonnil(*reply_id, "reply_id")?;
                optional_uuid(*character, "character")
            }
            Self::ReplyText {
                reply_id,
                character,
                offset,
                text,
            } => {
                nonnil(*reply_id, "reply_id")?;
                optional_uuid(*character, "character")?;
                // Preserve whitespace and all source text; unlike action text,
                // chunks are not independently meaningful user messages.
                if text.is_empty() || text.len() > MAX_REPLY_CHUNK_BYTES {
                    return invalid("reply text chunk must contain 1..4096 UTF-8 bytes");
                }
                if offset.checked_add(text.len() as u64).is_none() {
                    return invalid("reply text byte offset overflows");
                }
                Ok(())
            }
            Self::SpeechStarted {
                playback_id,
                character,
                reply_id,
                ..
            }
            | Self::SpeechStopped {
                playback_id,
                character,
                reply_id,
                ..
            } => {
                nonnil(*playback_id, "playback_id")?;
                optional_uuid(*character, "character")?;
                optional_uuid(*reply_id, "reply_id")
            }
            Self::TargetChanged { target } => {
                if let Some(target) = target {
                    target.validate()?;
                }
                Ok(())
            }
            Self::EmotionChanged {
                character,
                reply_id,
                ..
            } => {
                nonnil(*character, "character")?;
                optional_uuid(*reply_id, "reply_id")
            }
            Self::MotionFinished {
                target,
                motion_id,
                action_id,
                ..
            } => {
                target.validate()?;
                identifier(action_id, "action_id")?;
                identifier_limit(motion_id, "motion_id", MAX_REGISTERED_ID_BYTES)
            }
            Self::MessageDelivered {
                target,
                text,
                origin,
            } => {
                target.validate()?;
                text_field(text, "text", MAX_TEXT_BYTES)?;
                origin.validate()
            }
            Self::Emitted {
                target,
                name,
                data,
                origin,
            } => {
                target.validate()?;
                event_name(name)?;
                event_data(data)?;
                origin.validate()
            }
        }
    }
}

#[cfg(test)]
mod selection_wire_tests {
    use super::*;

    #[test]
    fn extension_started_wire_preserves_original_incarnation_and_validates_identity() {
        let installation_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let event = HostEvent::ExtensionStarted {
            installation_id,
            session_id,
            process_generation: 7,
        };
        event.validate().unwrap();
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"extension.started", "installation_id":installation_id,
            "session_id":session_id, "process_generation":7})
        );
        assert_eq!(serde_json::from_value::<HostEvent>(value).unwrap(), event);
        for (installation_id, session_id, process_generation) in [
            (Uuid::nil(), session_id, 1),
            (installation_id, Uuid::nil(), 1),
            (installation_id, session_id, 0),
        ] {
            assert!(
                HostEvent::ExtensionStarted {
                    installation_id,
                    session_id,
                    process_generation
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn app_stopping_wire_is_only_a_nonnil_run_identity_and_not_a_hook() {
        let app_run_id = Uuid::new_v4();
        let event = HostEvent::AppStopping { app_run_id };
        event.validate().unwrap();
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"app.stopping", "app_run_id":app_run_id})
        );
        assert_eq!(serde_json::from_value::<HostEvent>(value).unwrap(), event);
        assert!(
            HostEvent::AppStopping {
                app_run_id: Uuid::nil()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::extension_hooks::HookTrigger>(serde_json::json!(
                "app.stopping"
            ))
            .is_err()
        );
    }

    #[test]
    fn app_ready_wire_contains_only_a_nonnil_run_identity() {
        let app_run_id = Uuid::new_v4();
        let event = HostEvent::AppReady { app_run_id };
        event.validate().unwrap();
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"app.ready", "app_run_id":app_run_id})
        );
        assert_eq!(serde_json::from_value::<HostEvent>(value).unwrap(), event);
        assert!(
            HostEvent::AppReady {
                app_run_id: Uuid::nil()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_value::<HostEvent>(serde_json::json!({
                "type":"app.ready", "app_run_id":app_run_id, "target":null,
            }))
            .is_err()
        );
    }

    #[test]
    fn selection_wire_is_nullable_metadata_with_positive_revision() {
        let id = Uuid::from_u128(7);
        for (previous, new) in [(None, Some(id)), (Some(id), None)] {
            let event = HostEvent::CharacterSelected {
                previous,
                new,
                selection_revision: 9,
            };
            event.validate().unwrap();
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(
                value,
                serde_json::json!({
                    "type": "character.selected", "previous": previous, "new": new,
                    "selection_revision": 9,
                })
            );
            assert_eq!(serde_json::from_value::<HostEvent>(value).unwrap(), event);
        }
        for event in [
            HostEvent::CharacterSelected {
                previous: None,
                new: None,
                selection_revision: 1,
            },
            HostEvent::CharacterSelected {
                previous: None,
                new: Some(id),
                selection_revision: 0,
            },
            HostEvent::CharacterSelected {
                previous: None,
                new: Some(Uuid::nil()),
                selection_revision: 1,
            },
        ] {
            assert!(event.validate().is_err());
        }
        assert!(
            serde_json::from_value::<HostEvent>(serde_json::json!({
                "type": "character.selected", "previous": null, "new": id,
                "selection_revision": 1, "target": {"uuid": id, "generation": 1},
            }))
            .is_err()
        );
    }
}

fn optional_uuid(value: Option<Uuid>, field: &str) -> Checked {
    match value {
        Some(value) => nonnil(value, field),
        None => Ok(()),
    }
}

/// Lazily produce the full text followed by its terminal metadata. Each next()
/// allocates at most one 4 KiB chunk; no event vector or connection queue expands
/// with reply length. Empty replies still produce ReplyFinished. Offsets are
/// UTF-8 bytes. The caller must budget iteration/publication per frame and apply
/// ReplyRead vs LifecycleObserve independently; do not collect this iterator.
/// IDs are host-owned and validated at the normal protocol boundary.
pub fn reply_events(
    reply_id: Uuid,
    character: Option<Uuid>,
    text: &str,
    outcome: ReplyOutcome,
) -> impl Iterator<Item = HostEvent> + '_ {
    let mut offset = 0usize;
    std::iter::from_fn(move || {
        if offset == text.len() {
            return None;
        }
        let mut end = offset.saturating_add(MAX_REPLY_CHUNK_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let event = HostEvent::ReplyText {
            reply_id,
            character,
            offset: offset as u64,
            text: text[offset..end].to_owned(),
        };
        offset = end;
        Some(event)
    })
    .chain(std::iter::once(HostEvent::ReplyFinished {
        reply_id,
        character,
        outcome,
    }))
}

pub fn parse_action_line(line: &[u8]) -> Checked<ActionEnvelope> {
    let message: ActionEnvelope = parse_record(line)?;
    message.validate()?;
    Ok(message)
}

pub fn encode_action_line(message: &ActionEnvelope) -> Checked<Vec<u8>> {
    // Validate first: serde_json would encode nonfinite floats as null.
    message.validate()?;
    encode_record(message)
}

pub fn parse_host_line(line: &[u8]) -> Checked<HostEnvelope> {
    let message: HostEnvelope = parse_record(line)?;
    message.validate()?;
    Ok(message)
}

pub fn encode_host_line(message: &HostEnvelope) -> Checked<Vec<u8>> {
    message.validate()?;
    encode_record(message)
}

fn parse_record<T: serde::de::DeserializeOwned>(line: &[u8]) -> Checked<T> {
    let record = line
        .strip_suffix(b"\r\n")
        .or_else(|| line.strip_suffix(b"\n"))
        .unwrap_or(line);
    if record.len() > MAX_LINE_BYTES {
        return Err(ProtocolError::LineTooLong);
    }
    if record.is_empty() || record.contains(&b'\n') || record.contains(&b'\r') {
        return invalid("expected exactly one nonempty JSONL record");
    }
    serde_json::from_slice(record).map_err(|e| ProtocolError::InvalidJson(e.to_string()))
}

fn encode_record<T: Serialize>(message: &T) -> Checked<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec(message).map_err(|e| ProtocolError::InvalidJson(e.to_string()))?;
    if bytes.len() > MAX_LINE_BYTES {
        return Err(ProtocolError::LineTooLong);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

fn version(api_version: u32) -> Checked {
    if api_version != API_VERSION {
        return invalid("unsupported api_version; expected exactly 1");
    }
    Ok(())
}

fn nonnil(uuid: Uuid, field: &str) -> Checked {
    if uuid.is_nil() {
        return invalid(&format!("{field} must not be the nil UUID"));
    }
    Ok(())
}

fn text_field(value: &str, field: &str, max: usize) -> Checked {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return invalid(&format!(
            "{field} must be nonempty, without NUL, at most {max} bytes"
        ));
    }
    Ok(())
}

fn identifier(value: &str, field: &str) -> Checked {
    identifier_limit(value, field, MAX_ID_BYTES)
}
fn identifier_limit(value: &str, field: &str, limit: usize) -> Checked {
    text_field(value, field, limit)?;
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return invalid(&format!(
            "{field} must contain no whitespace or control characters"
        ));
    }
    Ok(())
}

/// Lowercase dotted namespace, e.g. org.example.integration.notice. Namespace
/// ownership is host policy; syntactic qualification alone grants no authority.
fn event_name(name: &str) -> Checked {
    identifier_limit(name, "event name", MAX_EVENT_NAME_BYTES)?;
    if !name.contains('.')
        || !name.split('.').all(|segment| {
            !segment.is_empty()
                && segment.as_bytes()[0].is_ascii_alphanumeric()
                && segment
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
        })
    {
        return invalid("event name must use a dotted lowercase namespace");
    }
    Ok(())
}

fn event_data(value: &Value) -> Checked {
    // Bound programmatically constructed Values before serializing them. The
    // raw-byte/node budget also bounds temporary escaping expansion (at most 6x).
    fn visit(value: &Value, depth: usize, nodes: &mut usize, raw: &mut usize) -> Checked {
        if depth > MAX_EVENT_DATA_DEPTH || *nodes == 0 || *raw == 0 {
            return invalid("event data exceeds depth, node or byte budget");
        }
        *nodes -= 1;
        *raw -= 1;
        match value {
            Value::String(s) => {
                *raw = raw.checked_sub(s.len()).ok_or_else(|| {
                    ProtocolError::InvalidField("event data byte budget exceeded".into())
                })?;
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, depth + 1, nodes, raw)?;
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    // Account for keys separately from value nodes.
                    *raw = raw.checked_sub(key.len()).ok_or_else(|| {
                        ProtocolError::InvalidField("event data byte budget exceeded".into())
                    })?;
                    visit(item, depth + 1, nodes, raw)?;
                }
            }
            // serde_json::Number cannot represent NaN or infinity.
            Value::Number(_) | Value::Bool(_) | Value::Null => {}
        }
        Ok(())
    }
    let mut nodes = MAX_EVENT_DATA_NODES;
    let mut raw = MAX_EVENT_DATA_BYTES;
    visit(value, 0, &mut nodes, &mut raw)?;
    let bytes = serde_json::to_vec(value).map_err(|e| ProtocolError::InvalidJson(e.to_string()))?;
    if bytes.len() > MAX_EVENT_DATA_BYTES {
        return invalid("serialized event data exceeds 8192 bytes");
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Checked<T> {
    Err(ProtocolError::InvalidField(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat() -> ChatMessage {
        ChatMessage {
            source_id: "twitch-chat-id:opaque/123".into(),
            author_id: "12345".into(),
            author_name: "Viewer Name".into(),
            text: "Hello 🦀".into(),
        }
    }

    #[test]
    fn chat_offer_roundtrip_bounds_and_opaque_ids() {
        let mut message = chat();
        message.source_id = "x".repeat(128);
        message.author_id = "é".repeat(64);
        message.author_name = "🦀".repeat(32);
        message.text = "🦀".repeat(512);
        let action = Action::ChatOffer {
            message: message.clone(),
        };
        assert_eq!(action.required_capability(), Capability::MessageDeliver);
        let envelope = request(action);
        let encoded = encode_action_line(&envelope).unwrap();
        assert_eq!(parse_action_line(&encoded).unwrap(), envelope);
        for field in ["source_id", "author_id", "author_name", "text"] {
            let mut value = serde_json::to_value(&message).unwrap();
            value[field] = json!(format!("{}x", value[field].as_str().unwrap()));
            assert!(
                serde_json::from_value::<ChatMessage>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
            for bad in ["", "\n", "hello\u{7f}"] {
                let mut value = serde_json::to_value(chat()).unwrap();
                value[field] = json!(bad);
                assert!(
                    serde_json::from_value::<ChatMessage>(value)
                        .unwrap()
                        .validate()
                        .is_err()
                );
            }
        }
        for field in ["source_id", "author_id"] {
            for bad in ["a b", "a\u{2003}b"] {
                let mut value = serde_json::to_value(chat()).unwrap();
                value[field] = json!(bad);
                assert!(
                    serde_json::from_value::<ChatMessage>(value)
                        .unwrap()
                        .validate()
                        .is_err()
                );
            }
        }
        chat().validate().unwrap();
    }

    #[test]
    fn chat_offer_preserves_raw_unicode_newlines_and_quotes() {
        let mut message = chat();
        message.text = "🦀 日本語\n\"quoted\"\r\tmore".into();
        let envelope = request(Action::ChatOffer { message });
        let encoded = encode_action_line(&envelope).unwrap();
        assert_eq!(encoded.iter().filter(|&&byte| byte == b'\n').count(), 1);
        assert_eq!(parse_action_line(&encoded).unwrap(), envelope);
        for bad in ["\n\r\t ", "hi\0", "hi\u{1}", "hi\u{85}"] {
            let mut message = chat();
            message.text = bad.into();
            assert!(message.validate().is_err());
        }
    }

    #[test]
    fn chat_offer_rejects_wrong_types_missing_and_unknown_fields() {
        let base = serde_json::to_value(request(Action::ChatOffer { message: chat() })).unwrap();
        for field in ["source_id", "author_id", "author_name", "text"] {
            for bad in [json!(null), json!(42), json!([]), json!({}), json!(true)] {
                let mut value = base.clone();
                value["action"]["message"][field] = bad;
                assert!(parse_value(&value).is_err());
            }
            let mut value = base.clone();
            value["action"]["message"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(parse_value(&value).is_err());
        }
        for nested in [true, false] {
            let mut value = base.clone();
            if nested {
                value["action"]["message"]["extra"] = json!("no");
            } else {
                value["action"]["extra"] = json!("no");
            }
            assert!(parse_value(&value).is_err());
        }
    }

    fn target() -> Target {
        Target {
            uuid: Uuid::from_u128(2),
            generation: 19,
        }
    }

    fn request(action: Action) -> ActionEnvelope {
        ActionEnvelope {
            api_version: API_VERSION,
            message_type: ActionEnvelopeType::Action,
            id: "request-1".into(),
            sequence: 1,
            session_id: Uuid::from_u128(1),
            process_generation: 7,
            target: target(),
            action,
        }
    }

    fn motion() -> Action {
        Action::MotionPlay {
            motion_id: "org.example.pack/nod".into(),
            mode: PlaybackMode::OneShot,
        }
    }

    fn origin() -> IntegrationOrigin {
        IntegrationOrigin {
            integration_id: "org.example.integration".into(),
            display_name: "Example integration".into(),
        }
    }

    fn host(message: HostMessage) -> HostEnvelope {
        HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: Uuid::from_u128(1),
            sequence: 11,
            process_generation: 7,
            message,
        }
    }

    fn parse_value(value: &Value) -> Checked<ActionEnvelope> {
        parse_action_line(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn lifecycle_roundtrips_source_ownership_and_all_handshake_grants() {
        let reply_id = Uuid::from_u128(41);
        let playback_id = Uuid::from_u128(42);
        let character = Some(Uuid::from_u128(43));
        for event in [
            HostEvent::ReplyFinished {
                reply_id,
                character,
                outcome: ReplyOutcome::Completed,
            },
            HostEvent::ReplyFinished {
                reply_id,
                character: None,
                outcome: ReplyOutcome::Failed,
            },
            HostEvent::ReplyFinished {
                reply_id,
                character,
                outcome: ReplyOutcome::Cancelled,
            },
            HostEvent::ReplyText {
                reply_id,
                character,
                offset: 0,
                text: "  ".into(),
            },
            HostEvent::SpeechStarted {
                playback_id,
                character,
                voice_generation: 0,
                reply_id: None,
            },
            HostEvent::SpeechStopped {
                playback_id,
                character: None,
                voice_generation: 99,
                reply_id: Some(reply_id),
                reason: SpeechStopReason::ObservedTerminal,
            },
            HostEvent::SpeechStopped {
                playback_id,
                character,
                voice_generation: 100,
                reply_id: Some(reply_id),
                reason: SpeechStopReason::Cancelled,
            },
        ] {
            let envelope = host(HostMessage::Event { event });
            let line = encode_host_line(&envelope).unwrap();
            assert_eq!(parse_host_line(&line).unwrap(), envelope);
            let wire: Value = serde_json::from_slice(&line).unwrap();
            assert!(wire["message"]["event"].get("target").is_none());
        }
        let handshake = host(HostMessage::Handshake {
            settings: Default::default(),
            origin: origin(),
            capabilities: vec![
                Capability::MotionPlay,
                Capability::ExpressionApply,
                Capability::MessageDeliver,
                Capability::EventEmit,
                Capability::CatalogGet,
                Capability::LifecycleObserve,
                Capability::ReplyRead,
            ],
        });
        assert_eq!(
            parse_host_line(&encode_host_line(&handshake).unwrap()).unwrap(),
            handshake
        );
    }

    #[test]
    fn emotion_events_roundtrip_both_source_owners_without_target() {
        for (owner, name) in [
            (EmotionOwner::Studio, "studio"),
            (EmotionOwner::SceneChat, "scene_chat"),
        ] {
            for reply_id in [None, Some(Uuid::from_u128(44))] {
                let envelope = host(HostMessage::Event {
                    event: HostEvent::EmotionChanged {
                        character: Uuid::from_u128(43),
                        owner,
                        previous: AffectEmotion::Neutral,
                        emotion: AffectEmotion::Happy,
                        reply_id,
                    },
                });
                let line = encode_host_line(&envelope).unwrap();
                assert_eq!(parse_host_line(&line).unwrap(), envelope);
                let wire: Value = serde_json::from_slice(&line).unwrap();
                let event = &wire["message"]["event"];
                assert_eq!(event["type"], "emotion.changed");
                assert_eq!(event["owner"], name);
                assert_eq!(event["previous"], "neutral");
                assert_eq!(event["emotion"], "happy");
                assert!(event.get("target").is_none());
            }
        }
    }

    #[test]
    fn emotion_events_reject_invalid_source_ids_values_and_target_claims() {
        let base = serde_json::to_value(host(HostMessage::Event {
            event: HostEvent::EmotionChanged {
                character: Uuid::from_u128(43),
                owner: EmotionOwner::Studio,
                previous: AffectEmotion::Neutral,
                emotion: AffectEmotion::Happy,
                reply_id: Some(Uuid::from_u128(44)),
            },
        }))
        .unwrap();
        for (field, value) in [
            ("character", json!(Uuid::nil())),
            ("character", json!("not-a-uuid")),
            ("character", Value::Null),
            ("reply_id", json!(Uuid::nil())),
            ("reply_id", json!("not-a-uuid")),
            ("owner", json!("scenechat")),
            ("previous", json!("unknown")),
            ("emotion", json!("unknown")),
            (
                "target",
                json!({"uuid": Uuid::from_u128(43), "generation": 1}),
            ),
        ] {
            let mut wire = base.clone();
            wire["message"]["event"][field] = value;
            assert!(
                parse_host_line(&serde_json::to_vec(&wire).unwrap()).is_err(),
                "accepted {field}"
            );
        }
    }

    #[test]
    fn motion_receipts_roundtrip_correlation_target_and_all_terminal_outcomes() {
        for (outcome, wire_name) in [
            (MotionOutcome::Completed, "completed"),
            (MotionOutcome::Cancelled, "cancelled"),
            (MotionOutcome::Failed, "failed"),
        ] {
            let envelope = host(HostMessage::Event {
                event: HostEvent::MotionFinished {
                    target: target(),
                    motion_id: "org.example.pack/nod".into(),
                    action_id: "motion-request-1".into(),
                    outcome,
                },
            });
            let line = encode_host_line(&envelope).unwrap();
            assert_eq!(parse_host_line(&line).unwrap(), envelope);
            let wire: Value = serde_json::from_slice(&line).unwrap();
            assert_eq!(
                wire["message"]["event"],
                json!({
                    "type": "motion.finished",
                    "target": target(),
                    "motion_id": "org.example.pack/nod",
                    "action_id": "motion-request-1",
                    "outcome": wire_name,
                })
            );
        }
    }

    #[test]
    fn motion_receipts_require_correlation_terminal_outcome_and_valid_identifiers() {
        let base = serde_json::to_value(host(HostMessage::Event {
            event: HostEvent::MotionFinished {
                target: target(),
                motion_id: "org.example.pack/nod".into(),
                action_id: "motion-request-1".into(),
                outcome: MotionOutcome::Completed,
            },
        }))
        .unwrap();
        let rejects = |value: &Value| {
            assert!(
                parse_host_line(&serde_json::to_vec(value).unwrap()).is_err(),
                "accepted invalid motion receipt: {value}"
            );
        };
        // Draft receipts have no compatibility defaults: all fields are required.
        for field in ["target", "motion_id", "action_id", "outcome"] {
            let mut value = base.clone();
            value["message"]["event"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            rejects(&value);
        }
        let mut draft = base.clone();
        let event = draft["message"]["event"].as_object_mut().unwrap();
        event.remove("action_id");
        event.remove("outcome");
        rejects(&draft);

        for (field, limit) in [
            ("action_id", MAX_ID_BYTES),
            ("motion_id", MAX_REGISTERED_ID_BYTES),
        ] {
            for invalid in [
                String::new(),
                " ".into(),
                "two words".into(),
                "line\nbreak".into(),
                "nul\0byte".into(),
                "x".repeat(limit + 1),
            ] {
                let mut value = base.clone();
                value["message"]["event"][field] = json!(invalid);
                rejects(&value);
            }
            let mut boundary = base.clone();
            boundary["message"]["event"][field] = json!("x".repeat(limit));
            let parsed = parse_host_line(&serde_json::to_vec(&boundary).unwrap()).unwrap();
            assert!(encode_host_line(&parsed).is_ok());
        }
        for invalid in [
            json!("pending"),
            json!("Completed"),
            json!("unknown"),
            Value::Null,
            json!(1),
        ] {
            let mut value = base.clone();
            value["message"]["event"]["outcome"] = invalid;
            rejects(&value);
        }
        for field in ["action_id", "motion_id"] {
            let mut value = base.clone();
            value["message"]["event"][field] = Value::Null;
            rejects(&value);
        }
        let mut nil_target = base.clone();
        nil_target["message"]["event"]["target"]["uuid"] = json!(Uuid::nil());
        rejects(&nil_target);
        let mut extra = base;
        extra["message"]["event"]["error"] = json!("unrecognized receipt field");
        rejects(&extra);

        // Programmatically constructed events pass through the same validator.
        let invalid = host(HostMessage::Event {
            event: HostEvent::MotionFinished {
                target: target(),
                motion_id: "org.example.pack/nod".into(),
                action_id: "not an identifier".into(),
                outcome: MotionOutcome::Failed,
            },
        });
        assert!(encode_host_line(&invalid).is_err());
    }

    #[test]
    fn reply_chunks_preserve_full_utf8_text_offsets_and_terminal_marker() {
        let reply_id = Uuid::from_u128(41);
        let text = format!("{}界🙂\n \0{}", "a".repeat(4095), "語".repeat(3000));
        let mut rebuilt = String::new();
        let mut finished = false;
        for event in reply_events(reply_id, None, &text, ReplyOutcome::Completed) {
            event.validate().unwrap();
            match event {
                HostEvent::ReplyText { offset, text, .. } => {
                    assert!(!finished);
                    assert_eq!(offset, rebuilt.len() as u64);
                    assert!(text.len() <= MAX_REPLY_CHUNK_BYTES);
                    rebuilt.push_str(&text);
                }
                HostEvent::ReplyFinished { outcome, .. } => {
                    assert!(!finished);
                    assert_eq!(rebuilt, text);
                    assert_eq!(outcome, ReplyOutcome::Completed);
                    finished = true;
                }
                _ => panic!("unexpected reply event"),
            }
        }
        assert!(finished);
        let mut empty = reply_events(reply_id, None, "", ReplyOutcome::Cancelled);
        assert!(matches!(
            empty.next(),
            Some(HostEvent::ReplyFinished { .. })
        ));
        assert!(empty.next().is_none());
    }

    #[test]
    fn lifecycle_rejects_invalid_ids_chunk_sizes_offsets_and_stop_reasons() {
        let reply_id = Uuid::from_u128(41);
        for (id, character, offset, text) in [
            (Uuid::nil(), None, 0, "x".into()),
            (reply_id, Some(Uuid::nil()), 0, "x".into()),
            (reply_id, None, 0, String::new()),
            (reply_id, None, 0, "x".repeat(MAX_REPLY_CHUNK_BYTES + 1)),
            (reply_id, None, u64::MAX, "x".into()),
        ] {
            assert!(
                HostEvent::ReplyText {
                    reply_id: id,
                    character,
                    offset,
                    text
                }
                .validate()
                .is_err()
            );
        }
        let event = HostEvent::SpeechStopped {
            playback_id: Uuid::from_u128(42),
            character: None,
            voice_generation: 1,
            reply_id: Some(reply_id),
            reason: SpeechStopReason::ObservedTerminal,
        };
        let base = serde_json::to_value(host(HostMessage::Event { event })).unwrap();
        for reason in ["completed", "failed", "unknown"] {
            let mut value = base.clone();
            value["message"]["event"]["reason"] = json!(reason);
            assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let mut value = base;
        value["message"]["event"]["target"] = json!({"uuid": reply_id, "generation": 1});
        assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn actions_roundtrip_and_map_to_capabilities() {
        for (action, capability) in [
            (motion(), Capability::MotionPlay),
            (
                Action::ExpressionApply {
                    name: "happy".into(),
                    strength: 0.5,
                    duration_ms: 1200,
                },
                Capability::ExpressionApply,
            ),
            (
                Action::MessageDeliver {
                    text: "Hello\nfrom an integration".into(),
                },
                Capability::MessageDeliver,
            ),
            (
                Action::EventEmit {
                    name: "org.example.notice".into(),
                    data: json!({"count": 3, "ready": true}),
                },
                Capability::EventEmit,
            ),
            (Action::CatalogGet {}, Capability::CatalogGet),
        ] {
            assert_eq!(action.required_capability(), capability);
            let message = request(action);
            let line = encode_action_line(&message).unwrap();
            assert!(line.ends_with(b"\n"));
            assert_eq!(parse_action_line(&line).unwrap(), message);
        }
    }

    #[test]
    fn rejects_wrong_versions_types_actions_and_missing_targets() {
        let base = serde_json::to_value(request(motion())).unwrap();
        for bad in [json!(0), json!(-1), json!(1.5), json!("1")] {
            let mut value = base.clone();
            value["sequence"] = bad;
            assert!(parse_value(&value).is_err());
        }
        let mut missing_sequence = base.clone();
        missing_sequence.as_object_mut().unwrap().remove("sequence");
        assert!(parse_value(&missing_sequence).is_err());
        for bad in [json!(0), json!(2), json!(-1), json!(1.5), json!("1")] {
            let mut value = base.clone();
            value["api_version"] = bad;
            assert!(parse_value(&value).is_err());
        }
        for bad in ["host", "result", "Action", "unknown"] {
            let mut value = base.clone();
            value["type"] = json!(bad);
            assert!(parse_value(&value).is_err());
        }
        let mut value = base.clone();
        value["action"]["type"] = json!("shell.run");
        assert!(parse_value(&value).is_err());
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove("target");
        assert!(parse_value(&value).is_err());
        for field in ["uuid", "generation"] {
            let mut value = base.clone();
            value["target"].as_object_mut().unwrap().remove(field);
            assert!(parse_value(&value).is_err());
        }
        for pointer in ["/session_id", "/target/uuid"] {
            let mut value = base.clone();
            *value.pointer_mut(pointer).unwrap() = json!(Uuid::nil());
            assert!(parse_value(&value).is_err());
        }
    }

    #[test]
    fn strict_unknown_fields_and_one_shot_only() {
        for action in [
            motion(),
            Action::CatalogGet {},
            Action::MessageDeliver {
                text: "hello".into(),
            },
        ] {
            let base = serde_json::to_value(request(action)).unwrap();
            for pointer in ["", "/target", "/action"] {
                let mut value = base.clone();
                value
                    .pointer_mut(pointer)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert("unknown".into(), json!(true));
                assert!(parse_value(&value).is_err(), "{pointer}: {value}");
            }
        }
        for mode in ["loop", "repeat", "one-shot"] {
            let mut value = serde_json::to_value(request(motion())).unwrap();
            value["action"]["mode"] = json!(mode);
            assert!(parse_value(&value).is_err());
        }
        for field in ["role", "origin", "integration_id"] {
            let mut value = serde_json::to_value(request(Action::MessageDeliver {
                text: "hello".into(),
            }))
            .unwrap();
            value["action"][field] = json!("system");
            assert!(parse_value(&value).is_err());
        }
    }

    #[test]
    fn bounds_are_bytes_and_expression_values_are_finite() {
        let mut message = request(motion());
        message.id = "a".repeat(MAX_ID_BYTES);
        assert!(encode_action_line(&message).is_ok());
        message.id.push('x');
        assert!(encode_action_line(&message).is_err());
        message.id = "é".repeat(65);
        assert!(encode_action_line(&message).is_err());
        for strength in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            let message = request(Action::ExpressionApply {
                name: "happy".into(),
                strength,
                duration_ms: 100,
            });
            assert!(encode_action_line(&message).is_err());
        }
        for duration_ms in [0, MAX_EXPRESSION_DURATION_MS + 1] {
            assert!(
                encode_action_line(&request(Action::ExpressionApply {
                    name: "happy".into(),
                    strength: 1.0,
                    duration_ms
                }))
                .is_err()
            );
        }
        for duration_ms in [1, MAX_EXPRESSION_DURATION_MS] {
            assert!(
                encode_action_line(&request(Action::ExpressionApply {
                    name: "happy".into(),
                    strength: 0.0,
                    duration_ms
                }))
                .is_ok()
            );
        }
        for text in [" ".into(), "a\0b".into(), "a".repeat(MAX_TEXT_BYTES + 1)] {
            assert!(encode_action_line(&request(Action::MessageDeliver { text })).is_err());
        }
        assert!(
            encode_action_line(&request(Action::MessageDeliver {
                text: "é".repeat(MAX_TEXT_BYTES / 2)
            }))
            .is_ok()
        );
    }

    #[test]
    fn line_framing_and_exact_size_boundary() {
        let base = encode_action_line(&request(motion())).unwrap();
        let mut record = base[..base.len() - 1].to_vec();
        record.resize(MAX_LINE_BYTES, b' ');
        assert!(parse_action_line(&record).is_ok());
        record.extend_from_slice(b"\r\n");
        assert!(parse_action_line(&record).is_ok());
        record.insert(0, b' ');
        assert_eq!(parse_action_line(&record), Err(ProtocolError::LineTooLong));
        let mut two = base.clone();
        two.extend_from_slice(&base);
        assert!(parse_action_line(&two).is_err());
        for bad in [b"".as_slice(), b"\n", b"{}\n\n", b"{\r}", b"\xff"] {
            assert!(parse_action_line(bad).is_err());
        }
        let mut trailing = base[..base.len() - 1].to_vec();
        trailing.extend_from_slice(b" {}");
        assert!(parse_action_line(&trailing).is_err());
    }

    #[test]
    fn rejects_nonfinite_json_literals_and_overflowing_numbers() {
        let value = serde_json::to_value(request(Action::ExpressionApply {
            name: "happy".into(),
            strength: 0.5,
            duration_ms: 100,
        }))
        .unwrap();
        let base = serde_json::to_string(&value).unwrap();
        for bad in ["NaN", "Infinity", "-Infinity", "1e999", "null"] {
            let line = base.replace("\"strength\":0.5", &format!("\"strength\":{bad}"));
            assert_ne!(line, base);
            assert!(parse_action_line(line.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn maximum_manifest_namespace_fits_registered_ids_and_owned_events() {
        let package = format!("{}.{}", "a".repeat(63), "b".repeat(64));
        assert_eq!(package.len(), 128);
        let motion = format!("{package}/{}", "c".repeat(64));
        assert_eq!(motion.len(), MAX_REGISTERED_ID_BYTES);
        assert!(
            request(Action::MotionPlay {
                motion_id: motion.clone(),
                mode: PlaybackMode::OneShot
            })
            .validate()
            .is_ok()
        );
        assert!(
            request(Action::MotionPlay {
                motion_id: format!("{motion}x"),
                mode: PlaybackMode::OneShot
            })
            .validate()
            .is_err()
        );
        assert!(
            request(Action::EventEmit {
                name: format!("integration.{package}.notice"),
                data: Value::Null
            })
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn event_names_and_data_have_independent_budgets() {
        for name in [
            "notice",
            "Org.example.notice",
            "org..notice",
            "org.example/notice",
            ".notice",
        ] {
            assert!(
                request(Action::EventEmit {
                    name: name.into(),
                    data: Value::Null
                })
                .validate()
                .is_err()
            );
        }
        for data in [
            json!("x".repeat(MAX_EVENT_DATA_BYTES)),
            json!(vec![0; MAX_EVENT_DATA_NODES + 1]),
            json!("\n".repeat(MAX_EVENT_DATA_BYTES / 2)),
        ] {
            assert!(
                request(Action::EventEmit {
                    name: "org.example.notice".into(),
                    data
                })
                .validate()
                .is_err()
            );
        }
        let mut data = Value::Null;
        for _ in 0..=MAX_EVENT_DATA_DEPTH {
            data = json!([data]);
        }
        assert!(
            request(Action::EventEmit {
                name: "org.example.notice".into(),
                data
            })
            .validate()
            .is_err()
        );
    }

    #[test]
    fn host_handshake_results_and_events_roundtrip() {
        let catalog = ResultValue::Catalog {
            motions: vec![RegisteredMotion {
                motion_id: "org.example.pack/nod".into(),
                name: "Nod".into(),
            }],
            expressions: vec![RegisteredExpression {
                name: "happy".into(),
            }],
        };
        let messages = [
            HostMessage::Handshake {
                settings: Default::default(),
                origin: origin(),
                capabilities: vec![Capability::CatalogGet, Capability::MotionPlay],
            },
            HostMessage::Result {
                id: "request-1".into(),
                target: target(),
                result: ActionResult::Ok { value: catalog },
            },
            HostMessage::Result {
                id: "request-1".into(),
                target: target(),
                result: ActionResult::Ok {
                    value: ResultValue::Accepted {},
                },
            },
            HostMessage::Result {
                id: "request-1".into(),
                target: target(),
                result: ActionResult::Error {
                    code: ErrorCode::StaleTarget,
                    message: "Target was replaced".into(),
                },
            },
            HostMessage::Event {
                event: HostEvent::TargetChanged { target: None },
            },
            HostMessage::Event {
                event: HostEvent::MotionFinished {
                    target: target(),
                    motion_id: "org.example.pack/nod".into(),
                    action_id: "request-1".into(),
                    outcome: MotionOutcome::Completed,
                },
            },
            HostMessage::Event {
                event: HostEvent::MessageDelivered {
                    target: target(),
                    text: "Hello".into(),
                    origin: origin(),
                },
            },
            HostMessage::Event {
                event: HostEvent::Emitted {
                    target: target(),
                    name: "org.example.notice".into(),
                    data: json!({"ok": true}),
                    origin: origin(),
                },
            },
        ];
        for message in messages {
            let envelope = host(message);
            assert_eq!(
                parse_host_line(&encode_host_line(&envelope).unwrap()).unwrap(),
                envelope
            );
        }
    }

    #[test]
    fn host_types_versions_attribution_and_catalog_are_strict() {
        let base = host(HostMessage::Handshake {
            settings: Default::default(),
            origin: origin(),
            capabilities: vec![Capability::MotionPlay],
        });
        for (pointer, bad) in [
            ("/api_version", json!(2)),
            ("/type", json!("action")),
            ("/message/type", json!("unknown")),
            ("/message/capabilities/0", json!("shell.run")),
        ] {
            let mut value = serde_json::to_value(&base).unwrap();
            *value.pointer_mut(pointer).unwrap() = bad;
            assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        for pointer in ["", "/message", "/message/origin"] {
            let mut value = serde_json::to_value(&base).unwrap();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("role".into(), json!("system"));
            assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        assert!(
            host(HostMessage::Handshake {
                settings: Default::default(),
                origin: origin(),
                capabilities: vec![Capability::MotionPlay; 2]
            })
            .validate()
            .is_err()
        );
        let entry = RegisteredMotion {
            motion_id: "org.example.pack/nod".into(),
            name: "Nod".into(),
        };
        for motions in [vec![entry.clone(); 2], vec![entry; MAX_CATALOG_ENTRIES + 1]] {
            assert!(
                ActionResult::Ok {
                    value: ResultValue::Catalog {
                        motions,
                        expressions: vec![]
                    }
                }
                .validate()
                .is_err()
            );
        }
        let mut value = serde_json::to_value(host(HostMessage::Result {
            id: "request-1".into(),
            target: target(),
            result: ActionResult::Ok {
                value: ResultValue::Accepted {},
            },
        }))
        .unwrap();
        value["message"]["result"]["value"]["ignored"] = json!(true);
        assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn handshake_settings_are_typed_bounded_and_optional_for_older_hosts() {
        use crate::extension_settings::{SettingValue, Settings};
        let settings = Settings::from([
            ("enabled".into(), SettingValue::Bool(true)),
            ("count".into(), SettingValue::Integer(3)),
            ("label".into(), SettingValue::Text("hello".into())),
        ]);
        let envelope = host(HostMessage::Handshake {
            origin: origin(),
            capabilities: vec![],
            settings,
        });
        assert_eq!(
            parse_host_line(&encode_host_line(&envelope).unwrap()).unwrap(),
            envelope
        );
        let mut value = serde_json::to_value(&envelope).unwrap();
        value["message"].as_object_mut().unwrap().remove("settings");
        assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_ok());
        for bad in [
            json!({"label": {"nested":true}}),
            json!({"label": 1.5}),
            json!({"label": "x".repeat(1025)}),
            json!({"bad key": true}),
        ] {
            value["message"]["settings"] = bad;
            assert!(parse_host_line(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn identity_fields_remain_distinct_without_claiming_freshness_checks() {
        let message = request(Action::CatalogGet {});
        assert_ne!(message.process_generation, message.target.generation);
        let decoded = parse_action_line(&encode_action_line(&message).unwrap()).unwrap();
        assert_eq!(decoded.session_id, Uuid::from_u128(1));
        assert_eq!(decoded.sequence, 1);
        assert_eq!(decoded.process_generation, 7);
        assert_eq!(decoded.target.generation, 19);
        // Comparing these to current state is deliberately a runtime responsibility.
        let mut another_incarnation = decoded;
        another_incarnation.process_generation += 1;
        assert!(another_incarnation.validate().is_ok());
    }
}

#[cfg(test)]
mod incarnation_stop_wire_tests {
    use super::*;
    #[test]
    fn stop_wire_has_explicit_nulls_fixed_reasons_and_valid_incarnation_identity() {
        let installation_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        for reason in [
            ExtensionStopReason::Disabled,
            ExtensionStopReason::Removed,
            ExtensionStopReason::Restarted,
            ExtensionStopReason::HostShutdown,
            ExtensionStopReason::Reconfigured,
        ] {
            let event = HostEvent::ExtensionStopping {
                installation_id,
                session_id,
                process_generation: 2,
                reason,
            };
            assert!(event.validate().is_ok());
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], "extension.stopping");
            assert_eq!(serde_json::from_value::<HostEvent>(value).unwrap(), event);
        }
        let event = HostEvent::ExtensionStopped {
            installation_id,
            session_id,
            process_generation: 2,
            exit_code: None,
            requested_reason: None,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"extension.stopped","installation_id":installation_id,"session_id":session_id,"process_generation":2,"exit_code":null,"requested_reason":null})
        );
        assert_eq!(
            serde_json::from_value::<HostEvent>(value.clone()).unwrap(),
            event
        );
        let mut bad = value.clone();
        bad["requested_reason"] = serde_json::json!("arbitrary child error");
        assert!(serde_json::from_value::<HostEvent>(bad).is_err());
        let mut bad = value.clone();
        bad["installation_id"] = serde_json::json!(Uuid::nil());
        assert!(
            serde_json::from_value::<HostEvent>(bad)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut bad = value;
        bad["process_generation"] = serde_json::json!(0);
        assert!(
            serde_json::from_value::<HostEvent>(bad)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[cfg(test)]
mod generic_motion_wire_tests {
    use super::*;
    #[test]
    fn motion_ended_wire_preserves_tagged_sources_nulls_and_validates_identity() {
        for source in [
            MotionPlaybackSource::Host,
            MotionPlaybackSource::Extension {
                integration_id: "org.example.test".into(),
                session_id: Uuid::new_v4(),
                process_generation: 2,
                action_id: "wave-1".into(),
            },
        ] {
            let event = HostEvent::MotionPlaybackEnded {
                playback_id: Uuid::new_v4(),
                source_target: None,
                motion_id: None,
                source: source.clone(),
                outcome: MotionOutcome::Failed,
            };
            assert!(event.validate().is_ok());
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], "motion.playback.ended");
            assert!(value["motion_id"].is_null() && value["source_target"].is_null());
            assert_eq!(
                value["source"]["type"],
                if matches!(source, MotionPlaybackSource::Host) {
                    "host"
                } else {
                    "extension"
                }
            );
            assert_eq!(
                serde_json::from_value::<HostEvent>(value.clone()).unwrap(),
                event
            );
            let mut bad = value;
            bad["playback_id"] = serde_json::json!(Uuid::nil());
            assert!(
                serde_json::from_value::<HostEvent>(bad)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod motion_start_wire_tests {
    use super::*;
    #[test]
    fn motion_started_preserves_source_and_rejects_terminal_fields_or_invalid_identity() {
        for source in [
            MotionPlaybackSource::Host,
            MotionPlaybackSource::Extension {
                integration_id: "org.example.sender".into(),
                session_id: Uuid::new_v4(),
                process_generation: 7,
                action_id: "wave-1".into(),
            },
        ] {
            let event = HostEvent::MotionPlaybackStarted {
                playback_id: Uuid::new_v4(),
                source_target: Some(Target {
                    uuid: Uuid::new_v4(),
                    generation: 3,
                }),
                motion_id: Some("org.example.pack/wave".into()),
                source,
            };
            assert!(event.validate().is_ok());
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], "motion.playback.started");
            assert!(value.get("outcome").is_none());
            assert_eq!(
                serde_json::from_value::<HostEvent>(value.clone()).unwrap(),
                event
            );
            let mut extra = value.clone();
            extra["outcome"] = serde_json::json!("completed");
            assert!(serde_json::from_value::<HostEvent>(extra).is_err());
            let mut invalid = value;
            invalid["playback_id"] = serde_json::json!(Uuid::nil());
            assert!(
                serde_json::from_value::<HostEvent>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
}
