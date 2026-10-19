//! Local fictional routine anchors. No scheduler, network calls or presence inference.
use serde::{Deserialize, Serialize};

pub const MAX_ANCHORS: usize = 32;
pub const MAX_LABEL_CHARS: usize = 120;
const WEEK: u32 = 7 * 24 * 60;
pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineAnchor {
    /// Monday = 0. An anchor remains current until the next weekly anchor.
    pub day: u8,
    pub minute: u16,
    pub activity: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutineConfig {
    pub enabled: bool,
    pub anchors: Vec<RoutineAnchor>,
}
pub struct RoutineView<'a> {
    pub current: &'a RoutineAnchor,
    pub next: &'a RoutineAnchor,
    pub minutes_until_next: u32,
}
impl RoutineConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.anchors.len() > MAX_ANCHORS {
            return Err("At most 32 routine anchors are supported.");
        }
        let mut times = std::collections::BTreeSet::new();
        for anchor in &self.anchors {
            if anchor.day >= 7 || anchor.minute >= 1440 {
                return Err("Choose a valid weekday and time.");
            }
            if anchor.activity.trim().is_empty()
                || anchor.activity.chars().count() > MAX_LABEL_CHARS
                || anchor.activity.chars().any(char::is_control)
            {
                return Err("Activity labels need 1–120 characters without control characters.");
            }
            if !times.insert((anchor.day, anchor.minute)) {
                return Err("Two activities cannot start at the same weekday and time.");
            }
        }
        Ok(())
    }
    /// Pure wall-clock lookup. No catch-up queue: after sleep or a clock change,
    /// evaluate only the current local weekday/minute. During DST repetition the
    /// same wall-clock activity appears again; skipped times have no side effects.
    pub fn at(&self, day: u8, minute: u16) -> Option<RoutineView<'_>> {
        if !self.enabled || day >= 7 || minute >= 1440 || self.validate().is_err() {
            return None;
        }
        let now = u32::from(day) * 1440 + u32::from(minute);
        let position = |a: &RoutineAnchor| u32::from(a.day) * 1440 + u32::from(a.minute);
        let current = self
            .anchors
            .iter()
            .min_by_key(|a| (now + WEEK - position(a)) % WEEK)?;
        // A matching anchor is current, not next. A lone anchor repeats next week.
        let distance = |a: &RoutineAnchor| {
            let delta = (position(a) + WEEK - now) % WEEK;
            if delta == 0 { WEEK } else { delta }
        };
        let next = self.anchors.iter().min_by_key(|a| distance(a))?;
        Some(RoutineView {
            current,
            next,
            minutes_until_next: distance(next),
        })
    }
}
/// Platform local timezone, including OS daylight-saving rules. No fixed-offset
/// approximation and no UTC fallback presented as local time.
#[cfg(unix)]
pub fn local_clock() -> Option<(u8, u16)> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let timestamp: libc::time_t = seconds.try_into().ok()?;
    let mut out = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: pointers reference valid storage; localtime_r initializes out on success.
    if unsafe { libc::localtime_r(&timestamp, out.as_mut_ptr()) }.is_null() {
        return None;
    }
    let out = unsafe { out.assume_init() };
    let day = u8::try_from((out.tm_wday + 6) % 7).ok()?;
    let minute = u16::try_from(out.tm_hour * 60 + out.tm_min).ok()?;
    (day < 7 && minute < 1440).then_some((day, minute))
}
#[cfg(not(unix))]
pub fn local_clock() -> Option<(u8, u16)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn anchor(day: u8, minute: u16, label: &str) -> RoutineAnchor {
        RoutineAnchor {
            day,
            minute,
            activity: label.into(),
        }
    }
    #[test]
    fn weekly_rollover_boundaries_and_unsorted_anchors() {
        let config = RoutineConfig {
            enabled: true,
            anchors: vec![anchor(6, 1380, "Reading"), anchor(0, 480, "Breakfast")],
        };
        let early = config.at(0, 0).unwrap();
        assert_eq!(early.current.activity, "Reading");
        assert_eq!(early.next.activity, "Breakfast");
        assert_eq!(early.minutes_until_next, 480);
        let exact = config.at(0, 480).unwrap();
        assert_eq!(exact.current.activity, "Breakfast");
        assert_eq!(exact.next.activity, "Reading");
        assert_eq!(config.at(6, 1439).unwrap().minutes_until_next, 481);
        let lone = RoutineConfig {
            enabled: true,
            anchors: vec![anchor(2, 600, "Drawing")],
        };
        assert_eq!(lone.at(2, 600).unwrap().minutes_until_next, WEEK);
    }
    #[test]
    fn invalid_or_disabled_schedules_never_produce_an_activity() {
        let mut config = RoutineConfig {
            enabled: false,
            anchors: vec![anchor(0, 0, "Tea")],
        };
        assert!(config.at(0, 0).is_none());
        config.enabled = true;
        for invalid in [
            anchor(7, 0, "Tea"),
            anchor(0, 1440, "Tea"),
            anchor(1, 0, " "),
            anchor(1, 0, "bad\nlabel"),
        ] {
            config.anchors.push(invalid);
            assert!(config.validate().is_err());
            assert!(config.at(0, 0).is_none());
            config.anchors.pop();
        }
        config.anchors.push(anchor(0, 0, "Duplicate"));
        assert!(config.validate().is_err());
        config.anchors = vec![anchor(0, 0, "Tea"); MAX_ANCHORS + 1];
        assert!(config.validate().is_err());
    }
    #[test]
    fn old_scene_profiles_remain_off_and_character_configs_are_independent() {
        let old: crate::data::SceneSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.routine, RoutineConfig::default());
        let mut edited = old.clone();
        edited.routine.enabled = true;
        edited.routine.anchors.push(anchor(0, 0, "Tea"));
        let restored: crate::data::SceneSettings =
            serde_json::from_str(&serde_json::to_string(&edited).unwrap()).unwrap();
        assert_eq!(restored.routine, edited.routine);
        assert!(old.routine.anchors.is_empty());
        assert!(!old.routine.enabled);
    }
}
