//! Explicit opt-in affect sources: reply metadata or bounded Jev character-reaction appraisal.
//! Run the filter before displaying, storing, or speaking provider deltas.
use crate::behavior::{
    BehaviorController, BehaviorToken, EmotionContribution, reserved_expression,
};
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const OPEN: &str = "<waifu-affect>";
const CLOSE: &str = "</waifu-affect>";
const MAX_METADATA: usize = 1024;
const MAX_CHARACTERS: usize = 256;
pub const AFFECT_PROMPT: &str = "After your normal reply, append exactly one trailing <waifu-affect>{\"emotion\":\"happy\",\"valence\":0.5,\"arousal\":0.3,\"dominance\":0.2}</waifu-affect> block. Emotion must be neutral, amused, happy, excited, curious, confused, thinking, surprised, angry, annoyed, embarrassed, grateful, optimistic, proud, nervous, sad, caring, relaxed, fear, or disgust; each VAD number must be finite in [-1,1]. Describe your character's own current feeling. No other fields or text after the block. Never include this marker in the spoken prose.";

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
    Fear,
    Disgust,
}
impl AffectEmotion {
    /// Canonical accepted catalog in motion-picker order.
    pub const ALL: [AffectEmotion; 20] = [
        Self::Neutral,
        Self::Amused,
        Self::Happy,
        Self::Excited,
        Self::Curious,
        Self::Confused,
        Self::Thinking,
        Self::Surprised,
        Self::Angry,
        Self::Annoyed,
        Self::Embarrassed,
        Self::Grateful,
        Self::Optimistic,
        Self::Proud,
        Self::Nervous,
        Self::Sad,
        Self::Caring,
        Self::Relaxed,
        Self::Fear,
        Self::Disgust,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Amused => "amused",
            Self::Happy => "happy",
            Self::Excited => "excited",
            Self::Curious => "curious",
            Self::Confused => "confused",
            Self::Thinking => "thinking",
            Self::Surprised => "surprised",
            Self::Angry => "angry",
            Self::Annoyed => "annoyed",
            Self::Embarrassed => "embarrassed",
            Self::Grateful => "grateful",
            Self::Optimistic => "optimistic",
            Self::Proud => "proud",
            Self::Nervous => "nervous",
            Self::Sad => "sad",
            Self::Caring => "caring",
            Self::Relaxed => "relaxed",
            Self::Fear => "fear",
            Self::Disgust => "disgust",
        }
    }

    pub(crate) fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Neutral => &[],
            Self::Fear => &["fear", "fearful", "afraid"],
            Self::Disgust => &["disgust", "disgusted"],
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffectSource {
    #[default]
    ReplyMetadata,
    UserMessageJev,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AffectConfig {
    pub enabled: bool,
    /// Saves without a source predate appraisal and keep reply metadata; new
    /// configs appraise the incoming message so emotion is known before speech.
    #[serde(default)]
    pub source: AffectSource,
    pub influence: f64,
    pub half_life_seconds: f64,
}
impl Default for AffectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            source: AffectSource::UserMessageJev,
            influence: 0.45,
            half_life_seconds: 45.0,
        }
    }
}
impl AffectConfig {
    pub fn requests_metadata(&self) -> bool {
        self.enabled && self.source == AffectSource::ReplyMetadata && self.validate().is_ok()
    }
    pub fn requests_appraisal(&self) -> bool {
        self.enabled && self.source == AffectSource::UserMessageJev && self.validate().is_ok()
    }
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

/// A committed change of emotion label, independent of source identity and
/// avatar expression selection. Same-label mood updates produce no transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffectTransition {
    pub previous: AffectEmotion,
    pub emotion: AffectEmotion,
}

