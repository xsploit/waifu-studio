//! Local playlists: emotion gestures take priority over talking and idle.
use super::motion_library::{MotionCategory, MotionEntry};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use waifu_studio::data::CompanionMotionConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Idle,
    Talking,
}
#[derive(Default)]
pub struct CompanionAnimator {
    owner: Option<(uuid::Uuid, u64)>,
    current: Option<(Role, PathBuf)>,
    idle_index: usize,
    talking_index: usize,
    next: Option<Instant>,
    pub paused: bool,
    pub speech_started: bool,
    pub loading_path: Option<PathBuf>,
    pub cache:
        std::collections::VecDeque<(PathBuf, std::sync::Arc<waifu_studio::animation::BvhClip>)>,
    pub note: Option<String>,
    failed: Vec<PathBuf>,
}
fn eligible(entry: &MotionEntry) -> bool {
    if !entry.auto_playable() {
        return false;
    }
    let name = entry.label.to_lowercase();
    !["sit", "kneel", "lay", "crawl"]
        .iter()
        .any(|word| name.contains(word))
}
pub fn playlist(entries: &[MotionEntry], configured: &[PathBuf], talking: bool) -> Vec<PathBuf> {
    if !configured.is_empty() {
        return configured
            .iter()
            .filter(|path| entries.iter().any(|e| e.path == **path && eligible(e)))
            .cloned()
            .collect();
    }
    let category = if talking {
        MotionCategory::Talking
    } else {
        MotionCategory::Idle
    };
    entries
        .iter()
        .filter(|e| eligible(e) && e.category() == category)
        .map(|e| e.path.clone())
        .collect()
}
impl CompanionAnimator {
    pub fn manual_override(&mut self) {
        self.paused = true;
        self.note = Some("Paused by manual motion controls".into());
    }
    pub fn resume(&mut self) {
        self.paused = false;
        self.current = None;
        self.failed.clear();
        self.next = None;
    }
    pub fn failed(&mut self, path: PathBuf, now: Instant) {
        if !self.failed.contains(&path) {
            self.failed.push(path);
        }
        self.current = None;
        self.next = Some(now + Duration::from_secs(2));
        self.note =
            Some("Skipped a motion that could not be loaded; rescan or Resume to retry".into());
    }
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        now: Instant,
        config: &CompanionMotionConfig,
        owner: Option<(uuid::Uuid, u64)>,
        talking: bool,
        gesture: bool,
        loading: bool,
        current_path: Option<&Path>,
        entries: &[MotionEntry],
    ) -> Option<PathBuf> {
        if self.owner != owner {
            *self = Self {
                owner,
                ..Self::default()
            };
        }
        if !config.enabled || owner.is_none() || self.paused || loading {
            return None;
        }
        if gesture {
            self.current = None;
            self.next = None;
            return None;
        }
        if self.current.is_none() && self.next.is_some_and(|deadline| now < deadline) {
            return None;
        }
        let role = if talking { Role::Talking } else { Role::Idle };
        if self.current.as_ref().is_some_and(|(old_role, path)| {
            *old_role == role && Some(path.as_path()) == current_path
        }) && self.next.is_some_and(|deadline| now < deadline)
        {
            return None;
        }
        let configured = if talking {
            &config.talking_paths
        } else {
            &config.idle_paths
        };
        let paths: Vec<_> = playlist(entries, configured, talking)
            .into_iter()
            .filter(|path| !self.failed.contains(path))
            .collect();
        if paths.is_empty() {
            self.note = Some(
                if talking {
                    "No available talking motions"
                } else {
                    "No available idle motions"
                }
                .into(),
            );
            return None;
        }
        let index = if talking {
            &mut self.talking_index
        } else {
            &mut self.idle_index
        };
        let mut chosen = paths[*index % paths.len()].clone();
        *index = index.wrapping_add(1);
        if paths.len() > 1 && Some(chosen.as_path()) == current_path {
            chosen = paths[*index % paths.len()].clone();
            *index = index.wrapping_add(1);
        }
        self.current = Some((role, chosen.clone()));
        self.next = Some(now + Duration::from_secs(if talking { 8 } else { 14 }));
        self.note = Some(
            if talking {
                "Talking playlist"
            } else {
                "Idle playlist"
            }
            .into(),
        );
        (Some(chosen.as_path()) != current_path).then_some(chosen)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn entries() -> Vec<MotionEntry> {
        [
            "idle-one.bvh",
            "idle-two.bvh",
            "talking-one.bvh",
            "talking-two.bvh",
            "sit_idle.bvh",
            "walk.bvh",
        ]
        .into_iter()
        .map(|name| MotionEntry {
            path: name.into(),
            label: name.into(),
            group: "test".into(),
            format: "BVH",
        })
        .collect()
    }
    #[test]
    fn playlists_preserve_scanned_choices_and_reject_seated_or_missing() {
        let entries = entries();
        assert_eq!(playlist(&entries, &[], false).len(), 2);
        assert_eq!(playlist(&entries, &[], true).len(), 2);
        assert!(
            playlist(
                &entries,
                &[
                    "walk.bvh".into(),
                    "sit_idle.bvh".into(),
                    "missing.bvh".into()
                ],
                true
            )
            .is_empty()
        );
        assert_eq!(
            playlist(&entries, &["idle-two.bvh".into()], true),
            [PathBuf::from("idle-two.bvh")]
        );
    }
    #[test]
    fn speech_switches_immediately_gesture_finishes_then_resumes_current_speech_state() {
        let now = Instant::now();
        let config = CompanionMotionConfig::default();
        let entries = entries();
        let mut animator = CompanionAnimator::default();
        let owner = Some((uuid::Uuid::new_v4(), 1));
        let idle = animator
            .frame(now, &config, owner, false, false, false, None, &entries)
            .unwrap();
        let talk = animator
            .frame(
                now,
                &config,
                owner,
                true,
                false,
                false,
                Some(&idle),
                &entries,
            )
            .unwrap();
        assert!(talk.to_string_lossy().contains("talking"));
        assert!(
            animator
                .frame(
                    now,
                    &config,
                    owner,
                    true,
                    true,
                    false,
                    Some(Path::new("emotion.bvh")),
                    &entries
                )
                .is_none()
        );
        assert!(
            animator
                .frame(
                    now,
                    &config,
                    owner,
                    true,
                    false,
                    true,
                    Some(&talk),
                    &entries
                )
                .is_none()
        );
        let resumed = animator
            .frame(
                now,
                &config,
                owner,
                true,
                false,
                false,
                Some(Path::new("emotion.bvh")),
                &entries,
            )
            .unwrap();
        assert!(resumed.to_string_lossy().contains("talking"));
        let idle = animator
            .frame(
                now,
                &config,
                owner,
                false,
                false,
                false,
                Some(&resumed),
                &entries,
            )
            .unwrap();
        assert!(idle.to_string_lossy().contains("idle"));
        animator.manual_override();
        assert!(
            animator
                .frame(
                    now + Duration::from_secs(30),
                    &config,
                    owner,
                    true,
                    false,
                    false,
                    Some(&idle),
                    &entries
                )
                .is_none()
        );
        animator.resume();
        assert!(
            animator
                .frame(
                    now,
                    &config,
                    owner,
                    true,
                    false,
                    false,
                    Some(&idle),
                    &entries
                )
                .is_some()
        );
    }
    #[test]
    fn rotates_without_each_frame_imports_and_failed_files_do_not_spin() {
        let now = Instant::now();
        let config = CompanionMotionConfig::default();
        let entries = entries();
        let owner = Some((uuid::Uuid::new_v4(), 1));
        let mut animator = CompanionAnimator::default();
        let first = animator
            .frame(now, &config, owner, false, false, false, None, &entries)
            .unwrap();
        assert!(
            animator
                .frame(
                    now + Duration::from_secs(1),
                    &config,
                    owner,
                    false,
                    false,
                    false,
                    Some(&first),
                    &entries
                )
                .is_none()
        );
        let next = animator
            .frame(
                now + Duration::from_secs(15),
                &config,
                owner,
                false,
                false,
                false,
                Some(&first),
                &entries,
            )
            .unwrap();
        assert_ne!(next, first);
        animator.failed(next, now);
        assert!(
            animator
                .frame(now, &config, owner, false, false, false, None, &entries)
                .is_none()
        );
        assert!(
            animator
                .frame(
                    now + Duration::from_secs(3),
                    &config,
                    owner,
                    false,
                    false,
                    false,
                    None,
                    &entries
                )
                .is_some()
        );
    }
}
