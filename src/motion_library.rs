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
    pub group: String,
    pub label: String,
    pub path: PathBuf,
    pub format: &'static str,
}

pub fn roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("WAIFU_MOTION_LIBRARY")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    roots.push(PathBuf::from(WEBWAIFU4_ANIMATIONS));
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn missing_roots_are_ignored() {
        assert!(scan(&[PathBuf::from("/nonexistent/waifu-motions")]).is_empty());
    }
}
