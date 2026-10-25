//! Packaged characters and extension examples, with development-checkout fallbacks.
//! A persisted character import receipt preserves edits and deletions.
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
    assets_path().join("characters/bundle.json")
}

/// Packaged assets live beside the executable. Only debug builds implicitly use
/// the source checkout; source release launches opt in with an explicit root.
pub fn assets_path() -> PathBuf {
    let configured = std::env::var_os("WAIFU_STUDIO_ASSET_ROOT");
    assets_at(
        std::env::current_exe().ok().as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        cfg!(debug_assertions),
        configured.as_deref().map(Path::new),
    )
}

/// Keeps the user's old local motion checkout a development convenience only.
/// Explicit external libraries remain controlled by WAIFU_MOTION_LIBRARY.
pub fn development_assets_in_use() -> bool {
    assets_path() == Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

pub fn extension_examples_path() -> PathBuf {
    assets_path().join("extensions")
}

fn assets_at(
    executable: Option<&Path>,
    development_root: &Path,
    debug_fallback: bool,
    configured: Option<&Path>,
) -> PathBuf {
    if let Some(root) = configured {
        return root.to_path_buf();
    }
    let adjacent = executable.and_then(Path::parent).map(|p| p.join("assets"));
    if debug_fallback && adjacent.as_ref().is_none_or(|p| !p.is_dir()) {
        return development_root.join("assets");
    }
    // A release with omitted assets must not silently borrow from its build host.
    adjacent.expect("Cannot locate application executable; set WAIFU_STUDIO_ASSET_ROOT")
}

#[cfg(test)]
fn extension_examples_at(executable: Option<&Path>, development_root: &Path) -> PathBuf {
    assets_at(executable, development_root, true, None).join("extensions")
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
            // Only a recorded bundled reference can be relocated. A missing
            // custom model (including legacy profiles without receipts) stays
            // selected so seeding cannot silently replace a deliberate choice.
            if library.bundled_models.get(&entry.id) == Some(&existing.model)
                && existing.model != path
            {
                existing.model = path.clone();
                changed += 1;
            }
            if library
                .bundled_models
                .insert(entry.id, path.clone())
                .as_ref()
                != Some(&path)
            {
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
        library.bundled_models.insert(entry.id, path.clone());
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

    #[test]
    fn release_with_missing_assets_cannot_borrow_from_an_existing_build_checkout() {
        let root = std::env::temp_dir().join(format!("waifu-release-assets-{}", Uuid::new_v4()));
        let development = root.join("checkout");
        std::fs::create_dir_all(development.join("assets/backgrounds")).unwrap();
        let executable = root.join("relocated app/waifu-studio");
        assert_eq!(
            assets_at(Some(&executable), &development, false, None),
            root.join("relocated app/assets")
        );
        assert_eq!(
            assets_at(Some(&executable), &development, true, None),
            development.join("assets")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaged_categories_use_the_same_root_even_when_optional_files_are_omitted() {
        let root = std::env::temp_dir().join(format!("waifu-category-assets-{}", Uuid::new_v4()));
        let packaged = root.join("moved app/assets");
        let development = root.join("checkout");
        std::fs::create_dir_all(&packaged).unwrap();
        for file in [
            "characters/bundle.json",
            "backgrounds/cozy-room.png",
            "rooms/wave-art.png",
            "animations/companion/idle.bvh",
            "extensions/example/extension.json",
        ] {
            let original = development.join("assets").join(file);
            std::fs::create_dir_all(original.parent().unwrap()).unwrap();
            std::fs::write(original, b"must not be borrowed").unwrap();
            assert!(
                !assets_at(
                    Some(&root.join("moved app/waifu-studio")),
                    &development,
                    false,
                    None
                )
                .join(file)
                .exists()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_asset_root_works_for_release_source_launches_and_external_bundles() {
        let executable = Path::new("/package/waifu-studio");
        let development = Path::new("/checkout");
        for configured in [Path::new("/checkout/assets"), Path::new("/custom assets")] {
            assert_eq!(
                assets_at(Some(executable), development, false, Some(configured)),
                configured
            );
        }
    }

    #[test]
    fn relocated_examples_install_without_the_build_checkout() {
        let root = std::env::temp_dir().join(format!("waifu-relocated-{}", Uuid::new_v4()));
        let original = root.join("original");
        let shipped = original.join("assets/extensions/lifecycle-observer");
        std::fs::create_dir_all(&shipped).unwrap();
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/lifecycle-observer");
        for file in ["extension.json", "observer.py", "LICENSE"] {
            std::fs::copy(source.join(file), shipped.join(file)).unwrap();
        }
        let relocated = root.join("moved application");
        std::fs::rename(original, &relocated).unwrap();
        let missing_checkout = root.join("removed-build-checkout");
        let examples =
            extension_examples_at(Some(&relocated.join("waifu-studio")), &missing_checkout);
        let mut store = crate::extension_store::ExtensionStore::open(root.join("profile")).unwrap();
        let id = store
            .install_integration(&examples.join("lifecycle-observer"))
            .unwrap();
        let pack = store
            .packs()
            .iter()
            .find(|p| p.installation_id == id)
            .unwrap();
        assert_eq!(pack.package_id, "studio.example.lifecycle-observer");
        assert!(!pack.enabled && pack.grants.is_empty());
        store.validate(pack).unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaged_assets_do_not_silently_borrow_omitted_examples_from_the_checkout() {
        let root = std::env::temp_dir().join(format!("waifu-assets-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("package/assets/characters")).unwrap();
        std::fs::create_dir_all(root.join("checkout/assets/extensions")).unwrap();
        assert_eq!(
            extension_examples_at(
                Some(&root.join("package/waifu-studio")),
                &root.join("checkout")
            ),
            root.join("package/assets/extensions")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn development_builds_without_adjacent_assets_keep_source_examples() {
        let root = std::env::temp_dir().join(format!("waifu-development-{}", Uuid::new_v4()));
        let development = root.join("checkout");
        let expected = development.join("assets/extensions");
        assert_eq!(
            extension_examples_at(Some(&root.join("target/waifu-studio")), &development),
            expected
        );
        assert_eq!(extension_examples_at(None, &development), expected);
    }
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
        assert_eq!(library.characters.len(), 25);
        assert_eq!(
            library
                .characters
                .iter()
                .map(|c| c.persona.as_str())
                .filter(|p| !p.is_empty())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(library.selected_character().unwrap().name, "Hikari-chan");
        assert!(
            library
                .selected_character()
                .unwrap()
                .model
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("hikari-wardrobe.vrm")
        );
        for variant in ["ARKit", "Cel", "Moto", "Refined", "Classic", "Original"] {
            assert!(
                library
                    .characters
                    .iter()
                    .any(|c| c.name == format!("Hikari · {variant}"))
            );
        }

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
        assert_eq!(seed(&mut library, &moved.join("bundle.json")).unwrap(), 2);
        assert!(library.characters[0].model.is_file());
        assert_eq!(library.characters[0].persona, "Kept");
        std::fs::remove_dir_all(moved).unwrap();
    }
    #[test]
    fn missing_custom_model_with_bundle_suffix_is_never_replaced() {
        for legacy in [false, true] {
            let (root, id) = fixture();
            let mut library = Library::default();
            seed(&mut library, &root.join("bundle.json")).unwrap();
            let custom = root.join("custom/models/avatar.vrm");
            library.characters[0].model = custom.clone();
            if legacy {
                library.bundled_models.clear();
            }
            let mut persisted: Library =
                serde_json::from_slice(&serde_json::to_vec(&library).unwrap()).unwrap();
            seed(&mut persisted, &root.join("bundle.json")).unwrap();
            assert_eq!(persisted.characters[0].model, custom);
            assert!(persisted.bundled_models.contains_key(&id));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn recorded_bundle_relocation_works_even_when_old_install_still_exists() {
        let (root, id) = fixture();
        let mut library = Library::default();
        seed(&mut library, &root.join("bundle.json")).unwrap();
        let moved = root.join("second installation");
        std::fs::create_dir_all(moved.join("models")).unwrap();
        std::fs::copy(
            root.join("models/avatar.vrm"),
            moved.join("models/avatar.vrm"),
        )
        .unwrap();
        std::fs::copy(root.join("bundle.json"), moved.join("bundle.json")).unwrap();
        let mut persisted: Library =
            serde_json::from_slice(&serde_json::to_vec(&library).unwrap()).unwrap();
        seed(&mut persisted, &moved.join("bundle.json")).unwrap();
        let expected = moved.join("models/avatar.vrm").canonicalize().unwrap();
        assert_eq!(persisted.characters[0].model, expected);
        assert_eq!(persisted.bundled_models[&id], expected);
        assert_eq!(seed(&mut persisted, &moved.join("bundle.json")).unwrap(), 0);
        std::fs::remove_dir_all(root).unwrap();
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
