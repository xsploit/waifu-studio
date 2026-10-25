#![cfg(target_os = "linux")]
//! Installed motion-ended metadata hook, real local child, no GPU or provider.
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{
    animation::{AnimationState, MotionObservations, parse_bvh},
    behavior::BehaviorController,
    extension_actions::{ActionCatalog, ActionControllers},
    extension_installed::InstalledRuntime,
    extension_lifecycle::{LifecycleEvent, LifecycleJournal},
    extension_protocol::{Capability, Target},
    extension_runtime::ExtensionRuntime,
    extension_store::ExtensionStore,
    extensions::PackView,
};
struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
    binding: InstalledRuntime,
    animation: AnimationState,
    observations: MotionObservations,
    behavior: BehaviorController,
    journal: LifecycleJournal,
}
impl Fixture {
    fn new() -> Self {
        let animation = AnimationState::default();
        let observations = animation.motion_observations();
        Self {
            root: std::env::temp_dir().join(format!("motion-end-lifecycle-{}", Uuid::new_v4())),
            runtime: Default::default(),
            binding: Default::default(),
            animation,
            observations,
            behavior: Default::default(),
            journal: Default::default(),
        }
    }
    fn frame(&mut self, views: &[PackView]) -> Vec<LifecycleEvent> {
        self.binding.reconcile(views, &self.runtime);
        assert!(self.binding.errors.is_empty(), "{:?}", self.binding.errors);
        self.observations.attach(&mut self.animation);
        self.runtime.collect_motion_observations(&self.observations);
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
        events.into_iter().map(|e| e.event).collect()
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
fn installed_motion_end_hook_receives_host_snapshot_null_target_without_actions_or_replay() {
    let mut f = Fixture::new();
    let mut store = ExtensionStore::open(f.root.join("store")).unwrap();
    let id = store
        .install_integration(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/extensions/motion-end-observer"),
        )
        .unwrap();
    // The installation may hold broad grants; invocation must still get only observe.
    store
        .set_grants(id, vec![Capability::LifecycleObserve])
        .unwrap();
    store.set_enabled(id, true).unwrap();
    let views = [view(&store)];
    let receipt = views[0]
        .data_directory
        .as_ref()
        .unwrap()
        .join("latest-motion-end.json");
    assert!(f.frame(&views).is_empty());
    f.animation.select_clip(
        Arc::new(
            parse_bvh(include_str!(
                "../assets/animations/companion/talking-nod.bvh"
            ))
            .unwrap(),
        ),
        Some("private-path-not-for-wire.bvh".into()),
    );
    f.animation.stop();
    let events = f.frame(&views);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        LifecycleEvent::MotionPlaybackStarted { .. }
    ));
    let LifecycleEvent::MotionPlaybackEnded { playback_id, .. } = events[1] else {
        panic!("missing generic terminal")
    };
    let expected = json!({"type":"motion.playback.ended","playback_id":playback_id,"source_target":null,"motion_id":null,"source":{"type":"host"},"outcome":"cancelled"});
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            f.frame(&views).is_empty(),
            "hook generated recursive source"
        );
        let actual = fs::read(&receipt)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if actual.as_ref() == Some(&expected) && f.runtime.process_count() == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "observer failed: {actual:?}, {:?}",
            f.runtime.views()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    f.runtime.stop(id);
    f.runtime.restart(id);
    assert!(f.frame(&views).is_empty());
    assert_eq!(f.runtime.process_count(), 0);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&receipt).unwrap()).unwrap(),
        expected
    );
}
