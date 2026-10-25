#![cfg(target_os = "linux")]
//! Installed test handler against real host expression ownership, without Twitch.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{
    animation::AnimationState,
    behavior::{BehaviorConfig, BehaviorController},
    extension_actions::{ActionCatalog, ActionControllers},
    extension_installed::InstalledRuntime,
    extension_protocol::{Capability, Target},
    extension_runtime::ExtensionRuntime,
    extension_settings::SettingValue,
    extension_store::ExtensionStore,
    extensions::PackView,
};

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for item in fs::read_dir(source).unwrap() {
        let item = item.unwrap();
        if item.file_type().unwrap().is_dir() {
            copy_tree(&item.path(), &destination.join(item.file_name()));
        } else {
            fs::copy(item.path(), destination.join(item.file_name())).unwrap();
        }
    }
}

struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    binding: InstalledRuntime,
    animation: AnimationState,
    behavior: BehaviorController,
    target: Target,
    tick: Instant,
}
impl Fixture {
    fn new() -> Self {
        let target = Target {
            uuid: Uuid::new_v4(),
            generation: 1,
        };
        let mut behavior = BehaviorController::default();
        behavior
            .select(
                Some(target.uuid),
                &BehaviorConfig::default(),
                &["happy".into()],
            )
            .unwrap();
        Self {
            root: std::env::temp_dir().join(format!("waifu-installed-test-{}", Uuid::new_v4())),
            runtime: Default::default(),
            binding: Default::default(),
            animation: Default::default(),
            behavior,
            target,
            tick: Instant::now(),
        }
    }
    fn frame(&mut self, view: &PackView) {
        self.frame_events(view, &[]);
    }
    fn frame_events(
        &mut self,
        view: &PackView,
        events: &[waifu_studio::extension_lifecycle::SequencedEvent],
    ) {
        self.binding
            .reconcile(std::slice::from_ref(view), &self.runtime);
        assert!(self.binding.errors.is_empty(), "{:?}", self.binding.errors);
        let now = Instant::now();
        self.behavior
            .tick(now.duration_since(self.tick).as_secs_f32());
        self.tick = now;
        let catalog = ActionCatalog::from_packs([], &["happy".into()]);
        self.runtime.frame(
            events,
            Some(self.target),
            false,
            &catalog,
            ActionControllers {
                animation: &mut self.animation,
                behavior: &mut self.behavior,
                external_messages: None,
            },
        );
    }
    fn until(&mut self, view: &PackView, mut ready: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.frame(view);
            if ready(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "test child timed out: {:?}",
                self.runtime.views()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn face_active(&self) -> bool {
        !self
            .behavior
            .weights_for(
                Some(self.target.uuid),
                &["happy".into()],
                &BTreeMap::new(),
                "",
            )
            .is_empty()
    }
}

#[test]
fn configurable_hook_filters_real_host_events_before_applying_saved_expression() {
    use waifu_studio::extension_lifecycle::{LifecycleEvent, ReplyOutcome, SequencedEvent};
    let mut f = Fixture::new();
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let id = store
        .install_integration(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/reaction-hook"),
        )
        .unwrap();
    store
        .set_grants(
            id,
            vec![Capability::LifecycleObserve, Capability::ExpressionApply],
        )
        .unwrap();
    store
        .set_settings(
            id,
            [
                ("event".into(), SettingValue::Text("Speech started".into())),
                ("motion".into(), SettingValue::Text(String::new())),
                ("expression".into(), SettingValue::Text("happy".into())),
                (
                    "character".into(),
                    SettingValue::Text(f.target.uuid.to_string()),
                ),
            ]
            .into(),
        )
        .unwrap();
    store.set_enabled(id, true).unwrap();
    let enabled = view(&store);
    let reply = SequencedEvent {
        sequence: 1,
        event: LifecycleEvent::ReplyFinished {
            reply_id: Uuid::new_v4(),
            character: Some(f.target.uuid),
            outcome: ReplyOutcome::Completed {
                text: "Private reply is not granted".into(),
            },
        },
    };
    f.frame_events(&enabled, &[reply]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.runtime.process_count() > 0 {
        f.frame(&enabled);
        assert!(
            !f.face_active(),
            "unselected source event applied an expression"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let receipt = enabled
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest-hook.json");
    let ignored: serde_json::Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(ignored, serde_json::json!({"state":"ignored"}));
    let speech = SequencedEvent {
        sequence: 2,
        event: LifecycleEvent::SpeechStarted {
            playback_id: Uuid::new_v4(),
            character: Some(f.target.uuid),
            voice_generation: 1,
            reply_id: None,
        },
    };
    f.frame_events(&enabled, &[speech]);
    f.until(&enabled, |f| f.face_active());
    f.until(&enabled, |f| f.runtime.process_count() == 0);
    assert!(
        !f.face_active(),
        "finished hook retained expression ownership"
    );
    let completed: serde_json::Value = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(completed, serde_json::json!({"state":"completed"}));
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.runtime.shutdown(Duration::from_secs(2));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn view(store: &ExtensionStore) -> PackView {
    let installed = store.packs()[0].clone();
    PackView {
        content: Some(store.resolved(&installed).unwrap()),
        data_directory: Some(store.data_directory(&installed).unwrap()),
        installed,
        credentials: Default::default(),
        error: None,
    }
}

#[test]
fn installed_manual_test_uses_saved_settings_and_cancels_expression_on_disable() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    let source = f.root.join("source");
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/twitch-live"),
        &source,
    );
    // A quiet fixture service replaces the network receiver. The real bundled
    // test handler, installation, settings, binding and host actions stay intact.
    fs::write(
        source.join("service.py"),
        "#!/usr/bin/env python3\nimport sys\nfor line in sys.stdin: pass\n",
    )
    .unwrap();
    fs::set_permissions(source.join("service.py"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = source.join("extension.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["process"] = serde_json::json!({"program":"service.py"});
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let id = store.install_integration(&source).unwrap();
    store
        .set_grants(
            id,
            vec![Capability::LifecycleObserve, Capability::ExpressionApply],
        )
        .unwrap();
    store
        .set_settings(
            id,
            [
                (
                    "celebration_motion".into(),
                    SettingValue::Text(String::new()),
                ),
                (
                    "celebration_expression".into(),
                    SettingValue::Text("happy".into()),
                ),
            ]
            .into(),
        )
        .unwrap();
    let disabled = view(&store);
    f.frame(&disabled);
    assert!(f.runtime.start_test(id).is_err());
    store.set_enabled(id, true).unwrap();
    let enabled = view(&store);
    f.frame(&enabled);
    assert!(!f.face_active());
    f.runtime.start_test(id).unwrap();
    assert!(
        f.runtime.start_test(id).is_err(),
        "repeated clicks overlapped tests"
    );
    f.until(&enabled, |f| f.face_active());
    f.until(&enabled, |f| !f.runtime.test_active(id));
    assert!(!f.face_active(), "completed test retained face ownership");
    let receipt = enabled
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest-test.json");
    let result: serde_json::Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"state":"completed"}),
        "handler exited without completing the saved reaction test"
    );

    f.runtime.start_test(id).unwrap();
    f.until(&enabled, |f| f.face_active());
    store.set_enabled(id, false).unwrap();
    let disabled = view(&store);
    f.until(&disabled, |f| f.runtime.is_empty());
    assert!(!f.face_active(), "disabled test retained face ownership");
    assert!(f.runtime.start_test(id).is_err());
}

#[test]
fn configurable_hook_preview_uses_saved_character_filter_and_real_face_ownership() {
    let mut f = Fixture::new();
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let id = store
        .install_integration(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/reaction-hook"),
        )
        .unwrap();
    store
        .set_grants(
            id,
            vec![Capability::LifecycleObserve, Capability::ExpressionApply],
        )
        .unwrap();
    let values = |character: String| {
        [
            ("motion".into(), SettingValue::Text(String::new())),
            ("expression".into(), SettingValue::Text("happy".into())),
            ("character".into(), SettingValue::Text(character)),
        ]
        .into()
    };
    store
        .set_settings(id, values(f.target.uuid.to_string()))
        .unwrap();
    store.set_enabled(id, true).unwrap();
    let enabled = view(&store);
    f.frame(&enabled);
    f.runtime.start_test(id).unwrap();
    f.until(&enabled, |f| f.face_active());
    f.until(&enabled, |f| !f.runtime.test_active(id));
    assert_eq!(
        f.runtime.test_status(id).as_deref(),
        Some("Test process completed")
    );
    assert!(!f.face_active(), "preview retained expression ownership");

    // A configured character is only a filter. It must never redirect an
    // action to a character other than the currently captured host target.
    store
        .set_settings(id, values(Uuid::new_v4().to_string()))
        .unwrap();
    let filtered = view(&store);
    f.until(&filtered, |f| {
        f.runtime
            .views()
            .iter()
            .any(|view| view.id == id && view.enabled)
    });
    f.runtime.start_test(id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.runtime.test_active(id) {
        f.frame(&filtered);
        assert!(
            !f.face_active(),
            "nonmatching character applied an expression"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        f.runtime.test_status(id).as_deref(),
        Some("Test process completed")
    );
}
