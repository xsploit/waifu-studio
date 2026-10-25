#![cfg(any(target_os = "linux", windows))]
//! Real installed/reconciled child and native replay, no network or models.
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{
    animation::AnimationState,
    behavior::BehaviorController,
    extension_actions::{ActionCatalog, ActionControllers, ExternalMessageSink},
    extension_installed::InstalledRuntime,
    extension_protocol::*,
    extension_runtime::ExtensionRuntime,
    extension_store::ExtensionStore,
    extensions::PackView,
};

#[derive(Default)]
struct Sink {
    accepted: Vec<(Target, IntegrationOrigin, ChatMessage)>,
    attempts: usize,
    busy: bool,
}
impl ExternalMessageSink for Sink {
    fn deliver(
        &mut self,
        _: Target,
        _: &IntegrationOrigin,
        _: &str,
    ) -> Result<Uuid, (ErrorCode, String)> {
        panic!("Discord replay must offer, never directly deliver")
    }
    fn offer_chat(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        message: &ChatMessage,
    ) -> Result<(), (ErrorCode, String)> {
        self.attempts += 1;
        if self.busy {
            return Err((ErrorCode::Busy, "Synthetic busy fixture".into()));
        }
        self.accepted
            .push((target, origin.clone(), message.clone()));
        Ok(())
    }
}
struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    binding: InstalledRuntime,
    animation: AnimationState,
    behavior: BehaviorController,
    sink: Sink,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!("discord-replay-installed-{}", Uuid::new_v4())),
            runtime: Default::default(),
            binding: Default::default(),
            animation: Default::default(),
            behavior: Default::default(),
            sink: Default::default(),
        }
    }
    fn install(&self) -> (ExtensionStore, Uuid) {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let source = self.root.join("source");
        fs::create_dir_all(&source).unwrap();
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/discord-replay");
        for file in ["extension.json", "run.py", "fixtures.json"] {
            fs::copy(example.join(file), source.join(file)).unwrap();
        }
        // Runtime current_exe is this Rust test harness. The fixture-only shim
        // supplies the actual app binary, then runs the unchanged package launcher.
        // No production URL/env override is added to the native worker.
        let executable = serde_json::to_string(env!("CARGO_BIN_EXE_waifu-studio")).unwrap();
        fs::write(source.join("test-launcher.py"),format!("#!/usr/bin/env python3\nimport os,runpy\nfrom pathlib import Path\nos.environ['WAIFU_STUDIO_EXECUTABLE']={executable}\nrunpy.run_path(str(Path(__file__).with_name('run.py')),run_name='__main__')\n")).unwrap();
        #[cfg(unix)]
        fs::set_permissions(
            source.join("test-launcher.py"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["process"]["program"] = serde_json::json!("test-launcher.py");
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut store = ExtensionStore::open(self.root.join("store")).unwrap();
        let id = store.install_integration(&source).unwrap();
        (store, id)
    }
    fn frame(&mut self, views: &[PackView], target: Option<Target>) {
        self.binding.reconcile(views, &self.runtime);
        assert!(self.binding.errors.is_empty(), "{:?}", self.binding.errors);
        self.runtime.frame(
            &[],
            target,
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                animation: &mut self.animation,
                behavior: &mut self.behavior,
                external_messages: Some(&mut self.sink),
            },
        );
    }
    fn until(&mut self, views: &[PackView], target: Option<Target>, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            self.frame(views, target);
            if done(self) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "installed replay timed out; views: {:?}",
                self.runtime.views()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn settle(&mut self, views: &[PackView], target: Option<Target>, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.frame(views, target);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime
            .begin_shutdown(&mut self.animation, &mut self.behavior);
        let _ = self.runtime.shutdown(Duration::from_secs(2));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn view(store: &ExtensionStore) -> PackView {
    let installed = store.packs()[0].clone();
    let content = store.resolved(&installed).unwrap();
    let data = store.data_directory(&installed).unwrap();
    PackView {
        credentials: Default::default(),
        installed,
        content: Some(content),
        data_directory: Some(data),
        error: None,
    }
}
fn target(generation: u64) -> Target {
    Target {
        uuid: Uuid::from_u128(11),
        generation,
    }
}
fn enable(store: &mut ExtensionStore, id: Uuid) {
    store
        .set_grants(
            id,
            vec![Capability::LifecycleObserve, Capability::MessageDeliver],
        )
        .unwrap();
    store.set_enabled(id, true).unwrap();
}
#[test]
fn installed_native_replay_waits_for_target_offers_once_restarts_and_disables() {
    let mut f = Fixture::new();
    let (mut store, id) = f.install();
    assert!(!store.packs()[0].enabled);
    assert!(store.packs()[0].grants.is_empty());
    f.settle(&[view(&store)], Some(target(1)), Duration::from_millis(50));
    assert!(f.runtime.is_empty());
    enable(&mut store, id);
    let views = [view(&store)];
    f.settle(&views, None, Duration::from_millis(200));
    assert!(f.sink.accepted.is_empty());
    f.until(&views, Some(target(1)), |f| f.sink.accepted.len() == 1);
    let (captured, origin, message) = &f.sink.accepted[0];
    assert_eq!(*captured, target(1));
    assert_eq!(origin.integration_id, "studio.example.discord-replay");
    assert_eq!(origin.display_name, "Discord replay (offline)");
    assert_eq!(message.author_id, "555");
    assert_eq!(message.author_name, "Replay viewer");
    assert_eq!(
        message.text,
        "Synthetic Discord replay: hello! This is an offline test message."
    );
    assert!(message.source_id.starts_with("discord:111:222:"));
    let first_id = message.source_id.clone();
    assert!(
        !views[0]
            .data_directory
            .as_ref()
            .unwrap()
            .join("credentials.json")
            .exists()
    );
    f.settle(&views, Some(target(2)), Duration::from_millis(200));
    assert_eq!(f.sink.accepted.len(), 1);
    f.runtime.restart(id);
    f.until(&views, Some(target(2)), |f| f.sink.accepted.len() == 2);
    assert_eq!(f.sink.accepted[1].0, target(2));
    assert_ne!(first_id, f.sink.accepted[1].2.source_id);
    store.set_enabled(id, false).unwrap();
    let disabled = [view(&store)];
    f.until(&disabled, Some(target(2)), |f| {
        f.runtime.process_count() == 0
    });
    f.settle(&disabled, Some(target(3)), Duration::from_millis(100));
    assert_eq!(f.sink.accepted.len(), 2);
}
#[test]
fn target_switch_cancels_busy_retry_in_real_installed_child() {
    let mut f = Fixture::new();
    f.sink.busy = true;
    let (mut store, id) = f.install();
    enable(&mut store, id);
    let views = [view(&store)];
    f.until(&views, Some(target(1)), |f| f.sink.attempts == 1);
    assert!(f.sink.accepted.is_empty());
    f.sink.busy = false;
    f.settle(&views, Some(target(2)), Duration::from_millis(1300));
    assert_eq!(
        f.sink.attempts, 1,
        "old-target Busy retry was retargeted or replayed"
    );
    assert!(f.sink.accepted.is_empty());
}
