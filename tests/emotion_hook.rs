#![cfg(target_os = "linux")]
//! Installed example -> production binding/runtime -> real one-shot Python child.
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{
    animation::AnimationState,
    behavior::BehaviorController,
    extension_actions::{ActionCatalog, ActionControllers},
    extension_installed::InstalledRuntime,
    extension_lifecycle::{LifecycleEvent, SequencedEvent},
    extension_protocol::{Capability, EmotionOwner, Target},
    extension_runtime::ExtensionRuntime,
    extension_store::ExtensionStore,
    extensions::PackView,
    reply_affect::AffectEmotion,
};

struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    binding: InstalledRuntime,
    animation: AnimationState,
    behavior: BehaviorController,
    catalog: ActionCatalog,
    target: Target,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!("emotion-hook-{}", Uuid::new_v4())),
            runtime: ExtensionRuntime::default(),
            binding: InstalledRuntime::default(),
            animation: AnimationState::default(),
            behavior: BehaviorController::default(),
            catalog: ActionCatalog::from_packs([], &[]),
            target: Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            },
        }
    }
    fn frame(&mut self, views: &[PackView], events: &[SequencedEvent]) {
        self.binding.reconcile(views, &self.runtime);
        assert!(self.binding.errors.is_empty(), "{:?}", self.binding.errors);
        self.runtime.frame(
            events,
            Some(self.target),
            false,
            &self.catalog,
            ActionControllers {
                animation: &mut self.animation,
                behavior: &mut self.behavior,
                external_messages: None,
            },
        );
        assert!(self.runtime.process_count() <= 1);
    }
    fn receipt_then_idle(&mut self, views: &[PackView], path: &std::path::Path, expected: &Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.frame(views, &[]);
            let receipt = fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            if receipt.as_ref() == Some(expected) && self.runtime.process_count() == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "receipt: {receipt:?}; runtime: {:?}",
                self.runtime.views()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.runtime.shutdown(Duration::from_secs(2));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn view(store: &ExtensionStore) -> PackView {
    let installed = store.packs()[0].clone();
    let content = store.resolved(&installed).unwrap();
    let data = store.data_directory(&installed).unwrap();
    PackView {
        credentials: waifu_studio::extension_credentials::status(
            &data,
            &content.manifest.credentials,
        )
        .unwrap(),
        installed,
        content: Some(content),
        data_directory: Some(data),
        error: None,
    }
}
fn emotion(
    sequence: u64,
    character: Uuid,
    owner: EmotionOwner,
    reply_id: Option<Uuid>,
) -> SequencedEvent {
    SequencedEvent {
        sequence,
        event: LifecycleEvent::EmotionChanged {
            character,
            owner,
            previous: AffectEmotion::Happy,
            emotion: AffectEmotion::Neutral,
            reply_id,
        },
    }
}
fn expected(character: Uuid, owner: &str, reply_id: Option<Uuid>) -> Value {
    json!({"type":"emotion.changed", "character":character, "owner":owner,
        "previous":"happy", "emotion":"neutral", "reply_id":reply_id})
}

#[test]
fn installed_emotion_example_records_source_metadata_and_respects_disable_reenable() {
    let mut f = Fixture::new();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/emotion-hook");
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let id = store.install_integration(&source).unwrap();
    store
        .set_grants(id, vec![Capability::LifecycleObserve])
        .unwrap();
    store.set_enabled(id, true).unwrap();
    let enabled = [view(&store)];
    let receipt = enabled[0]
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest.json");
    f.frame(&enabled, &[]);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "idle registration launched a child"
    );
    assert!(!receipt.exists());
    let character = Uuid::new_v4();
    assert_ne!(character, f.target.uuid);
    let reply_id = Some(Uuid::new_v4());
    f.frame(
        &enabled,
        &[emotion(1, character, EmotionOwner::Studio, reply_id)],
    );
    f.receipt_then_idle(&enabled, &receipt, &expected(character, "studio", reply_id));
    let original = fs::read(&receipt).unwrap();
    let actual: Value = serde_json::from_slice(&original).unwrap();
    assert!(actual.get("target").is_none());
    assert!(actual.get("text").is_none());

    store.set_enabled(id, false).unwrap();
    let disabled = [view(&store)];
    f.frame(
        &disabled,
        &[emotion(2, character, EmotionOwner::SceneChat, None)],
    );
    for _ in 0..3 {
        f.frame(&disabled, &[]);
        assert_eq!(
            f.runtime.process_count(),
            0,
            "disabled hook launched a child"
        );
        assert_eq!(fs::read(&receipt).unwrap(), original);
    }
    store.set_enabled(id, true).unwrap();
    let enabled = [view(&store)];
    f.frame(&enabled, &[]);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "reenable replayed a disabled event"
    );
    assert_eq!(fs::read(&receipt).unwrap(), original);
    f.frame(
        &enabled,
        &[emotion(3, character, EmotionOwner::SceneChat, None)],
    );
    f.receipt_then_idle(&enabled, &receipt, &expected(character, "scene_chat", None));
    assert!(f.runtime.shutdown(Duration::from_secs(2)));
}
