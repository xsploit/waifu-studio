//! Change-triggered drafts, never automatic promotion into recalled memory.
use super::{FactsMemoryState, MAX_CANDIDATES, RecordKind, reflection::ReflectionConfig};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomaticReflectionConfig {
    pub enabled: bool,
    pub new_records: u32,
    pub cooldown_minutes: u32,
    pub connection: ReflectionConfig,
}
impl Default for AutomaticReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            new_records: 3,
            cooldown_minutes: 30,
            connection: Default::default(),
        }
    }
}
#[derive(Default)]
struct Checkpoint {
    seen: HashSet<Uuid>,
    attempted: Option<Instant>,
}
#[derive(Default)]
pub struct AutomaticReflectionState {
    checkpoints: HashMap<Uuid, Checkpoint>,
    active: Option<(Uuid, AutomaticReflectionConfig)>,
    pub status: String,
}
impl AutomaticReflectionState {
    pub(super) fn finished(&mut self) {
        self.active = None;
    }
    fn due(
        &mut self,
        character: Uuid,
        ids: HashSet<Uuid>,
        config: &AutomaticReflectionConfig,
        at: Instant,
        blocked: bool,
    ) -> bool {
        // No catch-up on startup, selection or first enable. At most 128 characters
        // are tracked per process; fail closed beyond that rather than evict cooldowns.
        if !self.checkpoints.contains_key(&character) {
            if self.checkpoints.len() >= 128 {
                return false;
            }
            self.checkpoints.insert(
                character,
                Checkpoint {
                    seen: ids,
                    attempted: None,
                },
            );
            return false;
        }
        let checkpoint = self.checkpoints.get_mut(&character).unwrap();
        if !config.enabled {
            checkpoint.seen = ids;
            return false;
        }
        let cooldown = Duration::from_secs(u64::from(config.cooldown_minutes.clamp(10, 1440)) * 60);
        if blocked
            || checkpoint
                .attempted
                .is_some_and(|last| at.saturating_duration_since(last) < cooldown)
            || ids.difference(&checkpoint.seen).count() < config.new_records.clamp(1, 12) as usize
        {
            return false;
        }
        // Count attempts, including errors/abstentions/cancellation. Never retry an
        // unchanged snapshot each frame or when toggling the setting.
        checkpoint.seen = ids;
        checkpoint.attempted = Some(at);
        true
    }
}
impl FactsMemoryState {
    /// Call before poll(): disabling/changing consent discards an in-flight automatic
    /// result before it can enter the pending queue. Manual requests are unaffected.
    pub fn automatic_reflection_frame(&mut self, config: &AutomaticReflectionConfig) {
        if let Some((character, previous)) = &self.automatic.active {
            if !config.enabled
                || !self.enabled
                || self.selected != Some(*character)
                || previous != config
            {
                self.reflection.cancel();
                self.automatic.active = None;
                self.automatic.status =
                    "Automatic draft cancelled: character or settings changed.".into();
            } else if !self.reflection.running() {
                self.automatic.active = None;
            }
        }
        if !self.loaded || self.busy {
            return;
        }
        let Some(character) = self.selected else {
            return;
        };
        let sources = self.reflection_sources();
        let ids = sources.iter().map(|s| s.id).collect();
        let blocked = !self.enabled
            || self.reflection.running()
            || self.reflection.shutting_down()
            || self.pending_error.is_some()
            || self.candidates.len() >= MAX_CANDIDATES
            || self
                .candidates
                .iter()
                .any(|r| r.kind == RecordKind::Reflection);
        if !self
            .automatic
            .due(character, ids, config, Instant::now(), blocked)
        {
            return;
        }
        match self.start_reflection_with(config.connection.clone()) {
            Ok(()) => {
                self.automatic.active = Some((character, config.clone()));
                self.automatic.status =
                    "Automatic reflection requested; the result still requires review.".into();
            }
            Err(error) => {
                self.automatic.status = format!("Automatic reflection did not start: {error}")
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ids(values: &[u128]) -> HashSet<Uuid> {
        values.iter().map(|id| Uuid::from_u128(*id)).collect()
    }
    #[test]
    fn no_startup_catchup_and_attempts_need_new_evidence_and_cooldown() {
        let mut state = AutomaticReflectionState::default();
        let config = AutomaticReflectionConfig {
            enabled: true,
            new_records: 2,
            cooldown_minutes: 10,
            ..Default::default()
        };
        let character = Uuid::new_v4();
        let at = Instant::now();
        assert!(!state.due(character, ids(&[1, 2, 3]), &config, at, false));
        assert!(!state.due(character, ids(&[1, 2, 3, 4]), &config, at, false));
        assert!(state.due(character, ids(&[1, 2, 3, 4, 5]), &config, at, false));
        assert!(!state.due(
            character,
            ids(&[1, 2, 3, 4, 5, 6, 7]),
            &config,
            at + Duration::from_secs(599),
            false
        ));
        assert!(state.due(
            character,
            ids(&[1, 2, 3, 4, 5, 6, 7]),
            &config,
            at + Duration::from_secs(600),
            false
        ));
        assert!(!state.due(
            character,
            ids(&[1, 2, 3, 4, 5, 6, 7]),
            &config,
            at + Duration::from_secs(1200),
            false
        ));
    }
    #[test]
    fn disabled_and_blocked_do_not_send_or_consume_new_evidence() {
        let mut state = AutomaticReflectionState::default();
        let mut config = AutomaticReflectionConfig {
            new_records: 0,
            cooldown_minutes: 0,
            ..Default::default()
        };
        let character = Uuid::new_v4();
        let at = Instant::now();
        assert!(!state.due(character, ids(&[1]), &config, at, false));
        assert!(!state.due(character, ids(&[1, 2]), &config, at, false));
        config.enabled = true;
        assert!(!state.due(character, ids(&[1, 2]), &config, at, false));
        assert!(!state.due(character, ids(&[1, 2, 3]), &config, at, true));
        assert!(state.due(character, ids(&[1, 2, 3]), &config, at, false));
        assert!(!state.due(Uuid::new_v4(), ids(&[1, 2, 3]), &config, at, false));
        assert!(
            !serde_json::from_str::<AutomaticReflectionConfig>("{}")
                .unwrap()
                .enabled
        );
    }
}
