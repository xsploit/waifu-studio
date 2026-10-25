#![cfg(target_os = "linux")]
//! Optional Node.js example through the real installed runtime, without services.
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
    extension_protocol::Capability,
    extension_runtime::ExtensionRuntime,
    extension_store::ExtensionStore,
    extensions::PackView,
};
struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    binding: InstalledRuntime,
    animation: AnimationState,
    behavior: BehaviorController,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!("javascript-lifecycle-{}", Uuid::new_v4())),
            runtime: ExtensionRuntime::default(),
            binding: InstalledRuntime::default(),
            animation: AnimationState::default(),
            behavior: BehaviorController::default(),
        }
    }
    fn frame(&mut self, views: &[PackView], events: &[SequencedEvent]) {
        self.binding.reconcile(views, &self.runtime);
        assert!(self.binding.errors.is_empty(), "{:?}", self.binding.errors);
        self.runtime.frame(
            events,
            None,
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                animation: &mut self.animation,
                behavior: &mut self.behavior,
                external_messages: None,
            },
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime
            .begin_shutdown(&mut self.animation, &mut self.behavior);
        self.runtime.shutdown(Duration::from_secs(2));
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
#[test]
fn installed_javascript_observer_receives_ready_then_stopping_without_actions() {
    let Ok(node) = std::process::Command::new("node").arg("--version").output() else {
        eprintln!("SKIPPED: optional JavaScript example requires Node.js on PATH");
        return;
    };
    assert!(node.status.success());
    eprintln!(
        "Node.js fixture: {}",
        String::from_utf8_lossy(&node.stdout).trim()
    );
    let mut fixture = Fixture::new();
    let mut store = ExtensionStore::open(fixture.root.join("store")).unwrap();
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/javascript-observer");
    let id = store.install_integration(&source).unwrap();
    store
        .set_grants(id, vec![Capability::LifecycleObserve])
        .unwrap();
    store.set_enabled(id, true).unwrap();
    let views = [view(&store)];
    let data = views[0].data_directory.as_ref().unwrap();
    let app_run_id = Uuid::new_v4();
    fixture.frame(
        &views,
        &[SequencedEvent {
            sequence: 1,
            event: LifecycleEvent::AppReady { app_run_id },
        }],
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        fixture.frame(&views, &[]);
        let receipt = fs::read(data.join("latest-event.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
        if receipt == Some(json!({"type":"app.ready", "app_run_id":app_run_id})) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "JavaScript startup receipt: {receipt:?}; {:?}",
            fixture.runtime.views()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        data.join("started.json").is_file(),
        "real JavaScript process never started"
    );
    let start = Instant::now();
    fixture
        .runtime
        .begin_shutdown(&mut fixture.animation, &mut fixture.behavior);
    assert!(fixture.runtime.shutdown(Duration::from_secs(2)));
    assert!(start.elapsed() < Duration::from_millis(2300));
    let receipt: Value =
        serde_json::from_slice(&fs::read(data.join("latest-shutdown.json")).unwrap()).unwrap();
    assert_eq!(
        receipt,
        json!({"type":"app.stopping", "app_run_id":app_run_id})
    );
    assert!(fixture.runtime.views().iter().all(|entry| !entry.active));
    assert!(!fixture.animation.is_posing());
}
