//! Dynamic choices persist as registered IDs, independently of current availability.
use std::{fs, path::PathBuf};
use uuid::Uuid;
use waifu_studio::{
    extension_settings::{SettingValue, Settings},
    extension_store::ExtensionStore,
};

#[test]
fn twitch_motion_choice_survives_pack_disable_and_reopen_without_becoming_a_path() {
    let root = std::env::temp_dir().join(format!("waifu-motion-setting-{}", Uuid::new_v4()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions");
    let mut store = ExtensionStore::open(root.clone()).unwrap();
    let content = store.install(&examples.join("companion-starter")).unwrap();
    let twitch = store
        .install_integration(&examples.join("twitch-live"))
        .unwrap();
    let selected = "org.waifu-studio.companion-starter/talking-tilt";
    let values: Settings = [(
        "celebration_motion".into(),
        SettingValue::Text(selected.into()),
    )]
    .into();
    store.set_settings(twitch, values).unwrap();
    store.set_enabled(content, false).unwrap();
    drop(store);
    let mut store = ExtensionStore::open(root).unwrap();
    let pack = store
        .packs()
        .iter()
        .find(|p| p.installation_id == twitch)
        .unwrap();
    assert_eq!(
        pack.settings["celebration_motion"],
        SettingValue::Text(selected.into())
    );
    assert!(!pack.enabled);
    assert!(
        pack.grants.is_empty(),
        "changing a motion must not grant controls"
    );
    let before = pack.settings.clone();
    for invalid in [
        "motion with spaces".to_owned(),
        "x".repeat(194),
        "path\nvalue".into(),
    ] {
        assert!(
            store
                .set_settings(
                    twitch,
                    [("celebration_motion".into(), SettingValue::Text(invalid))].into()
                )
                .is_err()
        );
        assert_eq!(
            store
                .packs()
                .iter()
                .find(|p| p.installation_id == twitch)
                .unwrap()
                .settings,
            before
        );
    }
    store
        .set_settings(
            twitch,
            [(
                "celebration_motion".into(),
                SettingValue::Text(String::new()),
            )]
            .into(),
        )
        .unwrap();
    let pack = store
        .packs()
        .iter()
        .find(|p| p.installation_id == twitch)
        .unwrap();
    assert_eq!(
        pack.settings["celebration_motion"],
        SettingValue::Text(String::new())
    );
}
