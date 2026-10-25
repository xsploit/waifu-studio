//! Populated motion picker source. Motions are read in place from local
//! folders; nothing is copied into the app or its bundle. WebWaifu4's package
//! is UNLICENSED, so its folder is only used when it already exists on this
//! machine (the user's own checkout). `WAIFU_MOTION_LIBRARY` adds more folders
//! (`:`-separated).
use std::path::{Path, PathBuf};

const WEBWAIFU4_ANIMATIONS: &str = "/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4/public/assets/animations";
const MAX_DEPTH: usize = 4;
const MAX_ENTRIES: usize = 4000;

#[derive(Clone, Debug, PartialEq)]
pub struct MotionEntry {
    /// Folder the motion came from, relative to its library root.
    pub metadata: Option<waifu_studio::extension_manifest::MotionContent>,
    pub group: String,
    pub label: String,
    pub path: PathBuf,
    pub format: &'static str,
}

/// Preserve bundled selections across installations and operating systems.
/// External files require an exact path; a basename alone is never sufficient.
pub fn saved_motion<'a>(entries: &'a [MotionEntry], path: &Path) -> Option<&'a MotionEntry> {
    if let Some(entry) = entries.iter().find(|entry| entry.path == path) {
        return Some(entry);
    }
    fn bundled_key(path: &Path) -> Option<String> {
        let normalized = path.to_string_lossy().replace('\\', "/");
        let normalized = normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        let (_, relative) = normalized.rsplit_once("assets/animations/")?;
        if !["waifumations/", "companion/"]
            .iter()
            .any(|prefix| relative.starts_with(prefix))
            || relative.split('/').any(|part| matches!(part, "." | ".."))
        {
            return None;
        }
        Some(relative.to_owned())
    }
    let key = bundled_key(path)?;
    let mut matches = entries
        .iter()
        .filter(|entry| bundled_key(&entry.path).as_ref() == Some(&key));
    let entry = matches.next()?;
    matches.next().is_none().then_some(entry)
}

pub fn roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("WAIFU_MOTION_LIBRARY")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    roots.push(waifu_studio::bundled::assets_path().join("animations/waifumations"));
    roots.push(waifu_studio::bundled::assets_path().join("animations/companion"));
    // The user's existing unlicensed checkout is a development convenience,
    // never an implicit dependency of a relocated package.
    if waifu_studio::bundled::development_assets_in_use() {
        roots.push(PathBuf::from(WEBWAIFU4_ANIMATIONS));
    }
    roots.retain(|root| root.is_dir());
    roots.dedup();
    roots
}

fn format_of(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "bvh" => "BVH",
        "vrma" => "VRMA",
        "fbx" => "FBX",
        _ => return None,
    })
}

/// Readable name: `CC0animationrightwave1` → `rightwave1`, `action_greeting` → `action greeting`.
fn label_of(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let stem = stem.strip_prefix("CC0animation").unwrap_or(&*stem);
    stem.replace(['_', '-'], " ")
}

/// Regular motion files under `roots`, sorted by group then label. Symlinks are
/// skipped so a library cannot point outside its folder.
pub fn scan(roots: &[PathBuf]) -> Vec<MotionEntry> {
    let mut entries = Vec::new();
    for root in roots {
        let mut pending = vec![(root.clone(), 0)];
        while let Some((directory, depth)) = pending.pop() {
            let Ok(read) = std::fs::read_dir(&directory) else {
                continue;
            };
            for item in read.flatten() {
                let Ok(kind) = item.file_type() else { continue };
                let path = item.path();
                if kind.is_dir() && depth < MAX_DEPTH {
                    pending.push((path, depth + 1));
                } else if kind.is_file()
                    && let Some(format) = format_of(&path)
                {
                    if entries.len() >= MAX_ENTRIES {
                        break;
                    }
                    let group = path
                        .parent()
                        .and_then(|parent| parent.strip_prefix(root).ok())
                        .map(|relative| relative.to_string_lossy().into_owned())
                        .filter(|relative| !relative.is_empty())
                        .unwrap_or_else(|| {
                            root.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        });
                    entries.push(MotionEntry {
                        metadata: None,
                        group,
                        label: label_of(&path),
                        path,
                        format,
                    });
                }
            }
        }
    }
    entries.sort_by(|a, b| {
        (&a.group, a.label.to_lowercase()).cmp(&(&b.group, b.label.to_lowercase()))
    });
    entries
}

