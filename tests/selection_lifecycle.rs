#![cfg(target_os = "linux")]
//! Installed selection hook through production binding, relay and a real child.
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
    extension_protocol::{Capability, Target},
    extension_runtime::ExtensionRuntime,
    extension_store::ExtensionStore,
    extensions::PackView,
};

struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    installed: InstalledRuntime,
    animation: AnimationState,
    behavior: BehaviorController,
    target: Option<Target>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!("selection-lifecycle-{}", Uuid::new_v4())),
            runtime: ExtensionRuntime::default(),
            installed: InstalledRuntime::default(),
            animation: AnimationState::default(),
            behavior: BehaviorController::default(),
            target: Some(Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            }),
        }
    }
    fn frame(&mut self, views: &[PackView], events: &[SequencedEvent]) {
        self.installed.reconcile(views, &self.runtime);
        assert!(
            self.installed.errors.is_empty(),
            "{:?}",
            self.installed.errors
        );
        self.runtime.frame(
            events,
            self.target,
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                external_messages: None,
                animation: &mut self.animation,
                behavior: &mut self.behavior,
            },
        );
        assert!(self.runtime.process_count() <= 1);
    }
    fn wait(&mut self, views: &[PackView], receipt: &std::path::Path, expected: &Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // Deliberately change avatar availability while this metadata-only
            // invocation is launching/running. It must never gain that target.
            self.target = if self.target.is_some() {
                None
            } else {
                Some(Target {
                    uuid: Uuid::new_v4(),
                    generation: 7,
                })
            };
            self.frame(views, &[]);
            let actual = fs::read(receipt)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            if actual.as_ref() == Some(expected) && self.runtime.process_count() == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "selection observer did not finish: {actual:?}, {:?}",
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
fn selected(revision: u64, previous: Option<Uuid>, new: Option<Uuid>) -> SequencedEvent {
    SequencedEvent {
        sequence: revision,
        event: LifecycleEvent::CharacterSelected {
            previous,
            new,
            selection_revision: revision,
        },
    }
}

#[test]
fn installed_selection_observer_is_targetless_latest_only_and_never_replays_on_restart() {
    let mut f = Fixture::new();
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/selection-observer");
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
        .join("latest-selection.json");
    f.frame(&enabled, &[]);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "registration replayed selection at startup"
    );
    assert!(!receipt.exists());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let latest = selected(3, Some(b), Some(a));
    f.frame(
        &enabled,
        &[
            selected(1, None, Some(a)),
            selected(2, Some(a), Some(b)),
            latest.clone(),
        ],
    );
    f.wait(
        &enabled,
        &receipt,
        &json!({
            "type":"character.selected", "previous":b, "new":a, "selection_revision":3,
        }),
    );
    let original = fs::read(&receipt).unwrap();
    f.runtime.restart(id);
    f.frame(&enabled, &[latest]);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "restart replayed an observed selection"
    );
    assert_eq!(fs::read(&receipt).unwrap(), original);
    store.set_enabled(id, false).unwrap();
    f.frame(&[view(&store)], &[selected(4, Some(a), Some(b))]);
    assert_eq!(f.runtime.process_count(), 0);
    store.set_enabled(id, true).unwrap();
    let enabled = [view(&store)];
    f.frame(&enabled, &[]);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "reenable replayed a disabled selection"
    );
    assert_eq!(fs::read(&receipt).unwrap(), original);
    f.frame(&enabled, &[selected(5, Some(b), None)]);
    f.wait(
        &enabled,
        &receipt,
        &json!({
            "type":"character.selected", "previous":b, "new":null, "selection_revision":5,
        }),
    );
}
