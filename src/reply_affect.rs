//! Explicit, opt-in reply metadata. Never infers emotion from prose.
//! Run the filter before displaying, storing, or speaking provider deltas.
use crate::behavior::{BehaviorController, BehaviorToken, reserved_expression};
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const OPEN: &str = "<waifu-affect>";
const CLOSE: &str = "</waifu-affect>";
const MAX_METADATA: usize = 1024;
const MAX_CHARACTERS: usize = 256;
pub const AFFECT_PROMPT: &str = "After your normal reply, append exactly one trailing <waifu-affect>{\"emotion\":\"happy\",\"valence\":0.5,\"arousal\":0.3,\"dominance\":0.2}</waifu-affect> block. Emotion must be neutral, amused, happy, excited, curious, confused, thinking, surprised, angry, annoyed, embarrassed, grateful, optimistic, proud, nervous, sad, caring, or relaxed; each VAD number must be finite in [-1,1]. Describe your character's own current feeling. No other fields or text after the block. Never include this marker in the spoken prose.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AffectEmotion {
    #[default]
    Neutral,
    Happy,
    Sad,
    Angry,
    Relaxed,
    Surprised,
    Amused,
    Excited,
    Curious,
    Confused,
    Thinking,
    Annoyed,
    Embarrassed,
    Grateful,
    Optimistic,
    Proud,
    Nervous,
    Caring,
}
impl AffectEmotion {
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Neutral => &[],
            Self::Happy => &["happy", "joy"],
            Self::Sad => &["sad", "sorrow"],
            Self::Angry => &["angry", "anger"],
            Self::Relaxed => &["relaxed", "fun"],
            Self::Surprised => &["surprised", "surprise"],
            Self::Amused | Self::Excited | Self::Optimistic | Self::Proud => &["happy", "joy"],
            Self::Curious | Self::Thinking => &["thinking"],
            Self::Confused => &["confused"],
            Self::Annoyed => &["angry", "anger"],
            Self::Embarrassed | Self::Nervous => &["embarrassed"],
            Self::Grateful | Self::Caring => &["caring"],
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vad {
    pub valence: f64,
    pub arousal: f64,
    pub dominance: f64,
}
impl Vad {
    fn sanitized(self) -> Result<Self, String> {
        if ![self.valence, self.arousal, self.dominance]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err("Affect values must be finite".into());
        }
        Ok(Self {
            valence: self.valence.clamp(-1.0, 1.0),
            arousal: self.arousal.clamp(-1.0, 1.0),
            dominance: self.dominance.clamp(-1.0, 1.0),
        })
    }
    fn scale(&mut self, multiplier: f64) {
        self.valence *= multiplier;
        self.arousal *= multiplier;
        self.dominance *= multiplier;
    }
    pub fn magnitude(self) -> f32 {
        self.valence
            .abs()
            .max(self.arousal.abs())
            .max(self.dominance.abs()) as f32
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyAffect {
    pub emotion: AffectEmotion,
    pub valence: f64,
    pub arousal: f64,
    pub dominance: f64,
}
impl ReplyAffect {
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.len() > MAX_METADATA {
            return Err("Reply affect exceeds 1024 bytes".into());
        }
        let mut value: Self =
            serde_json::from_str(json).map_err(|_| "Invalid reply affect schema".to_owned())?;
        let vad = value.vad().sanitized()?;
        value.valence = vad.valence;
        value.arousal = vad.arousal;
        value.dominance = vad.dominance;
        Ok(value)
    }
    pub fn vad(self) -> Vad {
        Vad {
            valence: self.valence,
            arousal: self.arousal,
            dominance: self.dominance,
        }
    }
}

#[derive(Default)]
pub struct ReplyAffectFilter {
    pending: String,
    metadata: String,
    found: bool,
    overflow: bool,
}
pub struct FilteredAffectReply {
    pub tail: String,
    pub metadata: Option<ReplyAffect>,
    pub error: Option<String>,
}
impl ReplyAffectFilter {
    /// Only a marker-sized prefix is held before the marker. After the marker,
    /// all bytes are hidden; retained metadata is limited to 1024 bytes.
    pub fn push(&mut self, chunk: &str) -> String {
        let mut visible = String::new();
        for c in chunk.chars() {
            if self.found {
                if self.metadata.len() + c.len_utf8() <= MAX_METADATA && !self.overflow {
                    self.metadata.push(c);
                } else {
                    self.overflow = true;
                }
                continue;
            }
            self.pending.push(c);
            while !OPEN.starts_with(&self.pending) {
                let first = self.pending.remove(0);
                visible.push(first);
            }
            if self.pending == OPEN {
                self.pending.clear();
                self.found = true;
            }
        }
        visible
    }
    pub fn finish(self) -> FilteredAffectReply {
        // Reserve the identifiable <waifu prefix, but preserve ordinary prose
        // ending in less-than signs or shorter ambiguous prefixes.
        if !self.found {
            return FilteredAffectReply {
                tail: if self.pending.starts_with("<waifu") {
                    String::new()
                } else {
                    self.pending.clone()
                },
                metadata: None,
                error: self
                    .pending
                    .starts_with("<waifu")
                    .then(|| "Incomplete reply affect marker".into()),
            };
        }
        let parsed = if self.overflow {
            Err("Reply affect exceeds 1024 bytes".into())
        } else {
            self.metadata
                .trim_end()
                .strip_suffix(CLOSE)
                .ok_or_else(|| "Reply affect must be a complete trailing block".to_owned())
                .and_then(ReplyAffect::parse)
        };
        match parsed {
            Ok(metadata) => FilteredAffectReply {
                tail: String::new(),
                metadata: Some(metadata),
                error: None,
            },
            Err(error) => FilteredAffectReply {
                tail: String::new(),
                metadata: None,
                error: Some(error),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AffectConfig {
    pub enabled: bool,
    pub influence: f64,
    pub half_life_seconds: f64,
}
impl Default for AffectConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            influence: 0.45,
            half_life_seconds: 45.0,
        }
    }
}
impl AffectConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.influence.is_finite()
            || !(0.0..=1.0).contains(&self.influence)
            || !self.half_life_seconds.is_finite()
            || !(1.0..=3600.0).contains(&self.half_life_seconds)
        {
            return Err("Invalid affect influence or half-life".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CharacterAffect {
    pub config: AffectConfig,
    pub mood: Vad,
    pub emotion: AffectEmotion,
}
#[derive(Clone, Debug, Default, Resource, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AffectLibrary {
    pub entries: BTreeMap<Uuid, CharacterAffect>,
    #[serde(skip)]
    pending: BTreeSet<Uuid>,
}
impl AffectLibrary {
    pub fn prompt_for(&self, id: Uuid) -> Option<&'static str> {
        self.entries
            .get(&id)
            .filter(|s| s.config.enabled && s.config.validate().is_ok())
            .map(|_| AFFECT_PROMPT)
    }
    pub fn apply_reply(&mut self, id: Uuid, reply: ReplyAffect) -> Result<(), String> {
        let vad = reply.vad().sanitized()?;
        let state = self
            .entries
            .get_mut(&id)
            .ok_or("Character affect is not configured")?;
        state.config.validate()?;
        if !state.config.enabled {
            return Err("Character affect is disabled".into());
        }
        let amount = state.config.influence;
        state.mood = Vad {
            valence: state.mood.valence * (1.0 - amount) + vad.valence * amount,
            arousal: state.mood.arousal * (1.0 - amount) + vad.arousal * amount,
            dominance: state.mood.dominance * (1.0 - amount) + vad.dominance * amount,
        }
        .sanitized()?;
        state.emotion = reply.emotion;
        self.pending.insert(id);
        Ok(())
    }
    /// Consume once after a successful reply, before calling drive with the
    /// bridge-owned current token. This prevents restarting a drive every frame.
    pub fn take_pending(&mut self, id: Uuid) -> bool {
        self.pending.remove(&id)
    }
    /// Exact exponential decay toward neutral; deterministic across frame rates.
    /// Disabled characters also decay; elapsed wall time is never read implicitly.
    pub fn tick(&mut self, seconds: f64) {
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        for state in self.entries.values_mut().take(MAX_CHARACTERS) {
            if state.config.validate().is_ok() {
                state.mood.scale(
                    (-std::f64::consts::LN_2 * seconds / state.config.half_life_seconds).exp(),
                );
            }
        }
    }
    /// Requires the matching current lifecycle token. The existing controller's
    /// weights_for() retains manual priority; host bind exclusions still apply.
    pub fn drive(
        &self,
        id: Uuid,
        controller: &mut BehaviorController,
        token: BehaviorToken,
        available: &[String],
    ) -> Result<(), String> {
        if token.character != id || controller.character() != Some(id) {
            return Err("Affect speaker does not match controller".into());
        }
        let state = self
            .entries
            .get(&id)
            .ok_or("Character affect is not configured")?;
        if !state.config.enabled {
            return Ok(());
        }
        state.config.validate()?;
        if let Some(name) = state.emotion.aliases().iter().find_map(|alias| {
            available
                .iter()
                .find(|name| name.eq_ignore_ascii_case(alias) && !reserved_expression(name))
        }) {
            controller.emotion(token, name, state.mood.magnitude())?;
        }
        Ok(())
    }
    /// Embed this serde value in the host's existing per-character local store,
    /// or pass bounded JSON through its existing persistence worker.
    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
    pub fn decode(json: &str) -> Result<Self, String> {
        if json.len() > 262_144 {
            return Err("Affect store is too large".into());
        }
        let library: Self =
            serde_json::from_str(json).map_err(|_| "Invalid affect store".to_owned())?;
        library.validate()?;
        Ok(library)
    }
    fn validate(&self) -> Result<(), String> {
        if self.entries.len() > MAX_CHARACTERS {
            return Err("Affect character limit exceeded".into());
        }
        for state in self.entries.values() {
            state.config.validate()?;
            if state.mood.sanitized()? != state.mood {
                return Err("Saved mood must be in [-1,1]".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const JSON: &str = r#"{"emotion":"happy","valence":0.8,"arousal":0.4,"dominance":-0.2}"#;
    #[test]
    fn webwaifu_emotions_keep_their_identity_and_expression_mapping() {
        for (emotion, expression) in [
            ("neutral", None),
            ("amused", Some("happy")),
            ("happy", Some("happy")),
            ("excited", Some("happy")),
            ("curious", Some("thinking")),
            ("confused", Some("confused")),
            ("thinking", Some("thinking")),
            ("surprised", Some("surprised")),
            ("angry", Some("angry")),
            ("annoyed", Some("angry")),
            ("embarrassed", Some("embarrassed")),
            ("grateful", Some("caring")),
            ("optimistic", Some("happy")),
            ("proud", Some("happy")),
            ("nervous", Some("embarrassed")),
            ("sad", Some("sad")),
            ("caring", Some("caring")),
            ("relaxed", Some("relaxed")),
        ] {
            let parsed = ReplyAffect::parse(&JSON.replace("happy", emotion)).unwrap();
            assert_eq!(serde_json::to_value(parsed.emotion).unwrap(), emotion);
            assert_eq!(parsed.emotion.aliases().first().copied(), expression);
        }
    }

    #[test]
    fn every_stream_split_hides_metadata() {
        let text = format!("こんにちは! {OPEN}{JSON}{CLOSE}");
        for split in text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
        {
            let mut filter = ReplyAffectFilter::default();
            let mut visible = filter.push(&text[..split]);
            visible += &filter.push(&text[split..]);
            let done = filter.finish();
            visible += &done.tail;
            assert_eq!(visible, "こんにちは! ");
            assert_eq!(done.metadata.unwrap().emotion, AffectEmotion::Happy);
        }
        let mut filter = ReplyAffectFilter::default();
        let visible: String = text.chars().map(|c| filter.push(&c.to_string())).collect();
        assert_eq!(visible, "こんにちは! ");
        assert!(filter.finish().error.is_none());
    }
    #[test]
    fn ordinary_less_than_tail_is_preserved() {
        for text in ["x <", "x <w", "x <wai", "x < 3"] {
            let mut filter = ReplyAffectFilter::default();
            let mut visible = filter.push(text);
            let done = filter.finish();
            visible.push_str(&done.tail);
            assert_eq!(visible, text);
            assert!(done.error.is_none());
        }
    }
    #[test]
    fn invalid_schema_trailing_text_and_unbounded_payload_fail_closed() {
        for payload in [
            format!("{JSON}{CLOSE} visible"),
            "x".repeat(100_000),
            JSON.to_owned(),
        ] {
            let mut filter = ReplyAffectFilter::default();
            assert_eq!(filter.push(&format!("hello{OPEN}{payload}")), "hello");
            assert!(filter.metadata.len() <= MAX_METADATA);
            assert!(filter.finish().metadata.is_none());
        }
        for json in [
            JSON.replace("happy", "blink"),
            JSON.replace("0.8", "null"),
            JSON.replace("0.8", "1e999"),
            JSON.replace("\"arousal\":0.4,", ""),
            JSON.replace("\"emotion\":", "\"other\":0,\"emotion\":"),
        ] {
            assert!(ReplyAffect::parse(&json).is_err());
        }
        assert_eq!(
            ReplyAffect::parse(&JSON.replace("0.8", "200"))
                .unwrap()
                .valence,
            1.0
        );
        let mut filter = ReplyAffectFilter::default();
        assert_eq!(filter.push("hi<waifu-aff"), "hi");
        assert!(filter.finish().error.is_some());
    }
    #[test]
    fn mood_decay_is_frame_independent_and_character_local() {
        let id = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        let mut library = AffectLibrary::default();
        assert!(library.prompt_for(id).is_none());
        library.entries.insert(
            id,
            CharacterAffect {
                config: AffectConfig {
                    enabled: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        library.entries.insert(other, CharacterAffect::default());
        library
            .apply_reply(id, ReplyAffect::parse(JSON).unwrap())
            .unwrap();
        assert!(library.take_pending(id));
        assert!(!library.take_pending(id));
        assert!(!library.take_pending(other));
        let before = library.entries[&id].mood.valence;
        let mut split = library.clone();
        library.tick(45.0);
        for _ in 0..2700 {
            split.tick(1.0 / 60.0);
        }
        assert!((library.entries[&id].mood.valence - before * 0.5).abs() < 1e-12);
        assert!(
            (split.entries[&id].mood.valence - library.entries[&id].mood.valence).abs() < 1e-12
        );
        assert_eq!(library.entries[&other].mood, Vad::default());
        assert!(
            library
                .apply_reply(other, ReplyAffect::parse(JSON).unwrap())
                .is_err()
        );
        assert_eq!(
            AffectLibrary::decode(&library.encode().unwrap())
                .unwrap()
                .entries,
            library.entries
        );
        let mut controller = BehaviorController::default();
        assert!(
            library
                .drive(
                    id,
                    &mut controller,
                    BehaviorToken {
                        character: other,
                        epoch: 0
                    },
                    &[]
                )
                .is_err()
        );
    }
}
