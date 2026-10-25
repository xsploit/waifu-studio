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
/// Playback for the selected character, including the utterance lifetime.
pub struct SpeechActivity {
    pub generation: u64,
    pub playing: bool,
    pub pending: bool,
}
const MAX_SPEECH_GAP: Duration = Duration::from_millis(750);
#[derive(Default)]
pub struct CompanionAnimator {
    owner: Option<(uuid::Uuid, u64)>,
    current: Option<(Role, PathBuf)>,
    idle_index: usize,
    talking_index: usize,
    next: Option<Instant>,
    pub paused: bool,
    speech_generation: Option<u64>,
    last_audio: Option<Instant>,
    pub loading_path: Option<PathBuf>,
    pub cache:
        std::collections::VecDeque<(PathBuf, std::sync::Arc<waifu_studio::animation::BvhClip>)>,
    pub note: Option<String>,
    failed: Vec<PathBuf>,
}
fn eligible(entry: &MotionEntry) -> bool {
    if !entry.auto_playable()
        || entry.metadata.as_ref().is_some_and(|m| {
            m.loopable == Some(false)
                || m.role == waifu_studio::extension_manifest::MotionRole::Gesture
        })
    {
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
            .filter_map(|path| super::motion_library::saved_motion(entries, path))
            .filter(|entry| eligible(entry))
            .map(|entry| entry.path.clone())
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
    /// Reset ownership before reading either the manual hold or speech state.
    /// The root identity also changes when the same character's VRM is reloaded.
    pub fn select_owner(&mut self, owner: Option<(uuid::Uuid, u64)>) -> bool {
        if self.owner != owner {
            *self = Self {
                owner,
                ..Self::default()
            };
            return true;
        }
        false
    }
    /// Start talking only after this owner's audio actually begins. Brief gaps
    /// while more speech is pending retain that state; a new owner cannot inherit it.
    pub fn speech_frame(
        &mut self,
        now: Instant,
        owner: Option<(uuid::Uuid, u64)>,
        speech: Option<SpeechActivity>,
    ) -> bool {
        self.select_owner(owner);
        let speech = speech.filter(|_| owner.is_some());
        let generation = speech.as_ref().map(|speech| speech.generation);
        if self.speech_generation != generation {
            self.last_audio = None;
            self.speech_generation = generation;
        }
        let Some(speech) = speech else { return false };
        if speech.playing {
            self.last_audio = Some(now);
            return true;
        }
        let bridging = speech.pending
            && self
                .last_audio
                .is_some_and(|last| now.saturating_duration_since(last) <= MAX_SPEECH_GAP);
        if !bridging {
            self.last_audio = None;
        }
        bridging
    }
    /// Dragging and seating own the body until released; retain the user's
    /// pause choice and cached clips, but choose the current role on return.
    pub fn suspend(&mut self) {
        self.current = None;
        self.next = None;
        self.last_audio = None;
    }
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
    /// A busy worker or failure to spawn one is not evidence of a broken clip.
    pub fn retry_import(&mut self, now: Instant, error: String) {
        self.current = None;
        self.next = Some(now + Duration::from_millis(200));
        self.note = Some(format!("Motion not started: {error}. Retrying."));
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
        self.select_owner(owner);
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
    #[test]
    fn busy_import_retries_without_blacklisting_a_valid_playlist_motion() {
        let now = Instant::now();
        let owner = Some((uuid::Uuid::new_v4(), 1));
        let path = PathBuf::from("idle-one.bvh");
        let config = CompanionMotionConfig {
            idle_paths: vec![path.clone()],
            ..Default::default()
        };
        let mut animator = CompanionAnimator::default();
        let entries = entries();
        assert_eq!(
            animator.frame(now, &config, owner, false, false, false, None, &entries),
            Some(path.clone())
        );
        animator.retry_import(now, "An animation import is already running".into());
        assert!(
            animator
                .frame(
                    now + Duration::from_millis(199),
                    &config,
                    owner,
                    false,
                    false,
                    false,
                    None,
                    &entries
                )
                .is_none()
        );
        assert_eq!(
            animator.frame(
                now + Duration::from_millis(200),
                &config,
                owner,
                false,
                false,
                false,
                None,
                &entries
            ),
            Some(path)
        );
    }
    #[test]
    fn speech_gaps_belong_to_one_character_and_avatar_and_wait_for_actual_audio() {
        let character = uuid::Uuid::new_v4();
        let first = Some((character, 1));
        let second = Some((uuid::Uuid::new_v4(), 2));
        let mut animator = CompanionAnimator::default();
        let now = Instant::now();
        let speech = |generation, playing, pending| {
            Some(SpeechActivity {
                generation,
                playing,
                pending,
            })
        };
        assert!(!animator.speech_frame(now, first, speech(1, false, true)));
        assert!(animator.speech_frame(now, first, speech(1, true, true)));
        assert!(animator.speech_frame(
            now + Duration::from_millis(700),
            first,
            speech(1, false, true)
        ));
        assert!(!animator.speech_frame(
            now + Duration::from_millis(751),
            first,
            speech(1, false, true)
        ));
        assert!(animator.speech_frame(now, first, speech(1, true, true)));
        assert!(!animator.speech_frame(now, first, speech(2, false, true)));
        assert!(animator.speech_frame(now, first, speech(2, true, true)));
        // Queued speech for a new owner cannot inherit the old owner's latch.
        assert!(!animator.speech_frame(now, second, speech(2, false, true)));
        assert!(animator.speech_frame(now, second, speech(2, true, true)));
        assert!(!animator.speech_frame(now, second, speech(2, false, false)));
        assert!(animator.speech_frame(now, first, speech(2, true, true)));
        assert!(!animator.speech_frame(now, Some((character, 3)), speech(2, false, true)));
        assert!(!animator.speech_frame(now, None, speech(2, true, true)));
        assert!(!animator.speech_frame(now, first, None));
    }
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
            metadata: None,
            path: name.into(),
            label: name.into(),
            group: "test".into(),
            format: "BVH",
        })
        .collect()
    }
    #[test]
    fn desktop_pose_release_reselects_speech_role_without_clearing_manual_pause() {
        let now = Instant::now();
        let config = CompanionMotionConfig::default();
        let entries = entries();
        let owner = Some((uuid::Uuid::new_v4(), 42));
        let mut animator = CompanionAnimator::default();
        let idle = animator
            .frame(now, &config, owner, false, false, false, None, &entries)
            .unwrap();
        animator.suspend();
        let talking = animator
            .frame(
                now,
                &config,
                owner,
                true,
                false,
                false,
                Some(Path::new("idle-sitting.vrma")),
                &entries,
            )
            .unwrap();
        assert!(talking.to_string_lossy().contains("talking"));
        animator.manual_override();
        animator.suspend();
        assert!(
            animator
                .frame(
                    now,
                    &config,
                    owner,
                    false,
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
                    false,
                    false,
                    false,
                    Some(Path::new("grabbed.vrma")),
                    &entries
                )
                .unwrap()
                .to_string_lossy()
                .contains("idle")
        );
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
    fn relocated_bundled_playlists_work_across_windows_and_linux_without_basename_fallback() {
        let mut entries = entries();
        entries[0].path = "/new/app/assets/animations/waifumations/idle.bvh".into();
        let target = entries[0].path.clone();
        for saved in [
            r"D:\old\app\assets\animations\waifumations\idle.bvh",
            "/home/old/app/assets/animations/waifumations/idle.bvh",
        ] {
            assert_eq!(playlist(&entries, &[saved.into()], false), [target.clone()]);
        }
        assert!(playlist(&entries, &["/custom/idle.bvh".into()], false).is_empty());
        let duplicate = entries[0].clone();
        entries.push(duplicate);
        assert!(
            playlist(
                &entries,
                &["/old/assets/animations/waifumations/idle.bvh".into()],
                false
            )
            .is_empty()
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
