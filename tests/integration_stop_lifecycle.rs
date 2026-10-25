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
        let root = std::env::temp_dir().join(format!("integration-stop-{}", Uuid::new_v4()));
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
fn installed_stop_hooks_observe_first_request_and_real_terminal_without_recursion() {
    let mut f = Fixture::new();
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let observer = store
        .install_integration(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/extensions/integration-stop-observer"),
        )
        .unwrap();
    store
        .set_grants(observer, vec![Capability::LifecycleObserve])
        .unwrap();
    store.set_enabled(observer, true).unwrap();
    let views = [view(&store)];
    let data = views[0].data_directory.as_ref().unwrap();
    f.frame(&views);
    let service = Uuid::new_v4();
    f.start(service, "import sys\nfor line in sys.stdin: pass");
    let deadline = Instant::now() + Duration::from_secs(5);
    let (session, generation) = loop {
        let events = f.frame(&views);
        if let Some((session, generation)) = events.iter().find_map(|e| match e {
            LifecycleEvent::ExtensionStarted {
                installation_id,
                session_id,
                process_generation,
            } if *installation_id == service => Some((*session_id, *process_generation)),
            _ => None,
        }) {
            break (session, generation);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    f.runtime.stop(service);
    f.runtime.stop(service); // first reason only, no duplicate lifecycle records
    f.runtime.remove(service);
    let expected_request = serde_json::json!({"type":"extension.stopping", "installation_id":service, "session_id":session, "process_generation":generation, "reason":"disabled"});
    let expected_exit = serde_json::json!({"type":"extension.stopped", "installation_id":service, "session_id":session, "process_generation":generation, "exit_code":null, "requested_reason":"disabled"});
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut requests = 0;
    let mut exits = 0;
    loop {
        for event in f.frame(&views) {
            match event {
                LifecycleEvent::ExtensionStopping {
                    installation_id, ..
                } => {
                    assert_eq!(installation_id, service);
                    requests += 1;
                }
                LifecycleEvent::ExtensionStopped {
                    installation_id, ..
                } => {
                    assert_eq!(installation_id, service);
                    exits += 1;
                }
                LifecycleEvent::ExtensionStarted { .. } => {
                    panic!("observer generated recursive source")
                }
                _ => (),
            }
        }
        let read = |name| {
            fs::read(data.join(name))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        };
        if read("latest-stop-request.json").as_ref() == Some(&expected_request)
            && read("latest-exit.json").as_ref() == Some(&expected_exit)
            && f.runtime.process_count() == 0
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stop observer did not finish: {:?}",
            f.runtime.views()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!((requests, exits), (1, 1));
    assert!(f.frame(&views).is_empty());
}
