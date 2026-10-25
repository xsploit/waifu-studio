#![cfg(target_os = "linux")]
//! Actual persistent incarnations -> host journal -> installed metadata-only hook.
use serde_json::Value;
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
    extension_lifecycle::{LifecycleEvent, LifecycleJournal},
    extension_process::ProcessSpec,
    extension_protocol::{Capability, IntegrationOrigin, Target},
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
    journal: LifecycleJournal,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("integration-start-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            runtime: Default::default(),
            installed: Default::default(),
            animation: Default::default(),
            behavior: Default::default(),
            journal: Default::default(),
        }
    }
    fn frame(&mut self, views: &[PackView]) -> Vec<LifecycleEvent> {
        self.installed.reconcile(views, &self.runtime);
        assert!(
            self.installed.errors.is_empty(),
            "{:?}",
            self.installed.errors
        );
        self.runtime.drain_observations(&mut self.journal);
        let events = self.journal.drain();
        self.runtime.frame(
            &events,
            Some(Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            }),
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                external_messages: None,
                animation: &mut self.animation,
                behavior: &mut self.behavior,
            },
        );
        events.into_iter().map(|event| event.event).collect()
    }
    fn start(&self, id: Uuid, script: &str) {
        self.runtime
            .start(
                id,
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-c".into(), script.into()],
                    working_directory: self.root.clone(),
                    data_directory: self.root.join(id.to_string()),
                    lifetime: None,
                },
                IntegrationOrigin {
                    integration_id: "org.example.started-service".into(),
                    display_name: "Local service".into(),
                },
                [Capability::LifecycleObserve],
            )
            .unwrap();
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
fn installed_started_observer_tracks_actual_restart_and_fast_exit_without_hook_recursion() {
    let mut f = Fixture::new();
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let observer = store
        .install_integration(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/extensions/integration-start-observer"),
        )
        .unwrap();
    store
        .set_grants(observer, vec![Capability::LifecycleObserve])
        .unwrap();
    store.set_enabled(observer, true).unwrap();
    let views = [view(&store)];
    let receipt = views[0]
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest-integration-start.json");
    assert!(
        f.frame(&views)
            .iter()
            .all(|event| !matches!(event, LifecycleEvent::ExtensionStarted { .. }))
    );
    let service = Uuid::new_v4();
    f.start(service, "import sys\nfor line in sys.stdin: pass");
    let mut previous_session = None;
    for generation in 1..=2 {
        if generation == 2 {
            f.runtime.restart(service);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut observed = None;
        loop {
            for event in f.frame(&views) {
                if let LifecycleEvent::ExtensionStarted {
                    installation_id,
                    session_id,
                    process_generation,
                } = event
                {
                    assert_eq!(
                        installation_id, service,
                        "hook child must not generate a source event"
                    );
                    assert_eq!(process_generation, generation);
                    assert!(
                        observed.replace(session_id).is_none(),
                        "duplicate incarnation source"
                    );
                }
            }
            let actual = fs::read(&receipt)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            if let (Some(session), Some(actual)) = (observed, actual)
                && actual
                    == serde_json::json!({"type":"extension.started", "installation_id":service,
                    "session_id":session, "process_generation":generation})
                && f.runtime.process_count() == 1
            {
                assert_ne!(Some(session), previous_session);
                previous_session = Some(session);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "missing installed receipt: {:?}",
                f.runtime.views()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    // A real fast-exiting incarnation is still observed, even when removed
    // before the next ordinary source drain.
    f.runtime.remove(service);
    let deadline = Instant::now() + Duration::from_secs(3);
    while f.runtime.process_count() != 0 {
        assert!(
            f.frame(&views)
                .iter()
                .all(|event| !matches!(event, LifecycleEvent::ExtensionStarted { .. }))
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let fast = Uuid::new_v4();
    f.start(
        fast,
        "from pathlib import Path\nPath('fast-spawned').write_text('yes')",
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !f.root.join("fast-spawned").exists() {
        assert!(Instant::now() < deadline, "fast child never spawned");
        std::thread::sleep(Duration::from_millis(2));
    }
    f.runtime.remove(fast);
    let mut fast_sources = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    while f.runtime.process_count() != 0 {
        for event in f.frame(&views) {
            if let LifecycleEvent::ExtensionStarted {
                installation_id, ..
            } = event
            {
                assert_eq!(installation_id, fast);
                fast_sources += 1;
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    // Removal-frame collection may remain buffered until the following drain.
    for event in f.frame(&views) {
        if let LifecycleEvent::ExtensionStarted {
            installation_id, ..
        } = event
        {
            assert_eq!(installation_id, fast);
            fast_sources += 1;
        }
    }
    assert_eq!(fast_sources, 1, "fast incarnation lost or duplicated");
    f.runtime.begin_shutdown(&mut f.animation, &mut f.behavior);
    assert!(
        f.runtime
            .start(
                Uuid::new_v4(),
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec![],
                    working_directory: f.root.clone(),
                    data_directory: f.root.clone(),
                    lifetime: None,
                },
                IntegrationOrigin {
                    integration_id: "org.example.quiesced".into(),
                    display_name: "Quiesced".into()
                },
                []
            )
            .is_err()
    );
}
