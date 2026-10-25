//! Opt-in local lifecycle and explicitly supplied affect; never interprets text,
//! calls a provider, launches tools, or drives blink/mouth/look channels.
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const MAX_CHANNELS: usize = 8;
const MAX_NAMES: usize = 256;
pub const MAX_INTEGRATION_CONTRIBUTIONS: usize = 8;
pub const MAX_INTEGRATION_DURATION_SECONDS: f32 = 60.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BehaviorConfig {
    pub enabled: bool,
    /// Optional user-selected mappings; empty names produce no lifecycle morph.
    pub idle_expression: String,
    pub thinking_expression: String,
    pub speaking_expression: String,
    pub lifecycle_strength: f32,
    pub emotion_max: f32,
    pub smoothing_seconds: f32,
    pub decay_seconds: f32,
}
impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_expression: String::new(),
            thinking_expression: String::new(),
            speaking_expression: String::new(),
            lifecycle_strength: 0.15,
            emotion_max: 0.6,
            smoothing_seconds: 0.2,
            decay_seconds: 3.0,
        }
    }
}
impl BehaviorConfig {
    pub fn validate(&self) -> Result<(), String> {
        for (value, min, max, name) in [
            (self.lifecycle_strength, 0.0, 0.35, "Lifecycle strength"),
            (self.emotion_max, 0.0, 0.65, "Emotion strength limit"),
            (self.smoothing_seconds, 0.02, 5.0, "Smoothing seconds"),
            (self.decay_seconds, 0.1, 60.0, "Decay seconds"),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(format!("{name} must be finite and between {min} and {max}"));
            }
        }
        for name in [
            &self.idle_expression,
            &self.thinking_expression,
            &self.speaking_expression,
        ] {
            if name.len() > 256 || name.chars().any(char::is_control) {
                return Err(
                    "Expression names must have at most 256 bytes and no control characters".into(),
                );
            }
            if !name.is_empty() && reserved_expression(name) {
                return Err("Behavior cannot own blink, mouth, or look expressions".into());
            }
        }
        Ok(())
    }
}

