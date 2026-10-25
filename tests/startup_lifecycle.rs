#![cfg(target_os = "linux")]
//! Local startup source -> installed hook binding -> real metadata-only child.
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
    extension_lifecycle::{AppReadySource, LifecycleEvent, LifecycleJournal, SequencedEvent},
    extension_protocol::{Capability, Target},
    extension_runtime::ExtensionRuntime,
    extension_store::ExtensionStore,
    extensions::PackView,
};

struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    installed: InstalledRuntime,
    startup: AppReadySource,
    journal: LifecycleJournal,
    animation: AnimationState,
    behavior: BehaviorController,
    target: Option<Target>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!("startup-lifecycle-{}", Uuid::new_v4())),
            runtime: ExtensionRuntime::default(),
            installed: InstalledRuntime::default(),
            startup: AppReadySource::default(),
            journal: LifecycleJournal::default(),
            animation: AnimationState::default(),
            behavior: BehaviorController::default(),
            target: Some(Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            }),
        }
    }
    fn frame(
        &mut self,
        views: &[PackView],
        initial_load_busy: bool,
        source: &[LifecycleEvent],
    ) -> Vec<SequencedEvent> {
        // Same source boundary as extension_host::frame: binding before startup,
        // startup before drain. Initial discovery busy state is supplied locally.
        self.installed.reconcile(views, &self.runtime);
        assert!(
            self.installed.errors.is_empty(),
            "{:?}",
            self.installed.errors
        );
        for event in source {
            self.journal.record(event.clone());
        }
        self.startup
            .after_reconcile(!initial_load_busy, &mut self.journal);
        let events = self.journal.drain();
        self.runtime.frame(
            &events,
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
        events
    }
    fn wait(&mut self, views: &[PackView], receipt: &std::path::Path, expected: &Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut previous = None;
        let mut revision = 0;
        loop {
            self.target = if self.target.is_some() {
                None
            } else {
                Some(Target {
                    uuid: Uuid::new_v4(),
                    generation: 7,
                })
            };
            let new = Some(Uuid::new_v4());
            revision += 1;
            self.frame(
                views,
                false,
                &[LifecycleEvent::CharacterSelected {
                    previous,
                    new,
                    selection_revision: revision,
                }],
            );
            previous = new;
            let actual = fs::read(receipt)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            if actual.as_ref() == Some(expected) && self.runtime.process_count() == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "startup observer did not finish: {actual:?}, {:?}",
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
fn install(f: &Fixture) -> (ExtensionStore, Uuid) {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/startup-observer");
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let id = store.install_integration(&source).unwrap();
    store
        .set_grants(id, vec![Capability::LifecycleObserve])
        .unwrap();
    store.set_enabled(id, true).unwrap();
    (store, id)
}

#[test]
fn installed_startup_observer_receives_one_ready_after_binding_without_avatar_authority() {
    let mut f = Fixture::new();
    let (mut store, id) = install(&f);
    let enabled = [view(&store)];
    let receipt = enabled[0]
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest-startup.json");
    assert!(f.frame(&enabled, true, &[]).is_empty());
    assert_eq!(
        f.runtime.process_count(),
        0,
        "startup ran before initial loading settled"
    );
    assert!(!receipt.exists());
    let events = f.frame(&enabled, false, &[]);
    assert_eq!(events.len(), 1);
    let LifecycleEvent::AppReady { app_run_id } = events[0].event else {
        panic!("expected startup edge");
    };
    f.wait(
        &enabled,
        &receipt,
        &json!({"type":"app.ready", "app_run_id":app_run_id}),
    );
    let original = fs::read(&receipt).unwrap();
    f.runtime.restart(id);
    assert!(f.frame(&enabled, false, &[]).is_empty());
    assert_eq!(
        f.runtime.process_count(),
        0,
        "restart replayed historical startup"
    );
    store.set_enabled(id, false).unwrap();
    f.frame(&[view(&store)], false, &[]);
    store.set_enabled(id, true).unwrap();
    f.frame(&[view(&store)], false, &[]);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "enable replayed historical startup"
    );
    assert_eq!(fs::read(&receipt).unwrap(), original);
}

#[test]
fn late_install_does_not_receive_historical_app_ready() {
    let mut f = Fixture::new();
    let events = f.frame(&[], false, &[]);
    assert!(matches!(events[0].event, LifecycleEvent::AppReady { .. }));
    let (store, _) = install(&f);
    let enabled = [view(&store)];
    let receipt = enabled[0]
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest-startup.json");
    for busy in [true, false, false] {
        assert!(f.frame(&enabled, busy, &[]).is_empty());
        assert_eq!(f.runtime.process_count(), 0);
    }
    assert!(!receipt.exists());
}
