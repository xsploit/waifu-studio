//! Expression names are configuration references; they do not grant face ownership.
use std::{fs, path::PathBuf};
use uuid::Uuid;
use waifu_studio::{extension_settings::SettingValue, extension_store::ExtensionStore};

#[test]
fn twitch_update_requests_expression_permission_without_granting_it() {
    use waifu_studio::extension_protocol::Capability;
    let root = std::env::temp_dir().join(format!("waifu-expression-update-{}", Uuid::new_v4()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/twitch-live");
    let old = root.join("previous-twitch");
    fs::create_dir_all(old.join("motions")).unwrap();
    fs::copy(source.join("run.py"), old.join("run.py")).unwrap();
    fs::copy(source.join("motions/nod.bvh"), old.join("motions/nod.bvh")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(source.join("extension.json")).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("test_process");
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .retain(|value| value != "expression.apply");
    manifest["settings"]
        .as_array_mut()
        .unwrap()
        .retain(|value| value["id"] != "celebration_expression");
    fs::write(
        old.join("extension.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let mut store = ExtensionStore::open(root.join("store")).unwrap();
    let id = store.install_integration(&old).unwrap();
    let previous = vec![
        Capability::LifecycleObserve,
        Capability::MotionPlay,
        Capability::MessageDeliver,
    ];
    store.set_grants(id, previous.clone()).unwrap();
    assert_eq!(store.install_integration(&source).unwrap(), id);
    let pack = store
        .packs()
        .iter()
        .find(|p| p.installation_id == id)
        .unwrap();
    assert_eq!(pack.grants.len(), previous.len());
    assert!(previous.iter().all(|grant| pack.grants.contains(grant)));
    assert!(!pack.grants.contains(&Capability::ExpressionApply));
    assert_eq!(
        pack.settings["celebration_expression"],
        SettingValue::Text(String::new())
    );
}

#[test]
fn expression_selection_survives_without_avatar_and_never_grants_permission() {
    let root = std::env::temp_dir().join(format!("waifu-expression-setting-{}", Uuid::new_v4()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let mut store = ExtensionStore::open(root.clone()).unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/twitch-live");
    let id = store.install_integration(&source).unwrap();
    store
        .set_settings(
            id,
            [(
                "celebration_expression".into(),
                SettingValue::Text("Happy Face".into()),
            )]
            .into(),
        )
        .unwrap();
    drop(store);
    let mut store = ExtensionStore::open(root).unwrap();
    let pack = store
        .packs()
        .iter()
        .find(|p| p.installation_id == id)
        .unwrap();
    assert_eq!(
        pack.settings["celebration_expression"],
        SettingValue::Text("Happy Face".into())
    );
    assert!(pack.grants.is_empty());
    assert!(!pack.enabled);
    let saved = pack.settings.clone();
    for invalid in ["blink", "lookLeft", "aa", "  ", "bad\nface"] {
        assert!(
            store
                .set_settings(
                    id,
                    [(
                        "celebration_expression".into(),
                        SettingValue::Text(invalid.into())
                    )]
                    .into()
                )
                .is_err()
        );
        assert_eq!(
            store
                .packs()
                .iter()
                .find(|p| p.installation_id == id)
                .unwrap()
                .settings,
            saved
        );
    }
    store
        .set_settings(
            id,
            [(
                "celebration_expression".into(),
                SettingValue::Text(String::new()),
            )]
            .into(),
        )
        .unwrap();
    assert_eq!(
        store
            .packs()
            .iter()
            .find(|p| p.installation_id == id)
            .unwrap()
            .settings["celebration_expression"],
        SettingValue::Text(String::new())
    );
}
