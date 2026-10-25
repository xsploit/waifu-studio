//! Retained source evidence tests, independent of the lossy transport event queue.
use super::*;
use crate::{
    animation::AnimationState,
    behavior::BehaviorController,
    extension_lifecycle::{LifecycleEvent, LifecycleJournal},
};
struct Fixture {
    runtime: ExtensionRuntime,
    root: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("started-source-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Self {
            runtime: Default::default(),
            root,
        }
    }
    fn start(&self, id: Uuid, missing: bool) {
        self.start_script(id, missing, "pass");
    }
    fn start_script(&self, id: Uuid, missing: bool, script: &str) {
        self.runtime
            .start(
                id,
                ProcessSpec {
                    program: if missing {
                        self.root.join("missing")
                    } else {
                        "/usr/bin/python3".into()
                    },
                    args: vec!["-c".into(), script.into()],
                    working_directory: self.root.clone(),
                    data_directory: self.root.join(id.to_string()),
                    lifetime: None,
                },
                IntegrationOrigin {
                    integration_id: "org.example.started".into(),
                    display_name: "Started test".into(),
                },
                [Capability::LifecycleObserve],
            )
            .unwrap();
    }
    fn wait_terminal(&self, id: Uuid) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut inner = self.runtime.inner();
            let entry = inner.entries.get_mut(&id).unwrap();
            entry.connection.reap_shutdown();
            if entry.connection.termination().is_some() {
                return;
            }
            drop(inner);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.begin_shutdown(
            &mut AnimationState::default(),
            &mut BehaviorController::default(),
        );
        let _ = self.runtime.shutdown(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[test]
fn started_retained_receipt_collects_once_and_excludes_hook_and_manual_children_at_source() {
    let f = Fixture::new();
    let id = Uuid::new_v4();
    f.start(id, false);
    f.wait_terminal(id);
    let mut journal = LifecycleJournal::default();
    // Exercise exactly the collector used immediately before replacement/drop.
    {
        let mut inner = f.runtime.inner();
        let Inner {
            entries,
            observations,
            ..
        } = &mut *inner;
        let entry = entries.get_mut(&id).unwrap();
        entry.hook_target = Some(None);
        ExtensionRuntime::collect_started_entry(id, entry, observations);
        assert!(observations.pending.is_empty());
        entry.hook_target = None;
        entry.test_parent = Some(Uuid::new_v4());
        ExtensionRuntime::collect_started_entry(id, entry, observations);
        assert!(observations.pending.is_empty());
        entry.test_parent = None;
        ExtensionRuntime::collect_started_entry(id, entry, observations);
        ExtensionRuntime::collect_started_entry(id, entry, observations);
        entries.remove(&id);
    }
    f.runtime.drain_observations(&mut journal);
    let events = journal.drain();
    assert_eq!(events.len(), 2);
    assert!(
        matches!(events[0].event, LifecycleEvent::ExtensionStarted { installation_id, process_generation:1, .. } if installation_id == id)
    );
    f.runtime.drain_observations(&mut journal);
    assert!(journal.drain().is_empty());
}
#[test]
fn started_failed_os_spawn_has_no_source_and_persistent_regrant_refreshes_epoch() {
    let f = Fixture::new();
    let id = Uuid::new_v4();
    f.start(id, true);
    f.wait_terminal(id);
    let mut journal = LifecycleJournal::default();
    f.runtime.drain_observations(&mut journal);
    assert!(journal.drain().is_empty());
    let old_epoch = f.runtime.inner().entries[&id].subscribed_at;
    f.runtime.inner().last_started_sequence = 17;
    f.runtime.set_grants(id, []);
    f.runtime.set_grants(id, [Capability::LifecycleObserve]);
    let inner = f.runtime.inner();
    assert!(inner.entries[&id].subscribed_at >= old_epoch);
    assert_eq!(inner.entries[&id].started_sequence_floor, 17);
}

#[test]
fn stopped_real_normal_nonzero_and_fast_exit_ignore_late_host_requests() {
    use crate::extension_protocol::ExtensionStopReason;
    for (script, code) in [("pass", 0), ("import sys; sys.exit(7)", 7)] {
        for reason in [
            ExtensionStopReason::Disabled,
            ExtensionStopReason::Removed,
            ExtensionStopReason::Restarted,
        ] {
            let f = Fixture::new();
            let id = Uuid::new_v4();
            f.start_script(id, false, script);
            f.wait_terminal(id);
            // No source drain before this late request: retained receipt wins.
            match reason {
                ExtensionStopReason::Disabled => f.runtime.stop(id),
                ExtensionStopReason::Removed => f.runtime.remove(id),
                _ => f.runtime.restart(id),
            }
            let mut journal = LifecycleJournal::default();
            f.runtime.drain_observations(&mut journal);
            let events = journal.drain();
            assert_eq!(events.len(), 2);
            assert!(
                matches!(events[1].event, LifecycleEvent::ExtensionStopped { installation_id, exit_code:Some(actual), requested_reason:None, .. } if installation_id == id && actual == code)
            );
            f.runtime.drain_observations(&mut journal);
            assert!(journal.drain().is_empty());
        }
    }
}

#[test]
fn stopping_first_request_correlates_incarnation_and_shutdown_is_sticky() {
    use crate::extension_protocol::ExtensionStopReason;
    for reason in [
        ExtensionStopReason::Disabled,
        ExtensionStopReason::Removed,
        ExtensionStopReason::Restarted,
        ExtensionStopReason::HostShutdown,
        ExtensionStopReason::Reconfigured,
    ] {
        let f = Fixture::new();
        let id = Uuid::new_v4();
        f.start_script(id, false, "import time; time.sleep(30)");
        let deadline = Instant::now() + Duration::from_secs(3);
        while f.runtime.inner().entries[&id]
            .connection
            .spawn_evidence()
            .is_none()
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let session = f.runtime.inner().entries[&id].connection.session_id();
        match reason {
            ExtensionStopReason::Disabled => f.runtime.stop(id),
            ExtensionStopReason::Removed => f.runtime.remove(id),
            ExtensionStopReason::Restarted => f.runtime.restart(id),
            ExtensionStopReason::Reconfigured => f.runtime.remove_with_reason(id, reason),
            ExtensionStopReason::HostShutdown => {
                f.runtime.begin_shutdown(
                    &mut AnimationState::default(),
                    &mut BehaviorController::default(),
                );
                assert!(f.runtime.shutdown(Duration::from_secs(2)));
            }
        }
        // Repeated/no-op commands may not replace the first requested reason.
        f.runtime.stop(id);
        f.runtime.remove(id);
        f.wait_terminal(id);
        let mut journal = LifecycleJournal::default();
        f.runtime.drain_observations(&mut journal);
        let events = journal.drain();
        assert_eq!(events.len(), 3, "{:?}", events);
        assert!(
            matches!(events[1].event, LifecycleEvent::ExtensionStopping { installation_id, session_id, process_generation:1, reason:actual } if installation_id==id && session_id==session && actual==reason)
        );
        assert!(
            matches!(events[2].event, LifecycleEvent::ExtensionStopped { installation_id, session_id, process_generation:1, requested_reason:Some(actual), .. } if installation_id==id && session_id==session && actual==reason)
        );
        f.runtime.drain_observations(&mut journal);
        assert!(journal.drain().is_empty());
    }
}

#[test]
fn stopped_missing_retained_receipt_reports_gap_once_never_fabricates_exit() {
    let f = Fixture::new();
    let id = Uuid::new_v4();
    f.start(id, false);
    f.wait_terminal(id);
    f.runtime.inner().entries[&id]
        .connection
        .discard_terminal_for_test();
    f.runtime.stop(id);
    let mut journal = LifecycleJournal::default();
    f.runtime.drain_observations(&mut journal);
    let events = journal.drain();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].event,
        LifecycleEvent::ExtensionStarted { .. }
    ));
    assert!(matches!(
        events[1].event,
        LifecycleEvent::Gap {
            dropped_records: 1,
            ..
        }
    ));
    f.runtime.drain_observations(&mut journal);
    assert!(journal.drain().is_empty());
}

#[test]
fn early_cancellation_reports_only_real_spawned_incarnations() {
    for _ in 0..4 {
        let f = Fixture::new();
        let id = Uuid::new_v4();
        f.start_script(id, false, "import time; time.sleep(30)");
        f.runtime.stop(id);
        f.wait_terminal(id);
        let spawned = f.runtime.inner().entries[&id]
            .connection
            .spawn_evidence()
            .is_some();
        let mut journal = LifecycleJournal::default();
        f.runtime.drain_observations(&mut journal);
        let events = journal.drain();
        if !spawned {
            assert!(
                events.is_empty(),
                "cancellation before spawn invented incarnation"
            );
        } else {
            assert!(
                matches!(events.first().unwrap().event, LifecycleEvent::ExtensionStarted { installation_id, .. } if installation_id == id)
            );
            assert!(
                matches!(events.last().unwrap().event, LifecycleEvent::ExtensionStopped { installation_id, .. } if installation_id == id)
            );
        }
    }
}