/// Canonical VRM channels and VRM0 aliases reserved for existing face systems.
pub fn reserved_expression(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("blink")
        || name.starts_with("look")
        || name.starts_with("mouth")
        || matches!(
            name.as_str(),
            "aa" | "ih" | "ou" | "ee" | "oh" | "a" | "i" | "u" | "e" | "o"
        )
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BehaviorPhase {
    #[default]
    Idle,
    Thinking,
    Speaking,
}
impl BehaviorPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Thinking => "Thinking",
            Self::Speaking => "Speaking",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BehaviorToken {
    pub character: Uuid,
    pub epoch: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BehaviorEvent {
    ThinkingFinished,
    SpeechStarted,
    SpeechFinished,
    Interrupted,
}
#[derive(Default)]
struct Channel {
    // Lifecycle and emotion must be independently revocable, even on one name.
    value: f64,
    emotion_value: f64,
    drive: f64,
    emotion_owner: Option<(BehaviorToken, u64)>,
}

/// Identifies one named emotion decision, including its smoothing tail.
#[derive(Clone, Debug)]
pub struct EmotionContribution {
    token: BehaviorToken,
    serial: u64,
    expression: String,
}

/// Opaque ownership of one temporary contribution, independent of chat epochs.
/// Tokens are unique across controllers and are never reused after expiry/reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IntegrationExpressionToken(Uuid);

struct IntegrationExpression {
    expression: String,
    strength: f32,
    remaining_seconds: f64,
}

/// All state is bounded, local, and deterministic given configuration and events.
#[derive(Default)]
pub struct BehaviorController {
    character: Option<Uuid>,
    epoch: u64,
    phase: BehaviorPhase,
    config: BehaviorConfig,
    available: BTreeSet<String>,
    channels: BTreeMap<String, Channel>,
    emotion_serial: u64,
    integration_expressions: BTreeMap<IntegrationExpressionToken, IntegrationExpression>,
}
impl BehaviorController {
    pub fn phase(&self) -> BehaviorPhase {
        self.phase
    }
    pub fn character(&self) -> Option<Uuid> {
        self.character
    }
    pub fn enabled(&self) -> bool {
        self.character.is_some() && self.config.enabled
    }
    pub fn token(&self) -> Option<BehaviorToken> {
        self.enabled().then(|| BehaviorToken {
            character: self.character.unwrap(),
            epoch: self.epoch,
        })
    }
    /// Call when selection/configuration/catalog changes (or each frame). A new
    /// character, disable, or removal clears outputs and invalidates old events.
    pub fn select(
        &mut self,
        character: Option<Uuid>,
        config: &BehaviorConfig,
        available: &[String],
    ) -> Result<(), String> {
        if let Err(error) = config.validate() {
            self.reset();
            self.character = character;
            self.config.enabled = false;
            return Err(error);
        }
        let names: BTreeSet<String> = available
            .iter()
            .take(MAX_NAMES)
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 256
                    && !name.chars().any(char::is_control)
                    && !reserved_expression(name)
            })
            .cloned()
            .collect();
        if self.character != character || (self.config.enabled && !config.enabled) {
            self.reset();
        }
        self.character = character;
        self.config = config.clone();
        self.available = names;
        self.channels
            .retain(|name, _| self.available.contains(name));
        self.integration_expressions
            .retain(|_, contribution| self.available.contains(&contribution.expression));
        if !self.enabled() {
            self.channels.clear();
            self.integration_expressions.clear();
            self.phase = BehaviorPhase::Idle;
        }
        if self.enabled() {
            for name in [
                &config.idle_expression,
                &config.thinking_expression,
                &config.speaking_expression,
            ] {
                if !name.is_empty() && !self.available.contains(name) {
                    // A loading/replaced avatar may not support a persisted mapping.
                    // Do not emit that name; the host can display this diagnostic.
                    self.reset();
                    self.config.enabled = false;
                    return Err(format!(
                        "Behavior expression is not available on this avatar: {name}"
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn begin_turn(&mut self, character: Uuid) -> Result<BehaviorToken, String> {
        if !self.enabled() || self.character != Some(character) {
            return Err("Behavior is disabled or the character does not match".into());
        }
        self.advance_epoch();
        self.phase = BehaviorPhase::Thinking;
        for channel in self.channels.values_mut() {
            channel.drive = 0.0;
        }
        self.token().ok_or_else(|| "Behavior is disabled".into())
    }
    fn check(&self, token: BehaviorToken) -> Result<(), String> {
        if self.token() == Some(token) {
            Ok(())
        } else {
            Err("Ignored stale behavior epoch, wrong character, or disabled controller".into())
        }
    }
    pub fn event(&mut self, token: BehaviorToken, event: BehaviorEvent) -> Result<(), String> {
        self.check(token)?;
        match event {
            BehaviorEvent::ThinkingFinished if self.phase == BehaviorPhase::Thinking => {
                self.phase = BehaviorPhase::Idle
            }
            BehaviorEvent::SpeechStarted => self.phase = BehaviorPhase::Speaking,
            BehaviorEvent::SpeechFinished if self.phase == BehaviorPhase::Speaking => {
                self.phase = BehaviorPhase::Idle;
                self.advance_epoch();
            }
            BehaviorEvent::Interrupted => {
                self.phase = BehaviorPhase::Idle;
                self.advance_epoch();
                for channel in self.channels.values_mut() {
                    channel.drive = 0.0;
                }
            }
            _ => return Err("Behavior event does not match the current lifecycle phase".into()),
        }
        Ok(())
    }
    /// The caller supplies an explicit named decision or manual preview. Raw
    /// conversation text is never an input. Unknown and reserved names fail shut.
    pub fn emotion(
        &mut self,
        token: BehaviorToken,
        name: &str,
        strength: f32,
    ) -> Result<(), String> {
        self.emotion_tracked(token, name, strength).map(|_| ())
    }
    pub fn emotion_tracked(
        &mut self,
        token: BehaviorToken,
        name: &str,
        strength: f32,
    ) -> Result<EmotionContribution, String> {
        self.check(token)?;
        if !strength.is_finite() {
            return Err("Emotion strength must be finite".into());
        }
        if !self.available.contains(name) || reserved_expression(name) {
            return Err("Choose a supported non-mouth, non-blink, non-look expression".into());
        }
        if !self.channels.contains_key(name) && self.channels.len() >= MAX_CHANNELS {
            return Err("Behavior channel budget exhausted; let previous expressions decay".into());
        }
        let serial = self
            .emotion_serial
            .checked_add(1)
            .ok_or("Emotion serial exhausted")?;
        self.emotion_serial = serial;
        for channel in self.channels.values_mut() {
            channel.drive = 0.0;
        }
        let channel = self.channels.entry(name.into()).or_default();
        channel.drive = strength.clamp(0.0, 1.0) as f64 * self.config.emotion_max as f64;
        channel.emotion_owner = Some((token, serial));
        Ok(EmotionContribution {
            token,
            serial,
            expression: name.into(),
        })
    }
    /// A newer explicit emotion (including a manual preview) supersedes replay.
    pub fn is_current_emotion(&self, contribution: &EmotionContribution) -> bool {
        self.token() == Some(contribution.token)
            && self.emotion_serial == contribution.serial
            && self
                .channels
                .get(&contribution.expression)
                .is_some_and(|channel| {
                    channel.emotion_owner == Some((contribution.token, contribution.serial))
                })
    }
    /// Revoke exactly this emotion and its tail, preserving lifecycle smoothing
    /// and later decisions. Old epochs may still have a tail, so cancellation
    /// checks the channel's owner rather than requiring the current turn token.
    pub fn cancel_emotion(&mut self, contribution: &EmotionContribution) -> bool {
        if self.character != Some(contribution.token.character) {
            return false;
        }
        let Some(channel) = self.channels.get_mut(&contribution.expression) else {
            return false;
        };
        if channel.emotion_owner != Some((contribution.token, contribution.serial)) {
            return false;
        }
        channel.emotion_value = 0.0;
        channel.drive = 0.0;
        channel.emotion_owner = None;
        true
    }
    /// Add an immediate temporary overlay without modifying emotion drives or
    /// lifecycle smoothing. Finite strength is clamped to [0, 1], then scaled by
    /// emotion_max at composition time. Duration must be finite in (0, 60].
    ///
    /// At most eight contributions coexist; a full budget rejects new requests
    /// without evicting another owner. On each name the strongest overlay wins,
    /// and is added to the evolving host baseline with a final clamp to [0, 1].
    /// Expiry uses tick's explicit elapsed time, with no residual smoothing tail.
    pub fn add_integration_expression(
        &mut self,
        character: Uuid,
        expression: &str,
        strength: f32,
        duration_seconds: f32,
    ) -> Result<IntegrationExpressionToken, String> {
        if !self.enabled() || self.character != Some(character) {
            return Err("Behavior is disabled or the integration character does not match".into());
        }
        if !self.available.contains(expression) || reserved_expression(expression) {
            return Err("Choose a supported non-mouth, non-blink, non-look expression".into());
        }
        if !strength.is_finite() {
            return Err("Integration expression strength must be finite".into());
        }
        if !duration_seconds.is_finite()
            || duration_seconds <= 0.0
            || duration_seconds > MAX_INTEGRATION_DURATION_SECONDS
        {
            return Err("Integration expression duration must be finite in (0, 60] seconds".into());
        }
        if self.integration_expressions.len() >= MAX_INTEGRATION_CONTRIBUTIONS {
            return Err("Integration expression budget exhausted; cancel or await expiry".into());
        }
        let token = IntegrationExpressionToken(Uuid::new_v4());
        self.integration_expressions.insert(
            token,
            IntegrationExpression {
                expression: expression.into(),
                strength: strength.clamp(0.0, 1.0),
                remaining_seconds: duration_seconds as f64,
            },
        );
        Ok(token)
    }
    /// Cancel only this owner's temporary overlay, immediately revealing the
    /// current host baseline. Expired, foreign and already-cancelled tokens fail.
    pub fn cancel_integration_expression(&mut self, token: IntegrationExpressionToken) -> bool {
        self.integration_expressions.remove(&token).is_some()
    }
    pub fn integration_expression_active(&self, token: IntegrationExpressionToken) -> bool {
        self.integration_expressions.contains_key(&token)
    }
    /// Exact integration of a decaying drive through a first-order smoother.
    /// The result depends on elapsed seconds, not the number of rendered frames.
    pub fn tick(&mut self, seconds: f32) {
        if !self.enabled() || !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        let dt = (seconds as f64).min(86_400.0);
        self.integration_expressions.retain(|_, contribution| {
            contribution.remaining_seconds -= dt;
            contribution.remaining_seconds > 0.0
        });
        let k = 1.0 / self.config.smoothing_seconds as f64;
        let lambda = 1.0 / self.config.decay_seconds as f64;
        let smooth = (-k * dt).exp();
        let decay = (-lambda * dt).exp();
        let lifecycle = match self.phase {
            BehaviorPhase::Idle => &self.config.idle_expression,
            BehaviorPhase::Thinking => &self.config.thinking_expression,
            BehaviorPhase::Speaking => &self.config.speaking_expression,
        }
        .clone();
        if !lifecycle.is_empty()
            && self.available.contains(&lifecycle)
            && (self.channels.contains_key(&lifecycle) || self.channels.len() < MAX_CHANNELS)
        {
            self.channels.entry(lifecycle.clone()).or_default();
        }
        for (name, channel) in &mut self.channels {
            let target = if *name == lifecycle {
                self.config.lifecycle_strength as f64
            } else {
                0.0
            };
            let driven = if (k - lambda).abs() < 1e-8 {
                k * channel.drive * dt * smooth
            } else {
                k * channel.drive * (decay - smooth) / (k - lambda)
            };
            channel.value = (channel.value * smooth + target * (1.0 - smooth)).clamp(0.0, 1.0);
            channel.emotion_value = (channel.emotion_value * smooth + driven).clamp(0.0, 1.0);
            channel.drive *= decay;
        }
        self.channels.retain(|name, channel| {
            *name == lifecycle
                || channel.value + channel.emotion_value > 1e-6
                || channel.drive > 1e-6
        });
    }
    /// Manual controls win globally. The host additionally excludes expression
    /// metadata with blink/mouth/look override flags and the configured mouth
    /// expression before merging this map; some custom names target those binds.
    pub fn weights_for(
        &self,
        character: Option<Uuid>,
        available: &[String],
        manual_weights: &BTreeMap<String, f32>,
        selected_expression: &str,
    ) -> BTreeMap<String, f32> {
        if !self.enabled()
            || character != self.character
            || !selected_expression.is_empty()
            || manual_weights
                .values()
                .any(|value| value.is_finite() && *value > 0.0)
        {
            return BTreeMap::new();
        }
        let mut weights: BTreeMap<String, f32> = self
            .channels
            .iter()
            .filter(|(name, channel)| {
                channel.value + channel.emotion_value > 1e-6
                    && !reserved_expression(name)
                    && available.contains(name)
            })
            .map(|(name, channel)| {
                (
                    name.clone(),
                    (channel.value + channel.emotion_value).clamp(0.0, 1.0) as f32,
                )
            })
            .collect();
        // Max is commutative: ownership-token order and request order cannot
        // amplify or change the result for overlapping integration expressions.
        let mut temporary = BTreeMap::<&str, f32>::new();
        for contribution in self.integration_expressions.values() {
            let name = &contribution.expression;
            let weight = contribution.strength * self.config.emotion_max;
            if weight > 1e-6 && !reserved_expression(name) && available.contains(name) {
                temporary
                    .entry(name.as_str())
                    .and_modify(|current| *current = current.max(weight))
                    .or_insert(weight);
            }
        }
        for (name, weight) in temporary {
            weights
                .entry(name.into())
                .and_modify(|baseline| *baseline = (*baseline + weight).clamp(0.0, 1.0))
                .or_insert(weight);
        }
        weights
    }
    /// Clear state immediately (disable, character removal, host teardown).
    pub fn reset(&mut self) {
        self.advance_epoch();
        self.phase = BehaviorPhase::Idle;
        self.channels.clear();
        self.integration_expressions.clear();
    }
    fn advance_epoch(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }
}

#[derive(Resource)]
pub struct BehaviorState {
    pub controller: BehaviorController,
    pub preview_expression: String,
    pub preview_strength: f32,
    pub error: Option<String>,
}

impl Default for BehaviorState {
    fn default() -> Self {
        Self {
            controller: BehaviorController::default(),
            preview_expression: String::new(),
            preview_strength: 0.7,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (BehaviorController, Uuid, Vec<String>) {
        let id = Uuid::new_v4();
        let names = vec![
            "happy".into(),
            "sad".into(),
            "relaxed".into(),
            "aa".into(),
            "blink".into(),
        ];
        let config = BehaviorConfig {
            enabled: true,
            ..Default::default()
        };
        let mut controller = BehaviorController::default();
        controller.select(Some(id), &config, &names).unwrap();
        (controller, id, names)
    }
    #[test]
    fn defaults_are_off_and_lifecycle_requires_matching_character_epoch() {
        let mut c = BehaviorController::default();
        let id = Uuid::new_v4();
        assert!(!c.enabled());
        assert!(c.begin_turn(id).is_err());
        let (mut c, id, _) = fixture();
        assert!(c.begin_turn(Uuid::new_v4()).is_err());
        let token = c.begin_turn(id).unwrap();
        assert_eq!(c.phase(), BehaviorPhase::Thinking);
        c.event(token, BehaviorEvent::ThinkingFinished).unwrap();
        assert_eq!(c.phase(), BehaviorPhase::Idle);
        c.event(token, BehaviorEvent::SpeechStarted).unwrap();
        assert_eq!(c.phase(), BehaviorPhase::Speaking);
        c.event(token, BehaviorEvent::SpeechFinished).unwrap();
        assert_eq!(c.phase(), BehaviorPhase::Idle);
        assert!(c.emotion(token, "happy", 1.0).is_err());
    }
    #[test]
    fn decay_and_smoothing_match_at_15_30_60_120_fps() {
        let mut observations = Vec::new();
        for fps in [15, 30, 60, 120] {
            let (mut c, id, names) = fixture();
            let token = c.begin_turn(id).unwrap();
            c.emotion(token, "happy", 1.0).unwrap();
            for _ in 0..fps * 2 {
                c.tick(1.0 / fps as f32);
            }
            observations.push(c.weights_for(Some(id), &names, &BTreeMap::new(), "")["happy"]);
            for _ in 0..fps * 60 {
                c.tick(1.0 / fps as f32);
            }
            assert!(
                c.weights_for(Some(id), &names, &BTreeMap::new(), "")
                    .is_empty()
            );
        }
        for value in &observations {
            assert!((*value - observations[0]).abs() < 1e-5);
        }
    }
    #[test]
    fn interruption_and_replacement_reject_stale_decisions() {
        let (mut c, id, names) = fixture();
        let old = c.begin_turn(id).unwrap();
        c.emotion(old, "happy", 1.0).unwrap();
        c.tick(0.2);
        c.event(old, BehaviorEvent::Interrupted).unwrap();
        assert!(c.event(old, BehaviorEvent::SpeechStarted).is_err());
        assert!(c.emotion(old, "sad", 1.0).is_err());
        let current = c.begin_turn(id).unwrap();
        assert!(c.emotion(old, "happy", 1.0).is_err());
        c.emotion(current, "sad", 1.0).unwrap();
        c.tick(0.2);
        assert!(
            c.weights_for(Some(id), &names, &BTreeMap::new(), "")
                .contains_key("sad")
        );
    }
    #[test]
    fn manual_priority_and_reserved_channels_preserve_existing_face_controls() {
        let (mut c, id, names) = fixture();
        let token = c.begin_turn(id).unwrap();
        for name in ["missing", "aa", "blink", "lookUp", "mouthCustom"] {
            assert!(c.emotion(token, name, 1.0).is_err());
        }
        assert!(c.emotion(token, "happy", f32::NAN).is_err());
        c.emotion(token, "happy", 99.0).unwrap();
        c.tick(1.0);
        let values = c.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert!(values["happy"].is_finite() && values["happy"] <= 0.65);
        assert!(
            c.weights_for(Some(id), &names, &BTreeMap::from([("sad".into(), 0.1)]), "")
                .is_empty()
        );
        assert!(
            c.weights_for(Some(id), &names, &BTreeMap::new(), "relaxed")
                .is_empty()
        );
        assert!(
            c.weights_for(Some(Uuid::new_v4()), &names, &BTreeMap::new(), "")
                .is_empty()
        );
    }
    #[test]
    fn disable_character_swap_reset_and_invalid_time_clean_up() {
        let (mut c, id, names) = fixture();
        let token = c.begin_turn(id).unwrap();
        c.emotion(token, "happy", 1.0).unwrap();
        c.tick(0.1);
        let before = c.weights_for(Some(id), &names, &BTreeMap::new(), "");
        for dt in [f32::NAN, f32::INFINITY, -1.0] {
            c.tick(dt);
        }
        assert_eq!(
            before,
            c.weights_for(Some(id), &names, &BTreeMap::new(), "")
        );
        let off = BehaviorConfig {
            enabled: false,
            ..Default::default()
        };
        c.select(Some(id), &off, &names).unwrap();
        assert!(!c.enabled());
        assert!(c.channels.is_empty());
        assert!(c.event(token, BehaviorEvent::SpeechStarted).is_err());
        c.select(
            Some(Uuid::new_v4()),
            &BehaviorConfig {
                enabled: true,
                ..Default::default()
            },
            &names,
        )
        .unwrap();
        assert!(c.emotion(token, "happy", 1.0).is_err());
        c.reset();
        assert!(c.channels.is_empty());
        assert_eq!(c.phase(), BehaviorPhase::Idle);
        c.select(None, &BehaviorConfig::default(), &[]).unwrap();
        assert!(c.token().is_none());
    }
    #[test]
    fn cancelling_emotion_preserves_shared_lifecycle_and_unrelated_channels() {
        let (mut controller, id, names) = fixture();
        let config = BehaviorConfig {
            thinking_expression: "happy".into(),
            ..Default::default()
        };
        controller.select(Some(id), &config, &names).unwrap();
        let token = controller.begin_turn(id).unwrap();
        let mut lifecycle = BehaviorController::default();
        lifecycle.select(Some(id), &config, &names).unwrap();
        lifecycle.begin_turn(id).unwrap();
        let mapped = controller.emotion_tracked(token, "happy", 1.0).unwrap();
        controller.tick(0.2);
        lifecycle.tick(0.2);
        controller.emotion(token, "sad", 0.7).unwrap();
        controller.tick(0.2);
        lifecycle.tick(0.2);
        let before = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert!(controller.cancel_emotion(&mapped));
        let after = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert_eq!(
            after["happy"],
            lifecycle.weights_for(Some(id), &names, &BTreeMap::new(), "")["happy"]
        );
        assert_eq!(after["sad"], before["sad"]);
        assert_eq!(controller.token(), Some(token));
        assert_eq!(controller.phase(), BehaviorPhase::Thinking);
        assert!(!controller.cancel_emotion(&mapped));
    }

    #[test]
    fn cancellation_cannot_remove_later_manual_preview_on_same_expression() {
        let (mut controller, id, names) = fixture();
        let token = controller.begin_turn(id).unwrap();
        let mapped = controller.emotion_tracked(token, "happy", 0.8).unwrap();
        controller.tick(0.2);
        controller.emotion(token, "happy", 0.4).unwrap();
        controller.tick(0.2);
        let before = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert!(!controller.is_current_emotion(&mapped));
        assert!(!controller.cancel_emotion(&mapped));
        assert_eq!(
            controller.weights_for(Some(id), &names, &BTreeMap::new(), ""),
            before
        );
    }

    #[test]
    fn completed_epoch_tail_can_be_revoked_without_resetting_new_epoch() {
        let (mut controller, id, names) = fixture();
        let token = controller.begin_turn(id).unwrap();
        let mapped = controller.emotion_tracked(token, "happy", 0.8).unwrap();
        controller
            .event(token, BehaviorEvent::SpeechStarted)
            .unwrap();
        controller.tick(0.2);
        controller
            .event(token, BehaviorEvent::SpeechFinished)
            .unwrap();
        let current = controller.token();
        assert_ne!(current, Some(token));
        assert!(!controller.is_current_emotion(&mapped));
        assert!(controller.cancel_emotion(&mapped));
        assert!(
            controller
                .weights_for(Some(id), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        assert_eq!(controller.token(), current);
    }

    #[test]
    fn integration_cancel_and_expiry_restore_evolving_host_baseline() {
        for expire in [false, true] {
            let (mut controller, id, names) = fixture();
            let config = BehaviorConfig {
                thinking_expression: "happy".into(),
                ..Default::default()
            };
            let mut baseline = BehaviorController::default();
            controller.select(Some(id), &config, &names).unwrap();
            baseline.select(Some(id), &config, &names).unwrap();
            let token = controller.begin_turn(id).unwrap();
            let baseline_token = baseline.begin_turn(id).unwrap();
            let host = controller.emotion_tracked(token, "happy", 0.5).unwrap();
            baseline.emotion(baseline_token, "happy", 0.5).unwrap();
            let shared = controller
                .add_integration_expression(id, "happy", 0.6, 0.5)
                .unwrap();
            let extra = controller
                .add_integration_expression(id, "sad", 0.5, 0.5)
                .unwrap();
            assert!(controller.is_current_emotion(&host));
            controller.tick(0.25);
            baseline.tick(0.25);
            let with_overlay = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
            let without_overlay = baseline.weights_for(Some(id), &names, &BTreeMap::new(), "");
            assert!(with_overlay["happy"] > without_overlay["happy"]);
            assert!(with_overlay["sad"] > 0.0);
            // A fresh host decision must evolve underneath the temporary layer.
            controller.emotion(token, "relaxed", 0.4).unwrap();
            baseline.emotion(baseline_token, "relaxed", 0.4).unwrap();
            controller.tick(0.125);
            baseline.tick(0.125);
            if expire {
                controller.tick(0.125);
                baseline.tick(0.125);
                assert!(!controller.cancel_integration_expression(shared));
                assert!(!controller.cancel_integration_expression(extra));
            } else {
                assert!(controller.cancel_integration_expression(shared));
                assert!(controller.cancel_integration_expression(extra));
            }
            assert_eq!(
                controller.weights_for(Some(id), &names, &BTreeMap::new(), ""),
                baseline.weights_for(Some(id), &names, &BTreeMap::new(), "")
            );
            assert_eq!(controller.token(), Some(token));
            assert_eq!(controller.phase(), BehaviorPhase::Thinking);
            controller.tick(0.25);
            baseline.tick(0.25);
            assert_eq!(
                controller.weights_for(Some(id), &names, &BTreeMap::new(), ""),
                baseline.weights_for(Some(id), &names, &BTreeMap::new(), "")
            );
        }
    }

    #[test]
    fn integration_ownership_rejects_stale_and_foreign_cancellation() {
        let (mut controller, id, names) = fixture();
        let mut foreign = BehaviorController::default();
        foreign
            .select(Some(id), &Default::default(), &names)
            .unwrap();
        let expired = controller
            .add_integration_expression(id, "happy", 0.8, 0.25)
            .unwrap();
        let live = controller
            .add_integration_expression(id, "happy", 0.4, 0.5)
            .unwrap();
        let foreign_live = foreign
            .add_integration_expression(id, "happy", 0.9, 1.0)
            .unwrap();
        assert!(!foreign.cancel_integration_expression(live));
        assert!(!controller.cancel_integration_expression(foreign_live));
        controller.tick(0.25);
        let before = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert!(!controller.cancel_integration_expression(expired));
        assert_eq!(
            controller.weights_for(Some(id), &names, &BTreeMap::new(), ""),
            before
        );
        assert!(controller.cancel_integration_expression(live));
        assert!(!controller.cancel_integration_expression(live));
        let after = controller
            .add_integration_expression(id, "happy", 0.7, 0.5)
            .unwrap();
        assert_ne!(after, live);
        assert!(!controller.cancel_integration_expression(live));
        assert!(controller.cancel_integration_expression(after));
        assert!(foreign.cancel_integration_expression(foreign_live));
    }

    #[test]
    fn integration_overlaps_use_strongest_weight_independent_of_request_order() {
        let mut observations = Vec::new();
        for strengths in [[0.2, 0.8, 0.5], [0.5, 0.8, 0.2]] {
            let (mut controller, id, names) = fixture();
            let mut strongest = None;
            for strength in strengths {
                let token = controller
                    .add_integration_expression(id, "happy", strength, 1.0)
                    .unwrap();
                if strength == 0.8 {
                    strongest = Some(token);
                }
            }
            let weights = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
            assert_eq!(weights["happy"], 0.8 * controller.config.emotion_max);
            observations.push(weights);
            assert!(controller.cancel_integration_expression(strongest.unwrap()));
            assert_eq!(
                controller.weights_for(Some(id), &names, &BTreeMap::new(), "")["happy"],
                0.5 * controller.config.emotion_max
            );
        }
        assert_eq!(observations[0], observations[1]);
    }

    #[test]
    fn integration_budget_and_repeated_replacement_cannot_accumulate() {
        let (mut controller, id, names) = fixture();
        let mut tokens = Vec::new();
        for _ in 0..MAX_INTEGRATION_CONTRIBUTIONS {
            tokens.push(
                controller
                    .add_integration_expression(id, "happy", 1.0, 1.0)
                    .unwrap(),
            );
        }
        let before = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert!(
            controller
                .add_integration_expression(id, "sad", 1.0, 1.0)
                .is_err()
        );
        assert_eq!(before["happy"], controller.config.emotion_max);
        assert_eq!(
            controller.weights_for(Some(id), &names, &BTreeMap::new(), ""),
            before
        );
        assert!(
            controller.channels.is_empty(),
            "Integrations cannot consume host channel slots"
        );
        for token in tokens {
            assert!(controller.cancel_integration_expression(token));
        }
        for _ in 0..100 {
            let token = controller
                .add_integration_expression(id, "happy", 1.0, 1.0)
                .unwrap();
            assert!(controller.cancel_integration_expression(token));
            assert!(controller.integration_expressions.is_empty());
            assert!(
                controller
                    .weights_for(Some(id), &names, &BTreeMap::new(), "")
                    .is_empty()
            );
        }
        controller
            .add_integration_expression(id, "sad", 1.0, 1.0)
            .unwrap();
        controller.tick(1.0);
        assert!(controller.integration_expressions.is_empty());
    }

    #[test]
    fn integration_validation_manual_and_reserved_channel_precedence() {
        let (mut controller, id, names) = fixture();
        for expression in ["missing", "aa", "blink", "lookLeft", "mouthCustom"] {
            assert!(
                controller
                    .add_integration_expression(id, expression, 1.0, 1.0)
                    .is_err()
            );
        }
        for strength in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                controller
                    .add_integration_expression(id, "happy", strength, 1.0)
                    .is_err()
            );
        }
        for duration in [
            f32::NAN,
            f32::INFINITY,
            -1.0,
            0.0,
            MAX_INTEGRATION_DURATION_SECONDS + 1.0,
        ] {
            assert!(
                controller
                    .add_integration_expression(id, "happy", 1.0, duration)
                    .is_err()
            );
        }
        assert!(
            controller
                .add_integration_expression(Uuid::new_v4(), "happy", 1.0, 1.0)
                .is_err()
        );
        assert!(controller.integration_expressions.is_empty());
        let zero = controller
            .add_integration_expression(id, "sad", -2.0, 1.0)
            .unwrap();
        let high = controller
            .add_integration_expression(id, "happy", 99.0, 1.0)
            .unwrap();
        let before = controller.weights_for(Some(id), &names, &BTreeMap::new(), "");
        assert_eq!(before.len(), 1);
        assert_eq!(before["happy"], controller.config.emotion_max);
        for manual in ["happy", "sad", "aa", "blink", "lookLeft"] {
            assert!(
                controller
                    .weights_for(
                        Some(id),
                        &names,
                        &BTreeMap::from([(manual.into(), 0.3)]),
                        ""
                    )
                    .is_empty()
            );
        }
        assert!(
            controller
                .weights_for(Some(id), &names, &BTreeMap::new(), "relaxed")
                .is_empty()
        );
        assert!(
            controller
                .weights_for(Some(Uuid::new_v4()), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        assert!(
            controller
                .weights_for(Some(id), &[], &BTreeMap::new(), "")
                .is_empty()
        );
        for seconds in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            controller.tick(seconds);
            assert_eq!(
                controller.weights_for(Some(id), &names, &BTreeMap::new(), ""),
                before
            );
        }
        assert!(controller.cancel_integration_expression(zero));
        assert!(controller.cancel_integration_expression(high));
    }

    #[test]
    fn integration_survives_chat_epochs_but_clears_on_host_teardown_or_catalog_removal() {
        for transition in ["reset", "disable", "character", "catalog"] {
            let (mut controller, id, names) = fixture();
            let overlay = controller
                .add_integration_expression(id, "happy", 0.5, 2.0)
                .unwrap();
            let turn = controller.begin_turn(id).unwrap();
            controller.event(turn, BehaviorEvent::Interrupted).unwrap();
            assert!(controller.weights_for(Some(id), &names, &BTreeMap::new(), "")["happy"] > 0.0);
            match transition {
                "reset" => controller.reset(),
                "disable" => controller
                    .select(
                        Some(id),
                        &BehaviorConfig {
                            enabled: false,
                            ..Default::default()
                        },
                        &names,
                    )
                    .unwrap(),
                "character" => controller
                    .select(Some(Uuid::new_v4()), &Default::default(), &names)
                    .unwrap(),
                "catalog" => controller
                    .select(Some(id), &Default::default(), &["sad".into()])
                    .unwrap(),
                _ => unreachable!(),
            }
            assert!(!controller.cancel_integration_expression(overlay));
            assert!(controller.integration_expressions.is_empty());
            controller
                .select(Some(id), &Default::default(), &names)
                .unwrap();
            assert!(
                controller
                    .weights_for(Some(id), &names, &BTreeMap::new(), "")
                    .is_empty()
            );
        }
    }

    #[test]
    fn config_limits_equal_time_constants_and_channels_are_bounded() {
        let (mut c, id, mut names) = fixture();
        let mut config = BehaviorConfig {
            enabled: true,
            smoothing_seconds: 1.0,
            decay_seconds: 1.0,
            thinking_expression: "relaxed".into(),
            ..Default::default()
        };
        c.select(Some(id), &config, &names).unwrap();
        let token = c.begin_turn(id).unwrap();
        c.emotion(token, "happy", 1.0).unwrap();
        c.tick(1.0);
        assert!(c.channels.values().all(|channel| channel.value.is_finite()));
        assert!(
            c.weights_for(Some(id), &names, &BTreeMap::new(), "")
                .contains_key("relaxed")
        );
        config.emotion_max = f32::NAN;
        assert!(config.validate().is_err());
        config.emotion_max = 0.6;
        config.thinking_expression = "blink".into();
        assert!(config.validate().is_err());
        names.extend((0..10).map(|n| format!("custom{n}")));
        c.select(
            Some(id),
            &BehaviorConfig {
                enabled: true,
                ..Default::default()
            },
            &names,
        )
        .unwrap();
        for n in 0..10 {
            let _ = c.emotion(token, &format!("custom{n}"), 1.0);
            c.tick(0.01);
        }
        assert!(c.channels.len() <= MAX_CHANNELS);
    }
}