/// Coarse purpose of a motion, inferred only from clear filename words.
/// `Unknown` means the name gave no reliable signal; nothing is guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MotionCategory {
    Idle,
    Greeting,
    Talking,
    Thinking,
    Celebration,
    Dance,
    Locomotion,
    Unknown,
}

impl MotionCategory {
    pub const ALL: [MotionCategory; 8] = [
        Self::Idle,
        Self::Greeting,
        Self::Talking,
        Self::Thinking,
        Self::Celebration,
        Self::Dance,
        Self::Locomotion,
        Self::Unknown,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Greeting => "greeting",
            Self::Talking => "talking",
            Self::Thinking => "thinking",
            Self::Celebration => "celebration",
            Self::Dance => "dance",
            Self::Locomotion => "locomotion",
            Self::Unknown => "unknown",
        }
    }
}

/// Whole-word signals, checked in this order so the first match wins
/// (`sit_idle` is idle, `sitwave` is a greeting, `action_walk` is locomotion).
const CATEGORY_WORDS: &[(MotionCategory, &[&str])] = &[
    (MotionCategory::Dance, &["dance", "dancing"]),
    (
        MotionCategory::Locomotion,
        &[
            "walk", "walking", "run", "running", "jog", "jump", "crawl", "crawling",
        ],
    ),
    (
        MotionCategory::Greeting,
        &[
            "wave",
            "waving",
            "rightwave",
            "leftwave",
            "sitwave",
            "greet",
            "greeting",
            "hello",
            "bow",
        ],
    ),
    (
        MotionCategory::Celebration,
        &[
            "celebrate",
            "celebration",
            "cheer",
            "joy",
            "happy",
            "excitement",
            "excited",
            "victory",
            "clap",
        ],
    ),
    (
        MotionCategory::Thinking,
        &[
            "think",
            "thinking",
            "ponder",
            "curiosity",
            "confusion",
            "realization",
        ],
    ),
    // Sachi CC0 clips use Japanese names; WebWaifu4's catalog labels zatu
    // "casual talk", ruru "talk", point "explain", hima "waiting", stand "stand".
    (
        MotionCategory::Talking,
        &[
            "talk", "talking", "speak", "speaking", "explain", "zatu", "ruru", "point",
        ],
    ),
    (
        MotionCategory::Idle,
        &["idle", "neutral", "breathing", "hima", "stand"],
    ),
];

/// Category from a file name alone; digits separate words and are ignored.
pub fn category_of(path: &Path) -> MotionCategory {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let stem = stem.strip_prefix("cc0animation").unwrap_or(&stem);
    let words: Vec<&str> = stem
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .collect();
    CATEGORY_WORDS
        .iter()
        .find(|(_, signals)| words.iter().any(|word| signals.contains(word)))
        .map_or(MotionCategory::Unknown, |(category, _)| *category)
}

impl MotionEntry {
    pub fn category(&self) -> MotionCategory {
        use waifu_studio::extension_manifest::MotionRole;
        self.metadata.as_ref().map_or_else(
            || category_of(&self.path),
            |m| match m.role {
                MotionRole::Idle => MotionCategory::Idle,
                MotionRole::Talking => MotionCategory::Talking,
                MotionRole::Gesture => MotionCategory::Unknown,
            },
        )
    }

    /// Automatic playback never starts locomotion.
    pub fn auto_playable(&self) -> bool {
        category_of(&self.path) != MotionCategory::Locomotion
    }
}

