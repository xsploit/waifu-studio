//! Editable starter characters. A persisted import receipt preserves edits and deletions.
use crate::data::{Character, Library};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[derive(Deserialize)]
struct Bundle {
    version: u32,
    characters: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: Uuid,
    name: String,
    model: PathBuf,
    persona: String,
    #[serde(default)]
    greeting: String,
    format: String,
    #[serde(default)]
    voice: Option<crate::voice::VoiceConfig>,
}

pub fn manifest_path() -> PathBuf {
    if let Some(path) = std::env::var_os("WAIFU_STUDIO_BUNDLE") {
        return path.into();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let path = parent.join("assets/characters/bundle.json");
        if path.is_file() {
            return path;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/characters/bundle.json")
}

/// Returns the number of persistent changes. Validate everything before mutation.
pub fn seed(library: &mut Library, manifest: &Path) -> Result<usize, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(manifest)
        .map_err(|e| e.to_string())?
        .take(4_000_001)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4_000_000 {
        return Err("Character bundle manifest is too large".into());
    }
    let bundle: Bundle =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid character bundle: {e}"))?;
    if bundle.version != 1 || bundle.characters.len() > 1024 {
        return Err("Unsupported character bundle version or size".into());
    }
    let root = manifest
        .parent()
        .ok_or("Character bundle has no directory")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut ids = std::collections::HashSet::new();
    let mut resolved = Vec::new();
    for entry in bundle.characters {
        if !ids.insert(entry.id)
            || entry.model.as_os_str().is_empty()
            || entry
                .model
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || entry.name.len() > 1024
            || entry.persona.len() > 256_000
            || entry.greeting.len() > 32_000
        {
            return Err("Invalid or duplicate bundled character".into());
        }
        let path = root
            .join(&entry.model)
            .canonicalize()
            .map_err(|e| format!("Missing bundled model: {e}"))?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err("Bundled model must be a file inside the bundle".into());
        }
        resolved.push((entry, path));
    }
    let mut changed = 0;
    for (entry, path) in resolved {
        if let Some(existing) = library.characters.iter_mut().find(|c| c.id == entry.id) {
            // Repair a relocated bundle only; retain deliberate model replacements.
            if !existing.model.exists() && existing.model.ends_with(&entry.model) {
                existing.model = path;
                changed += 1;
            }
            if library.bundled_seen.insert(entry.id) {
                changed += 1;
            }
            continue;
        }
        if !library.bundled_seen.insert(entry.id) {
            continue;
        }
        library.characters.push(Character {
            id: entry.id,
            name: entry.name,
            model: path,
            persona: entry.persona,
            greeting: entry.greeting,
            format: entry.format,
            delegation: Default::default(),
            scene: Some(Default::default()),
            provider: None,
            voice: entry.voice,
        });
        changed += 1;
    }
    if library.selected.is_none()
        && let Some(first) = library.characters.first()
    {
        library.select_character(first.id);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, Uuid) {
        let root = std::env::temp_dir().join(format!("waifu-bundle-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("models")).unwrap();
        std::fs::write(root.join("models/avatar.vrm"), b"fixture").unwrap();
        let id = Uuid::new_v4();
        std::fs::write(
            root.join("bundle.json"),
            serde_json::to_vec(&serde_json::json!({
                "version":1,"characters":[{"id":id,"name":"Original","model":"models/avatar.vrm",
                "persona":"Exact original\ntext.","greeting":"Hello","format":"VRM 0.0"}]
            }))
            .unwrap(),
        )
        .unwrap();
        (root, id)
    }
    #[test]
    fn packaged_models_have_valid_vrm_metadata_and_exact_profile_defaults() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/characters/bundle.json");
        if !manifest.is_file() {
            return;
        }
        let mut library = Library::default();
        seed(&mut library, &manifest).unwrap();
        assert_eq!(library.characters.len(), 22);
        assert_eq!(
            library
                .characters
                .iter()
                .filter(|c| !c.persona.is_empty())
                .count(),
            3
        );
        assert_eq!(library.selected_character().unwrap().name, "Hikari-chan");
        for character in &library.characters {
            crate::inspect_file(&character.model)
                .unwrap_or_else(|error| panic!("{}: {error}", character.name));
        }
    }
    #[test]
    fn defaults_are_editable_and_deletions_survive_restarts() {
        let (root, id) = fixture();
        let mut library = Library::default();
        assert_eq!(seed(&mut library, &root.join("bundle.json")).unwrap(), 1);
        assert_eq!(library.characters[0].persona, "Exact original\ntext.");
        library.characters[0].persona = "My edit".into();
        let mut loaded: Library =
            serde_json::from_slice(&serde_json::to_vec(&library).unwrap()).unwrap();
        assert_eq!(seed(&mut loaded, &root.join("bundle.json")).unwrap(), 0);
        assert_eq!(loaded.characters[0].persona, "My edit");
        loaded.characters.clear();
        loaded.selected = None;
        assert_eq!(seed(&mut loaded, &root.join("bundle.json")).unwrap(), 0);
        assert!(loaded.characters.is_empty() && loaded.bundled_seen.contains(&id));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn relocation_repairs_paths_without_replacing_personality() {
        let (root, _) = fixture();
        let mut library = Library::default();
        seed(&mut library, &root.join("bundle.json")).unwrap();
        library.characters[0].persona = "Kept".into();
        let moved = root.with_extension("moved");
        std::fs::rename(&root, &moved).unwrap();
        assert_eq!(seed(&mut library, &moved.join("bundle.json")).unwrap(), 1);
        assert!(library.characters[0].model.is_file());
        assert_eq!(library.characters[0].persona, "Kept");
        std::fs::remove_dir_all(moved).unwrap();
    }
    #[test]
    fn traversal_rejects_whole_bundle_before_import() {
        let (root, _) = fixture();
        let manifest = root.join("bundle.json");
        let text = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("models/avatar.vrm", "../avatar.vrm");
        std::fs::write(&manifest, text).unwrap();
        let mut library = Library::default();
        assert!(seed(&mut library, &manifest).is_err());
        assert!(library.characters.is_empty() && library.bundled_seen.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