/// Runtime-only adapter for enabled content packs. The host supplies mappings in
/// precedence order; affect persistence stays independent of manifest schemas.
#[derive(Clone, Copy, Debug)]
pub struct AffectExpressionMapping<'a> {
    pub emotion: AffectEmotion,
    pub expression: &'a str,
    /// Multiplier on mood magnitude, clamped to [0, 1]. Nonfinite values are ignored.
    pub strength: f32,
}

/// Resolved mapping identity, independent of the mood's ongoing decay.
#[derive(Clone, Debug, PartialEq)]
pub struct AffectExpressionSelection {
    pub expression: String,
    pub multiplier: f32,
    pub mapped: bool,
}

#[derive(Clone, Debug)]
pub struct AffectContribution {
    pub selection: AffectExpressionSelection,
    pub contribution: EmotionContribution,
}

fn available_expression<'a>(available: &'a [String], requested: &str) -> Option<&'a str> {
    if reserved_expression(requested) {
        return None;
    }
    // Prefer authored casing, then lexical order so avatar catalog order cannot
    // change the selected channel when names differ only by ASCII case.
    available
        .iter()
        .find(|name| name.as_str() == requested)
        .or_else(|| {
            available
                .iter()
                .filter(|name| name.eq_ignore_ascii_case(requested))
                .min()
        })
        .filter(|name| !reserved_expression(name))
        .map(String::as_str)
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
            .filter(|s| s.config.requests_metadata())
            .map(|_| AFFECT_PROMPT)
    }
    pub fn apply_reply(&mut self, id: Uuid, reply: ReplyAffect) -> Result<(), String> {
        self.apply_reply_tracked(id, reply).map(|_| ())
    }
    /// Apply validated affect and report a label transition only after commit.
    /// Successful same-label updates still blend mood and mark pending behavior.
    /// The caller must reject stale source results before invoking this method;
    /// this layer has no turn, request, or avatar identity to authenticate them.
    pub fn apply_reply_tracked(
        &mut self,
        id: Uuid,
        reply: ReplyAffect,
    ) -> Result<Option<AffectTransition>, String> {
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
        let previous = state.emotion;
        state.emotion = reply.emotion;
        self.pending.insert(id);
        Ok((previous != reply.emotion).then_some(AffectTransition {
            previous,
            emotion: reply.emotion,
        }))
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
        self.drive_with_mappings(id, controller, token, available, std::iter::empty())
    }
    /// First usable mapping for the exact emotion wins. Missing/reserved targets
    /// and nonfinite strengths are skipped; built-in aliases remain the fallback.
    /// All decisions still pass through the controller's lifecycle and limits.
    pub fn drive_with_mappings<'a>(
        &self,
        id: Uuid,
        controller: &mut BehaviorController,
        token: BehaviorToken,
        available: &[String],
        mappings: impl IntoIterator<Item = AffectExpressionMapping<'a>>,
    ) -> Result<(), String> {
        self.drive_with_mappings_tracked(id, controller, token, available, mappings)
            .map(|_| ())
    }
    pub fn drive_with_mappings_tracked<'a>(
        &self,
        id: Uuid,
        controller: &mut BehaviorController,
        token: BehaviorToken,
        available: &[String],
        mappings: impl IntoIterator<Item = AffectExpressionMapping<'a>>,
    ) -> Result<Option<AffectContribution>, String> {
        if token.character != id || controller.character() != Some(id) {
            return Err("Affect speaker does not match controller".into());
        }
        let Some(selection) = self.expression_with_mappings(id, available, mappings)? else {
            return Ok(None);
        };
        let magnitude = self.entries[&id].mood.sanitized()?.magnitude();
        let contribution = controller.emotion_tracked(
            token,
            &selection.expression,
            magnitude * selection.multiplier,
        )?;
        Ok(Some(AffectContribution {
            selection,
            contribution,
        }))
    }
    /// Resolve without driving. Comparing this value across catalog refreshes
    /// avoids restarting the decaying emotion when only unrelated content changed.
    pub fn expression_with_mappings<'a>(
        &self,
        id: Uuid,
        available: &[String],
        mappings: impl IntoIterator<Item = AffectExpressionMapping<'a>>,
    ) -> Result<Option<AffectExpressionSelection>, String> {
        let state = self
            .entries
            .get(&id)
            .ok_or("Character affect is not configured")?;
        if !state.config.enabled {
            return Ok(None);
        }
        state.config.validate()?;
        let mapped = mappings.into_iter().find_map(|mapping| {
            if mapping.emotion != state.emotion || !mapping.strength.is_finite() {
                return None;
            }
            available_expression(available, mapping.expression)
                .map(|name| (name, mapping.strength.clamp(0.0, 1.0), true))
        });
        let selected = mapped.or_else(|| {
            state.emotion.aliases().iter().find_map(|alias| {
                available_expression(available, alias).map(|name| (name, 1.0, false))
            })
        });
        Ok(
            selected.map(|(name, multiplier, mapped)| AffectExpressionSelection {
                expression: name.into(),
                multiplier,
                mapped,
            }),
        )
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

    #[test]
    fn canonical_catalog_is_unique_and_matches_persisted_and_prompt_names() {
        let mut names = std::collections::BTreeSet::new();
        for emotion in AffectEmotion::ALL {
            assert!(names.insert(emotion.as_str()));
            let value = serde_json::to_value(emotion).unwrap();
            assert_eq!(value.as_str(), Some(emotion.as_str()));
            assert_eq!(
                serde_json::from_value::<AffectEmotion>(value).unwrap(),
                emotion
            );
            assert!(AFFECT_PROMPT.contains(emotion.as_str()));
        }
        assert_eq!(names.len(), 20);
    }
    use super::*;

    fn mapping_fixture() -> (
        AffectLibrary,
        BehaviorController,
        BehaviorToken,
        Vec<String>,
    ) {
        let id = Uuid::new_v4();
        let mut library = AffectLibrary::default();
        library.entries.insert(
            id,
            CharacterAffect {
                emotion: AffectEmotion::Happy,
                mood: Vad {
                    valence: 0.8,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let available = ["happy", "Joy", "CustomSmile", "blink", "lookUp", "aa"]
            .map(String::from)
            .to_vec();
        let mut controller = BehaviorController::default();
        controller
            .select(Some(id), &Default::default(), &available)
            .unwrap();
        let token = controller.begin_turn(id).unwrap();
        (library, controller, token, available)
    }

    #[test]
    fn custom_mappings_match_exact_emotion_and_scale_mood_strength() {
        for strength in [-2.0_f32, 0.0, 0.25, 1.0, 4.0] {
            let (library, mut controller, token, available) = mapping_fixture();
            let mut baseline = BehaviorController::default();
            baseline
                .select(Some(token.character), &Default::default(), &available)
                .unwrap();
            let baseline_token = baseline.begin_turn(token.character).unwrap();
            library
                .drive(token.character, &mut baseline, baseline_token, &available)
                .unwrap();
            library
                .drive_with_mappings(
                    token.character,
                    &mut controller,
                    token,
                    &available,
                    [
                        AffectExpressionMapping {
                            emotion: AffectEmotion::Excited,
                            expression: "Joy",
                            strength: 1.0,
                        },
                        AffectExpressionMapping {
                            emotion: AffectEmotion::Happy,
                            expression: "customsmile",
                            strength,
                        },
                    ],
                )
                .unwrap();
            controller.tick(0.2);
            baseline.tick(0.2);
            let weights =
                controller.weights_for(Some(token.character), &available, &BTreeMap::new(), "");
            let default_weight = baseline.weights_for(
                Some(token.character),
                &available,
                &BTreeMap::new(),
                "",
            )["happy"];
            let actual = weights.get("CustomSmile").copied().unwrap_or(0.0);
            assert!((actual - default_weight * strength.clamp(0.0, 1.0)).abs() < 1e-6);
            assert!(actual.is_finite() && (0.0..=0.65).contains(&actual));
            assert!(!weights.contains_key("Joy"));
            assert!(!weights.contains_key("happy"));
        }
    }

    #[test]
    fn unusable_mappings_fall_through_to_next_mapping_then_builtin_aliases() {
        for use_second_mapping in [false, true] {
            let (library, mut controller, token, mut available) = mapping_fixture();
            available.retain(|name| name != "happy");
            controller
                .select(Some(token.character), &Default::default(), &available)
                .unwrap();
            let mut mappings: Vec<_> = [
                ("missing", 1.0),
                ("blink", 1.0),
                ("lookUp", 1.0),
                ("aa", 1.0),
                ("CustomSmile", f32::NAN),
                ("CustomSmile", f32::INFINITY),
                ("CustomSmile", f32::NEG_INFINITY),
            ]
            .into_iter()
            .map(|(expression, strength)| AffectExpressionMapping {
                emotion: AffectEmotion::Happy,
                expression,
                strength,
            })
            .collect();
            if use_second_mapping {
                mappings.push(AffectExpressionMapping {
                    emotion: AffectEmotion::Happy,
                    expression: "CustomSmile",
                    strength: 0.5,
                });
            }
            library
                .drive_with_mappings(
                    token.character,
                    &mut controller,
                    token,
                    &available,
                    mappings,
                )
                .unwrap();
            controller.tick(0.2);
            let weights =
                controller.weights_for(Some(token.character), &available, &BTreeMap::new(), "");
            assert_eq!(weights.len(), 1);
            assert!(
                weights[if use_second_mapping {
                    "CustomSmile"
                } else {
                    "Joy"
                }] > 0.0
            );
        }
    }

    #[test]
    fn expression_resolution_prefers_exact_case_then_lexical_case_and_alias_order() {
        let mut names = ["joy", "HAPPY", "Happy", "happy"].map(String::from);
        assert_eq!(available_expression(&names, "happy"), Some("happy"));
        assert_eq!(available_expression(&names, "HaPpY"), Some("HAPPY"));
        names.reverse();
        assert_eq!(available_expression(&names, "HaPpY"), Some("HAPPY"));
        let (library, mut controller, token, _) = mapping_fixture();
        controller
            .select(Some(token.character), &Default::default(), &names)
            .unwrap();
        library
            .drive(token.character, &mut controller, token, &names)
            .unwrap();
        controller.tick(0.2);
        let weights = controller.weights_for(Some(token.character), &names, &BTreeMap::new(), "");
        assert!(weights.contains_key("happy"));
        assert!(!weights.contains_key("joy"));
    }

    #[test]
    fn pack_mappings_preserve_manual_reserved_and_epoch_ownership() {
        let (library, mut controller, token, available) = mapping_fixture();
        let mappings = [AffectExpressionMapping {
            emotion: AffectEmotion::Happy,
            expression: "CustomSmile",
            strength: 1.0,
        }];
        library
            .drive_with_mappings(
                token.character,
                &mut controller,
                token,
                &available,
                mappings,
            )
            .unwrap();
        controller.tick(0.2);
        for manual in ["CustomSmile", "happy", "blink", "aa", "lookUp"] {
            assert!(
                controller
                    .weights_for(
                        Some(token.character),
                        &available,
                        &BTreeMap::from([(manual.into(), 0.3)]),
                        "",
                    )
                    .is_empty()
            );
        }
        assert!(
            controller
                .weights_for(Some(token.character), &available, &BTreeMap::new(), "Joy")
                .is_empty()
        );
        controller.reset();
        assert!(
            library
                .drive_with_mappings(
                    token.character,
                    &mut controller,
                    token,
                    &available,
                    mappings
                )
                .is_err()
        );
        assert!(
            controller
                .weights_for(Some(token.character), &available, &BTreeMap::new(), "")
                .is_empty()
        );
    }

    #[test]
    fn affect_sources_are_exclusive_and_old_profiles_request_metadata() {
        let mut config: AffectConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(config.requests_metadata());
        assert!(!config.requests_appraisal());
        config.source = AffectSource::UserMessageJev;
        assert!(!config.requests_metadata());
        assert!(config.requests_appraisal());
        let id = Uuid::new_v4();
        let mut library = AffectLibrary::default();
        library.entries.insert(
            id,
            CharacterAffect {
                config: config.clone(),
                ..Default::default()
            },
        );
        assert!(library.prompt_for(id).is_none());
        config.enabled = false;
        assert!(!config.requests_metadata());
        assert!(!config.requests_appraisal());
    }

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
            ("fear", Some("fear")),
            ("disgust", Some("disgust")),
        ] {
            let parsed = ReplyAffect::parse(&JSON.replace("happy", emotion)).unwrap();
            assert_eq!(serde_json::to_value(parsed.emotion).unwrap(), emotion);
            assert_eq!(parsed.emotion.aliases().first().copied(), expression);
        }
    }

    #[test]
    fn fear_and_disgust_use_only_matching_custom_expressions_and_preserve_manual_priority() {
        use crate::behavior::BehaviorConfig;
        let id = Uuid::new_v4();
        for (emotion, alias) in [("fear", "Afraid"), ("disgust", "Disgusted")] {
            let mut library = AffectLibrary::default();
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
            library
                .apply_reply(
                    id,
                    ReplyAffect::parse(&JSON.replace("happy", emotion)).unwrap(),
                )
                .unwrap();
            for matching in [false, true] {
                let available = vec![if matching {
                    alias.into()
                } else {
                    "happy".into()
                }];
                let mut controller = BehaviorController::default();
                controller
                    .select(
                        Some(id),
                        &BehaviorConfig {
                            enabled: true,
                            ..Default::default()
                        },
                        &available,
                    )
                    .unwrap();
                let token = controller.begin_turn(id).unwrap();
                library
                    .drive(id, &mut controller, token, &available)
                    .unwrap();
                controller.tick(0.2);
                let weights = controller.weights_for(Some(id), &available, &BTreeMap::new(), "");
                if matching {
                    assert!(weights.get(alias).is_some_and(|w| *w > 0.));
                    assert!(
                        controller
                            .weights_for(
                                Some(id),
                                &available,
                                &BTreeMap::from([(alias.into(), 0.5)]),
                                ""
                            )
                            .is_empty()
                    );
                } else {
                    assert!(weights.is_empty(), "No unrelated preset fallback");
                }
            }
            assert_eq!(
                AffectLibrary::decode(&library.encode().unwrap())
                    .unwrap()
                    .entries,
                library.entries
            );
            let text = format!("Hello! {OPEN}{}{CLOSE}", JSON.replace("happy", emotion));
            for split in 0..=text.len() {
                let mut filter = ReplyAffectFilter::default();
                let mut spoken = filter.push(&text[..split]);
                spoken += &filter.push(&text[split..]);
                let result = filter.finish();
                spoken += &result.tail;
                assert_eq!(spoken, "Hello! ");
                assert_eq!(
                    serde_json::to_value(result.metadata.unwrap().emotion).unwrap(),
                    emotion
                );
            }
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
    fn tracked_transitions_report_previous_committed_label_and_stay_character_local() {
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut library = AffectLibrary::default();
        library.entries.insert(id, CharacterAffect::default());
        library.entries.insert(other, CharacterAffect::default());
        let unaffected = library.entries[&other].clone();
        let mut previous = AffectEmotion::Neutral;
        for emotion in [
            AffectEmotion::Happy,
            AffectEmotion::Sad,
            AffectEmotion::Neutral,
        ] {
            let reply = ReplyAffect {
                emotion,
                ..ReplyAffect::parse(JSON).unwrap()
            };
            assert_eq!(
                library.apply_reply_tracked(id, reply).unwrap(),
                Some(AffectTransition { previous, emotion })
            );
            assert_eq!(library.entries[&id].emotion, emotion);
            assert!(library.take_pending(id));
            assert!(!library.take_pending(id));
            assert_eq!(library.entries[&other], unaffected);
            assert!(!library.take_pending(other));
            previous = emotion;
        }
    }

    #[test]
    fn same_label_updates_mood_and_pending_without_another_transition() {
        let id = Uuid::new_v4();
        let mut library = AffectLibrary::default();
        library.entries.insert(id, CharacterAffect::default());
        let reply = ReplyAffect::parse(JSON).unwrap();
        assert!(library.apply_reply_tracked(id, reply).unwrap().is_some());
        assert!(library.take_pending(id));
        let previous_mood = library.entries[&id].mood;
        let influence = library.entries[&id].config.influence;
        assert_eq!(library.apply_reply_tracked(id, reply).unwrap(), None);
        assert!(library.take_pending(id));
        let mood = library.entries[&id].mood;
        assert!(mood.valence > previous_mood.valence);
        assert!(
            (mood.valence
                - (previous_mood.valence * (1.0 - influence) + reply.valence * influence))
                .abs()
                < 1e-12
        );
        assert_eq!(library.entries[&id].emotion, AffectEmotion::Happy);
    }

    #[test]
    fn rejected_tracked_applications_preserve_state_and_existing_pending_work() {
        let id = Uuid::new_v4();
        for had_pending in [false, true] {
            for invalid in 0..6 {
                let mut library = AffectLibrary::default();
                library.entries.insert(id, CharacterAffect::default());
                library
                    .apply_reply(id, ReplyAffect::parse(JSON).unwrap())
                    .unwrap();
                if !had_pending {
                    assert!(library.take_pending(id));
                }
                let mut reply = ReplyAffect {
                    emotion: AffectEmotion::Sad,
                    ..ReplyAffect::parse(JSON).unwrap()
                };
                let state = library.entries.get_mut(&id).unwrap();
                match invalid {
                    0 => reply.valence = f64::NAN,
                    1 => reply.arousal = f64::INFINITY,
                    2 => reply.dominance = f64::NEG_INFINITY,
                    3 => state.config.enabled = false,
                    4 => state.config.influence = 2.0,
                    5 => state.config.half_life_seconds = 0.0,
                    _ => unreachable!(),
                }
                let before = library.entries.clone();
                assert!(library.apply_reply_tracked(id, reply).is_err());
                assert_eq!(library.entries, before);
                assert_eq!(library.take_pending(id), had_pending);
            }
        }
        let mut library = AffectLibrary::default();
        assert!(
            library
                .apply_reply_tracked(id, ReplyAffect::parse(JSON).unwrap())
                .is_err()
        );
        assert!(library.entries.is_empty());
        assert!(!library.take_pending(id));
    }

    #[test]
    fn legacy_apply_wrapper_preserves_tracked_state_pending_and_errors() {
        let id = Uuid::new_v4();
        let mut legacy = AffectLibrary::default();
        legacy.entries.insert(id, CharacterAffect::default());
        let mut tracked = legacy.clone();
        let reply = ReplyAffect::parse(JSON).unwrap();
        for reply in [
            reply,
            reply,
            ReplyAffect {
                emotion: AffectEmotion::Neutral,
                ..reply
            },
            ReplyAffect {
                valence: f64::NAN,
                ..reply
            },
        ] {
            assert_eq!(
                legacy.apply_reply(id, reply),
                tracked.apply_reply_tracked(id, reply).map(|_| ())
            );
            assert_eq!(legacy.entries, tracked.entries);
            assert_eq!(legacy.take_pending(id), tracked.take_pending(id));
        }
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
        let mut off = CharacterAffect::default();
        off.config.enabled = false;
        library.entries.insert(other, off);
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