/// Most options ever offered to a chooser in one catalog.
pub const MAX_OFFERED: usize = 256;
/// Answer that declines every offered motion.
pub const NONE_CHOICE: &str = "none";
const MAX_LABEL_CHARS: usize = 48;
const MAX_ANSWER_BYTES: usize = 64;

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes.iter().chain(&[0xff]) {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// FNV-1a over every scanned path and format; changes whenever the scan does.
fn revision_of(entries: &[MotionEntry]) -> u64 {
    entries.iter().fold(0xcbf2_9ce4_8422_2325, |hash, entry| {
        let hash = fnv(hash, entry.path.as_os_str().as_encoded_bytes());
        let hash = fnv(hash, entry.format.as_bytes());
        fnv(
            hash,
            &serde_json::to_vec(&entry.metadata).unwrap_or_default(),
        )
    })
}

/// One option as a chooser sees it: an opaque ID, a short label and a category.
/// Paths never leave the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedMotion {
    pub id: String,
    pub label: String,
    pub category: MotionCategory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceError {
    /// The scan changed since the catalog was offered, or the ID is from another catalog.
    Stale,
    /// The answer is neither `none` nor an offered ID.
    NotOffered,
}

/// Bounded set of scanned motions offered for one decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MotionCatalog {
    revision: u64,
    offered: Vec<(OfferedMotion, PathBuf)>,
}

impl MotionCatalog {
    /// Offers at most `limit` (capped at [`MAX_OFFERED`]) entries whose category is
    /// in `allowed`, taking them round-robin across `allowed` in its order so one
    /// category cannot crowd out the rest. Within a category, scan order is kept.
    pub fn offer(entries: &[MotionEntry], allowed: &[MotionCategory], limit: usize) -> Self {
        let revision = revision_of(entries);
        let limit = limit.min(MAX_OFFERED);
        let mut allowed_once = Vec::new();
        for category in allowed {
            if !allowed_once.contains(category) {
                allowed_once.push(*category);
            }
        }
        let mut queues: Vec<_> = allowed_once
            .iter()
            .map(|category| {
                entries
                    .iter()
                    .filter(move |entry| entry.category() == *category)
                    .peekable()
            })
            .collect();
        let mut picked = Vec::new();
        while picked.len() < limit && queues.iter_mut().any(|queue| queue.peek().is_some()) {
            for queue in &mut queues {
                if picked.len() >= limit {
                    break;
                }
                if let Some(entry) = queue.next() {
                    picked.push(entry);
                }
            }
        }
        // IDs are salted with the offered set too, so an ID from a different
        // offer over the same scan never aliases a slot in this one.
        let key = picked.iter().fold(revision, |hash, entry| {
            fnv(hash, entry.path.as_os_str().as_encoded_bytes())
        });
        let offered = picked
            .into_iter()
            .enumerate()
            .map(|(slot, entry)| {
                let motion = OfferedMotion {
                    id: format!("m{key:016x}-{slot}"),
                    label: entry.label.chars().take(MAX_LABEL_CHARS).collect(),
                    category: entry.category(),
                };
                (motion, entry.path.clone())
            })
            .collect();
        Self { revision, offered }
    }

    pub fn options(&self) -> impl Iterator<Item = &OfferedMotion> {
        self.offered.iter().map(|(motion, _)| motion)
    }

    pub fn is_empty(&self) -> bool {
        self.offered.is_empty()
    }

    /// Whether `entries` is still the scan this catalog was offered from.
    pub fn is_current(&self, entries: &[MotionEntry]) -> bool {
        self.revision == revision_of(entries)
    }

