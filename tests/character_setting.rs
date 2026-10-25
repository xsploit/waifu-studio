//! Character references persist independently of loaded avatars or library membership.
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use uuid::Uuid;
use waifu_studio::{
    extension_settings::{SettingValue, Settings},
    extension_store::{ExtensionStore, InstalledPack},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("character-setting-{}", Uuid::new_v4())))
    }
    fn store(&self) -> ExtensionStore {
        ExtensionStore::open(self.0.join("store")).unwrap()
    }
    fn package(&self, version: &str, settings: Value) -> PathBuf {
        let source = self.0.join("source");
        fs::create_dir_all(&source).unwrap();
        let program = source.join("worker.py");
        // Never launched: persistence tests need no network, providers, or library.
        fs::write(&program, b"#!/usr/bin/env python3\nraise SystemExit(0)\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let manifest = json!({
            "api_version":1, "id":"org.example.character-setting", "version":version,
            "name":"Character setting fixture", "description":"Offline persistence fixture",
            "provenance":{"author":"Fixture","license":"MIT"},
            "process":{"program":"worker.py"}, "settings":settings,
        });
        fs::write(
            source.join("extension.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        source
    }
    fn index(&self) -> Vec<u8> {
        fs::read(self.0.join("store/installed.json")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn definition(id: &str, default: &str) -> Value {
    json!({"id":id,"label":"Character","type":"character","default":default})
}
fn values(value: SettingValue) -> Settings {
    [("character".into(), value)].into()
}
fn pack(store: &ExtensionStore, id: Uuid) -> InstalledPack {
    store
        .packs()
        .iter()
        .find(|pack| pack.installation_id == id)
        .unwrap()
        .clone()
}

#[test]
fn character_selection_survives_reopen_enable_cycle_and_package_schema_reconciliation() {
    let f = Fixture::new();
    let selected = "12345678-abcd-4def-8abc-123456789abc";
    let new_default = "87654321-abcd-4def-8abc-123456789abc";
    let source = f.package(
        "1.0.0",
        json!([
            definition("character", ""),
            definition("changed", ""),
            definition("removed", ""),
        ]),
    );
    let mut store = f.store();
    let id = store.install_integration(&source).unwrap();
    assert_eq!(
        pack(&store, id).settings["character"],
        SettingValue::Text(String::new())
    );
    let settings: Settings = ["character", "changed", "removed"]
        .into_iter()
        .map(|key| (key.into(), SettingValue::Text(selected.into())))
        .collect();
    store.set_settings(id, settings.clone()).unwrap();
    store.set_enabled(id, true).unwrap();
    let saved = pack(&store, id);
    assert!(
        saved.grants.is_empty(),
        "selection must not grant capabilities"
    );
    drop(store);
    let mut store = f.store();
    assert_eq!(pack(&store, id), saved);
    // Re-enabling/reopening must not require that the selected character exists.
    store.set_enabled(id, false).unwrap();
    store.set_enabled(id, true).unwrap();
    assert_eq!(pack(&store, id).settings, settings);
    let prior_revision = pack(&store, id).revision;
    let source = f.package(
        "2.0.0",
        json!([
            definition("character", new_default),
            {"id":"changed","label":"Changed type","type":"integer","default":3,"min":0,"max":5},
            definition("renamed", new_default),
        ]),
    );
    assert_eq!(store.install_integration(&source).unwrap(), id);
    let updated = pack(&store, id);
    assert_ne!(updated.revision, prior_revision);
    assert_eq!(
        updated.settings,
        Settings::from([
            ("character".into(), SettingValue::Text(selected.into())),
            ("changed".into(), SettingValue::Integer(3)),
            ("renamed".into(), SettingValue::Text(new_default.into())),
        ])
    );
    assert!(updated.enabled && updated.grants.is_empty());
    assert!(!updated.settings.contains_key("removed"));
    drop(store);
    let store = f.store();
    assert_eq!(pack(&store, id), updated);
    assert!(!f.0.join("library.json").exists());
}

#[test]
fn invalid_character_values_cannot_change_revision_settings_or_saved_index_and_none_persists() {
    let f = Fixture::new();
    let source = f.package("1.0.0", json!([definition("character", "")]));
    let mut store = f.store();
    let id = store.install_integration(&source).unwrap();
    let selected = "12345678-abcd-4def-8abc-123456789abc";
    store
        .set_settings(id, values(SettingValue::Text(selected.into())))
        .unwrap();
    let previous = pack(&store, id);
    let original_index = f.index();
    for invalid in [
        SettingValue::Text(Uuid::nil().to_string()),
        SettingValue::Text(selected.to_uppercase()),
        SettingValue::Text(selected.replace('-', "")),
        SettingValue::Text(format!("{{{selected}}}")),
        SettingValue::Text(format!("urn:uuid:{selected}")),
        SettingValue::Text(format!(" {selected}")),
        SettingValue::Text(format!("{selected}\n")),
        SettingValue::Text("not-a-uuid".into()),
        SettingValue::Text(" ".into()),
        SettingValue::Bool(false),
        SettingValue::Integer(1),
    ] {
        assert!(store.set_settings(id, values(invalid)).is_err());
        assert_eq!(pack(&store, id), previous);
        assert_eq!(f.index(), original_index);
    }
    store
        .set_settings(id, values(SettingValue::Text(String::new())))
        .unwrap();
    let none = pack(&store, id);
    assert_eq!(
        none.settings["character"],
        SettingValue::Text(String::new())
    );
    assert!(none.grants.is_empty());
    drop(store);
    let store = f.store();
    assert_eq!(pack(&store, id), none);
}