    /// Resolves a chooser's answer against the *current* scan. `Ok(None)` is an
    /// explicit `none`; `Ok(Some)` is always an entry of `current`, never a
    /// caller-supplied path.
    pub fn resolve<'a>(
        &self,
        answer: &str,
        current: &'a [MotionEntry],
    ) -> Result<Option<&'a MotionEntry>, ChoiceError> {
        let answer = answer.trim();
        if answer.len() > MAX_ANSWER_BYTES {
            return Err(ChoiceError::NotOffered);
        }
        if answer.eq_ignore_ascii_case(NONE_CHOICE) {
            return Ok(None);
        }
        if !self.is_current(current) {
            return Err(ChoiceError::Stale);
        }
        let Some((_, path)) = self.offered.iter().find(|(motion, _)| motion.id == answer) else {
            return if answer.starts_with('m') && answer.contains('-') {
                Err(ChoiceError::Stale)
            } else {
                Err(ChoiceError::NotOffered)
            };
        };
        current
            .iter()
            .find(|entry| &entry.path == path)
            .map(Some)
            .ok_or(ChoiceError::Stale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_kimodo_library_parses_and_supplies_automatic_playlists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/animations/waifumations");
        let entries = scan(&[root]);
        assert_eq!(
            entries.len(),
            167,
            "Original Kimodo library must ship intact"
        );
        for entry in &entries {
            let text = std::fs::read_to_string(&entry.path).unwrap();
            let clip = waifu_studio::animation::parse_bvh(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", entry.path.display()));
            assert!(clip.frame_count > 1 && clip.frame_time > 0.0);
        }
        assert!(
            entries
                .iter()
                .filter(|e| e.category() == MotionCategory::Idle)
                .count()
                >= 16
        );
        assert!(
            entries
                .iter()
                .filter(|e| e.category() == MotionCategory::Talking)
                .count()
                >= 20
        );
        assert!(
            entries
                .iter()
                .filter(|e| e.category() == MotionCategory::Locomotion)
                .all(|e| !e.auto_playable())
        );
    }
    #[test]
    fn scans_supported_motions_with_groups_and_readable_labels() {
        let root = std::env::temp_dir().join(format!("motion-lib-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("sachi-vrma")).unwrap();
        std::fs::create_dir_all(root.join("silly-bvh")).unwrap();
        for file in [
            "Wave.fbx",
            "notes.txt",
            "sachi-vrma/CC0animationrightwave1.vrma",
            "silly-bvh/action_greeting.bvh",
            "silly-bvh/Joy.BVH",
        ] {
            std::fs::write(root.join(file), b"x").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Wave.fbx"), root.join("linked.fbx")).unwrap();
        let entries = scan(std::slice::from_ref(&root));
        let names: Vec<_> = entries
            .iter()
            .map(|e| (e.group.as_str(), e.label.as_str(), e.format))
            .collect();
        let top = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            names,
            vec![
                (top.as_str(), "Wave", "FBX"),
                ("sachi-vrma", "rightwave1", "VRMA"),
                ("silly-bvh", "action greeting", "BVH"),
                ("silly-bvh", "Joy", "BVH"),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn entry(group: &str, file: &str) -> MotionEntry {
        let path = PathBuf::from(format!("/lib/{group}/{file}"));
        MotionEntry {
            metadata: None,
            group: group.into(),
            label: label_of(&path),
            format: format_of(&path).unwrap(),
            path,
        }
    }

    #[test]
    fn categories_come_only_from_clear_filename_words() {
        let cases = [
            ("Idle2.fbx", MotionCategory::Idle),
            ("sit_idle3.bvh", MotionCategory::Idle),
            ("neutral4.bvh", MotionCategory::Idle),
            ("Wave.fbx", MotionCategory::Greeting),
            ("CC0animationrightwave1.vrma", MotionCategory::Greeting),
            ("CC0animationsitwave01.vrma", MotionCategory::Greeting),
            ("action_greeting1.bvh", MotionCategory::Greeting),
            ("Thinking.fbx", MotionCategory::Thinking),
            ("confusion2.bvh", MotionCategory::Thinking),
            ("CC0animationhappy01.vrma", MotionCategory::Celebration),
            ("joy3.bvh", MotionCategory::Celebration),
            ("dance_2.bvh", MotionCategory::Dance),
            ("action_walk.bvh", MotionCategory::Locomotion),
            ("talking.fbx", MotionCategory::Talking),
            ("CC0animationzatu01.vrma", MotionCategory::Talking),
            ("CC0animationruru02.vrma", MotionCategory::Talking),
            ("CC0animationpoint1.vrma", MotionCategory::Talking),
            ("CC0animationhima01.vrma", MotionCategory::Idle),
            ("CC0animationstand01.vrma", MotionCategory::Idle),
            ("action_standup.bvh", MotionCategory::Unknown),
            ("CC0animationunwave.vrma", MotionCategory::Unknown),
            ("CC0animationunknown3.vrma", MotionCategory::Unknown),
            ("CC0animationother1.vrma", MotionCategory::Unknown),
            ("CC0animationrotate6.vrma", MotionCategory::Unknown),
            ("anger2.bvh", MotionCategory::Unknown),
            ("idlewalkers.bvh", MotionCategory::Unknown),
        ];
        for (file, expected) in cases {
            assert_eq!(category_of(Path::new(file)), expected, "{file}");
        }
    }

    #[test]
    fn restored_library_offers_every_stationary_motion_including_uncategorized_reactions() {
        let entries = scan(&[
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/animations/waifumations")
        ]);
        assert_eq!(entries.len(), 167);
        let categories = MotionCategory::ALL
            .into_iter()
            .filter(|c| *c != MotionCategory::Locomotion)
            .collect::<Vec<_>>();
        let catalog = MotionCatalog::offer(&entries, &categories, MAX_OFFERED);
        assert_eq!(
            catalog.options().count(),
            entries.iter().filter(|entry| entry.auto_playable()).count()
        );
        assert!(catalog.options().count() > 16);
        assert!(
            catalog
                .options()
                .any(|entry| entry.category == MotionCategory::Unknown)
        );
    }

    #[test]
    fn catalog_offers_a_bounded_round_robin_without_paths() {
        let entries: Vec<_> = [
            "Idle.fbx",
            "Idle2.fbx",
            "Idle3.fbx",
            "Wave.fbx",
            "dance_1.bvh",
        ]
        .iter()
        .map(|file| entry("lib", file))
        .chain((0..40).map(|n| entry("bulk", &format!("idle{n}.bvh"))))
        .collect();
        let allowed = [
            MotionCategory::Idle,
            MotionCategory::Greeting,
            MotionCategory::Idle,
            MotionCategory::Talking,
        ];
        let catalog = MotionCatalog::offer(&entries, &allowed, 4);
        let shown: Vec<_> = catalog
            .options()
            .map(|o| (o.label.as_str(), o.category))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("Idle", MotionCategory::Idle),
                ("Wave", MotionCategory::Greeting),
                ("Idle2", MotionCategory::Idle),
                ("Idle3", MotionCategory::Idle),
            ]
        );
        assert!(catalog.options().all(|o| !o.id.contains('/')));
        assert_eq!(catalog, MotionCatalog::offer(&entries, &allowed, 4));
        assert_eq!(
            MotionCatalog::offer(&entries, &[MotionCategory::Idle], 1000)
                .options()
                .count(),
            43
        );
        assert!(MotionCatalog::offer(&entries, &[], 8).is_empty());
    }

    #[test]
    fn choices_resolve_only_offered_entries_of_the_current_scan() {
        let entries = vec![entry("lib", "Idle.fbx"), entry("lib", "Wave.fbx")];
        let catalog = MotionCatalog::offer(&entries, &[MotionCategory::Greeting], 4);
        let id = catalog.options().next().unwrap().id.clone();
        assert_eq!(
            catalog.resolve(&format!(" {id} "), &entries),
            Ok(Some(&entries[1]))
        );
        assert_eq!(catalog.resolve("NONE", &entries), Ok(None));
        for bogus in ["/lib/lib/Idle.fbx", "Wave", "", &"m".repeat(200)] {
            assert_eq!(
                catalog.resolve(bogus, &entries),
                Err(ChoiceError::NotOffered)
            );
        }
        let idle_only = MotionCatalog::offer(&entries, &[MotionCategory::Idle], 4);
        let idle_id = idle_only.options().next().unwrap().id.clone();
        assert_eq!(catalog.resolve(&idle_id, &entries), Err(ChoiceError::Stale));

        let mut rescanned = entries.clone();
        rescanned.push(entry("lib", "joy.bvh"));
        assert!(!catalog.is_current(&rescanned));
        assert_eq!(catalog.resolve(&id, &rescanned), Err(ChoiceError::Stale));
        assert_eq!(catalog.resolve("none", &rescanned), Ok(None));
    }

    #[test]
    fn missing_roots_are_ignored() {
        assert!(scan(&[PathBuf::from("/nonexistent/waifu-motions")]).is_empty());
    }
}
