//! Reconcile accepted installed revisions with supervised processes.
//! Failures stay visible until an explicit configuration change or restart;
//! ordinary frames never create an automatic crash/retry loop.
use crate::{
    extension_hooks::{HookProcess, HookTrigger},
    extension_process::ProcessSpec,
    extension_protocol::{Capability, IntegrationOrigin},
    extension_runtime::ExtensionRuntime,
    extensions::PackView,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq)]
struct Accepted {
    settings: crate::extension_settings::Settings,
    revision: Uuid,
    credential_revision: Option<Uuid>,
    grants: BTreeSet<Capability>,
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        animation::AnimationState,
        behavior::BehaviorController,
        extension_actions::{ActionCatalog, ActionControllers},
        extension_store::ExtensionStore,
    };
    use std::{
        fs,
        time::{Duration, Instant},
    };

    #[test]
    fn capacity_deferral_starts_fifth_installation_after_disabled_process_is_reaped() {
        struct Cleanup {
            runtime: ExtensionRuntime,
            root: std::path::PathBuf,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                // Also clean up real children when a regression assertion panics.
                let _ = self.runtime.shutdown(Duration::from_secs(2));
                let _ = fs::remove_dir_all(&self.root);
            }
        }
        let root =
            std::env::temp_dir().join(format!("waifu-installed-capacity-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let cleanup = Cleanup {
            runtime: ExtensionRuntime::default(),
            root: root.clone(),
        };
        let runtime = &cleanup.runtime;
        let mut store = ExtensionStore::open(root.join("store")).unwrap();
        let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/extensions/lifecycle-observer");
        let template: serde_json::Value =
            serde_json::from_slice(&fs::read(example.join("extension.json")).unwrap()).unwrap();
        let mut ids = Vec::new();
        let mut receipts = Vec::new();
        for number in 0..5 {
            let source = root.join(format!("source-{number}"));
            fs::create_dir(&source).unwrap();
            let mut manifest = template.clone();
            manifest["id"] = serde_json::json!(format!("studio.example.capacity-{number}"));
            manifest["name"] = serde_json::json!(format!("Capacity fixture {number}"));
            fs::write(
                source.join("extension.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::copy(example.join("observer.py"), source.join("observer.py")).unwrap();
            // The source must be executable; installation must preserve that bit.
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                source.join("observer.py"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            let id = store.install_integration(&source).unwrap();
            store
                .set_grants(id, vec![Capability::LifecycleObserve])
                .unwrap();
            if number < 4 {
                store.set_enabled(id, true).unwrap();
            }
            let installed = store
                .packs()
                .iter()
                .find(|p| p.installation_id == id)
                .unwrap();
            receipts.push(
                store
                    .data_directory(installed)
                    .unwrap()
                    .join("latest-event.json"),
            );
            ids.push(id);
        }
        let views = |store: &ExtensionStore| -> Vec<PackView> {
            store
                .packs()
                .iter()
                .map(|installed| PackView {
                    credentials: Default::default(),
                    content: Some(store.resolved(installed).unwrap()),
                    data_directory: Some(store.data_directory(installed).unwrap()),
                    installed: installed.clone(),
                    error: None,
                })
                .collect()
        };
        let mut binding = InstalledRuntime::default();
        let catalog = ActionCatalog::from_packs([], &[]);
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        binding.reconcile(&views(&store), runtime);
        assert!(binding.errors.is_empty(), "{:?}", binding.errors);
        assert_eq!(runtime.views().len(), 4);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !receipts[..4].iter().all(|path| path.exists()) {
            runtime.frame(
                &[],
                None,
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
            assert!(
                Instant::now() < deadline,
                "initial children not ready: {:?}",
                runtime.views()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(runtime.views().iter().all(|view| view.active));
        assert!(!receipts[4].exists());

        // Apply both durable changes before a single reconciliation. Removal
        // signals stop, but only runtime.frame can reap the manager's entry.
        store.set_enabled(ids[0], false).unwrap();
        store.set_enabled(ids[4], true).unwrap();
        let desired = views(&store);
        binding.reconcile(&desired, runtime);
        assert_eq!(runtime.views().len(), 4);
        assert!(runtime.views().iter().any(|view| view.id == ids[0]));
        assert!(!runtime.views().iter().any(|view| view.id == ids[4]));
        assert!(
            !binding.attempted.contains_key(&ids[4]),
            "capacity wait must not consume an executable attempt"
        );
        assert!(!binding.accepted.contains_key(&ids[4]));
        assert!(
            binding
                .errors
                .get(&ids[4])
                .is_some_and(|error| error.contains("Waiting"))
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.frame(
                &[],
                None,
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
            binding.reconcile(&desired, runtime);
            let running = runtime.views();
            assert!(running.len() <= 4, "process capacity exceeded: {running:?}");
            if receipts[4].exists() && running.iter().any(|view| view.id == ids[4] && view.active) {
                assert!(!running.iter().any(|view| view.id == ids[0]));
                assert_eq!(running.len(), 4);
                assert!(running.iter().all(|view| view.active));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "replacement never started: {running:?}, errors: {:?}",
                binding.errors
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(binding.errors.is_empty(), "{:?}", binding.errors);
        assert!(binding.accepted.contains_key(&ids[4]));
        assert!(binding.attempted.contains_key(&ids[4]));
        assert!(runtime.shutdown(Duration::from_secs(2)));
    }

    #[test]
    fn installed_example_stays_off_until_enabled_and_restores_after_reopen() {
        let root = std::env::temp_dir().join(format!("waifu-installed-{}", Uuid::new_v4()));
        let mut store = ExtensionStore::open(root.clone()).unwrap();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/extensions/lifecycle-observer");
        assert!(store.install(&source).is_err());
        let id = store.install_integration(&source).unwrap();
        let view = |store: &ExtensionStore| {
            let installed = store.packs()[0].clone();
            PackView {
                credentials: Default::default(),
                content: Some(store.resolved(&installed).unwrap()),
                data_directory: Some(store.data_directory(&installed).unwrap()),
                installed,
                error: None,
            }
        };
        let runtime = ExtensionRuntime::default();
        let mut binding = InstalledRuntime::default();
        binding.reconcile(&[view(&store)], &runtime);
        assert!(runtime.is_empty());
        store
            .set_grants(id, vec![Capability::LifecycleObserve])
            .unwrap();
        store.set_enabled(id, true).unwrap();
        drop(store);
        let mut store = ExtensionStore::open(root.clone()).unwrap();
        let pack = view(&store);
        let receipt = pack
            .data_directory
            .as_ref()
            .unwrap()
            .join("latest-event.json");
        binding.reconcile(&[pack], &runtime);
        assert!(binding.errors.is_empty(), "{:?}", binding.errors);
        assert_eq!(runtime.views().len(), 1);
        let catalog = ActionCatalog::from_packs([], &[]);
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !receipt.exists() {
            runtime.frame(
                &[],
                None,
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
            assert!(Instant::now() < deadline, "{:?}", runtime.views());
            std::thread::sleep(Duration::from_millis(10));
        }
        // Durable disabling removes the process, rather than leaving a second
        // runtime-only enable switch that disagrees with the installed index.
        store.set_enabled(id, false).unwrap();
        binding.reconcile(&[view(&store)], &runtime);
        while !runtime.is_empty() {
            runtime.frame(
                &[],
                None,
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(runtime.shutdown(Duration::from_secs(1)));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    struct SettingsChild {
        runtime: ExtensionRuntime,
        binding: InstalledRuntime,
        root: std::path::PathBuf,
        catalog: ActionCatalog,
        animation: AnimationState,
        behavior: BehaviorController,
    }

    impl SettingsChild {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("waifu-installed-settings-{}", Uuid::new_v4()));
            let source = root.join("source");
            fs::create_dir_all(&source).unwrap();
            // Keep this fixture independent of the user-facing observer example.
            // Each child records the actual handshake it received, then remains
            // alive until the host stops it. Atomic replacement avoids partial
            // receipts, and the history cap also catches runaway replacement.
            let script = r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
handshake = json.loads(sys.stdin.readline())
assert handshake['message']['type'] == 'handshake'
data = Path(os.environ['WAIFU_EXTENSION_DATA'])
history_file = data / 'startups.json'
history = json.loads(history_file.read_text()) if history_file.exists() else []
assert len(history) < 8, 'unexpected restart loop'
history.append({
    'handshake': handshake,
    'session_id': handshake['session_id'],
    'process_generation': handshake['process_generation'],
    'settings': handshake['message']['settings'],
})
temporary = data / 'startups.tmp'
temporary.write_text(json.dumps(history))
temporary.replace(history_file)
for line in sys.stdin:
    json.loads(line)
"#;
            let program = source.join("settings.py");
            fs::write(&program, script).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            let manifest = serde_json::json!({
                "api_version": 1,
                "id": "studio.example.settings-fixture",
                "version": "1.0.0",
                "name": "Installed settings fixture",
                "description": "Records startup settings without executing host actions",
                "provenance": {"author": "Fixture", "license": "MIT"},
                "process": {"program": "settings.py"},
                "settings": [
                    {"id": "record_events", "label": "Record events", "type": "toggle", "default": true},
                    {"id": "limit", "label": "Limit", "type": "integer", "default": 3, "min": 0, "max": 20},
                    {"id": "label", "label": "Receipt label", "type": "text", "default": "default label", "max_length": 64},
                    {"id": "group", "label": "Event group", "type": "choice", "default": "all", "options": ["all", "speech"]}
                ]
            });
            fs::write(
                source.join("extension.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            Self {
                runtime: ExtensionRuntime::default(),
                binding: InstalledRuntime::default(),
                root,
                catalog: ActionCatalog::from_packs([], &[]),
                animation: AnimationState::default(),
                behavior: BehaviorController::default(),
            }
        }

        fn view(store: &ExtensionStore) -> PackView {
            let installed = store.packs()[0].clone();
            let content = store.resolved(&installed).unwrap();
            let data = store.data_directory(&installed).unwrap();
            let credentials =
                crate::extension_credentials::status(&data, &content.manifest.credentials).unwrap();
            PackView {
                credentials,
                content: Some(content),
                data_directory: Some(data),
                installed,
                error: None,
            }
        }

        fn startups(data: &std::path::Path) -> Vec<serde_json::Value> {
            match fs::read(data.join("startups.json")) {
                Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => panic!("cannot read child startup receipts: {error}"),
            }
        }

        fn frame(&mut self, packs: &[PackView]) {
            self.binding.reconcile(packs, &self.runtime);
            assert!(self.binding.errors.is_empty(), "{:?}", self.binding.errors);
            self.runtime.frame(
                &[],
                None,
                false,
                &self.catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut self.animation,
                    behavior: &mut self.behavior,
                },
            );
            assert!(
                self.runtime.views().len() <= 1,
                "replacement overlapped old process"
            );
        }

        fn until(&mut self, packs: &[PackView], mut ready: impl FnMut(&Self) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                self.frame(packs);
                if ready(self) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "settings child timed out: {:?}",
                    self.runtime.views()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn unchanged_frames(&mut self, packs: &[PackView]) {
            for _ in 0..16 {
                self.frame(packs);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(self.runtime.views().iter().all(|view| view.active));
        }
    }

    impl Drop for SettingsChild {
        fn drop(&mut self) {
            let _ = self.runtime.shutdown(Duration::from_secs(2));
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn installed_hooks_wait_for_enable_and_reconfigure_durable_settings_revision_and_grants() {
        let mut child = SettingsChild::new();
        let source = child.root.join("source");
        let manifest_path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let handler = manifest.as_object_mut().unwrap().remove("process").unwrap();
        manifest["hooks"] = serde_json::json!([
            {"event":"reply.finished", "handler":handler},
            {"event":"speech.started", "handler":handler},
            {"event":"speech.stopped", "handler":handler}
        ]);
        manifest["capabilities"] = serde_json::json!(["lifecycle.observe"]);
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut store = ExtensionStore::open(child.root.join("store")).unwrap();
        let id = store.install_integration(&source).unwrap();
        let disabled = SettingsChild::view(&store);
        let data = disabled.data_directory.clone().unwrap();
        child.frame(&[disabled]);
        assert!(child.runtime.is_empty());
        store
            .set_grants(id, vec![Capability::LifecycleObserve])
            .unwrap();
        store.set_enabled(id, true).unwrap();
        child.frame(&[SettingsChild::view(&store)]);
        assert_eq!(child.runtime.views().len(), 1);
        assert!(child.runtime.views()[0].enabled);
        assert_eq!(child.runtime.process_count(), 0);
        assert!(SettingsChild::startups(&data).is_empty());

        store
            .set_settings(
                id,
                serde_json::from_value(serde_json::json!({"label":"hook settings"})).unwrap(),
            )
            .unwrap();
        let changed = SettingsChild::view(&store);
        child.until(std::slice::from_ref(&changed), |child| {
            child
                .binding
                .accepted
                .get(&id)
                .is_some_and(|accepted| accepted.settings == changed.installed.settings)
                && child.runtime.views().len() == 1
        });
        assert_eq!(
            child.binding.accepted[&id].grants,
            BTreeSet::from([Capability::LifecycleObserve])
        );
        assert_eq!(child.runtime.process_count(), 0);
        store.set_grants(id, vec![]).unwrap();
        child.frame(&[SettingsChild::view(&store)]);
        assert!(child.binding.accepted[&id].grants.is_empty());

        manifest["version"] = serde_json::json!("2.0.0");
        manifest["hooks"].as_array_mut().unwrap().pop();
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        store.install_integration(&source).unwrap();
        let updated = SettingsChild::view(&store);
        assert_ne!(updated.installed.revision, changed.installed.revision);
        assert_eq!(updated.data_directory.as_ref(), Some(&data));
        assert_eq!(updated.installed.settings, changed.installed.settings);
        child.until(std::slice::from_ref(&updated), |child| {
            child
                .binding
                .accepted
                .get(&id)
                .is_some_and(|accepted| accepted.revision == updated.installed.revision)
        });
        assert_eq!(child.runtime.process_count(), 0);
        assert!(SettingsChild::startups(&data).is_empty());
        store.set_enabled(id, false).unwrap();
        child.until(&[SettingsChild::view(&store)], |child| {
            child.runtime.is_empty()
        });
    }

    #[test]
    fn idle_installed_hook_rows_do_not_consume_persistent_process_slots() {
        let mut child = SettingsChild::new();
        let source = child.root.join("source");
        let template: serde_json::Value =
            serde_json::from_slice(&fs::read(source.join("extension.json")).unwrap()).unwrap();
        let mut store = ExtensionStore::open(child.root.join("store")).unwrap();
        for number in 0..4 {
            let hook_source = child.root.join(format!("hook-source-{number}"));
            fs::create_dir(&hook_source).unwrap();
            fs::copy(source.join("settings.py"), hook_source.join("settings.py")).unwrap();
            let mut manifest = template.clone();
            manifest["id"] = serde_json::json!(format!("studio.example.idle-hook-{number}"));
            let handler = manifest.as_object_mut().unwrap().remove("process").unwrap();
            manifest["hooks"] = serde_json::json!([{"event":"reply.finished", "handler":handler}]);
            fs::write(
                hook_source.join("extension.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            let id = store.install_integration(&hook_source).unwrap();
            store.set_enabled(id, true).unwrap();
        }
        let views = |store: &ExtensionStore| -> Vec<PackView> {
            store
                .packs()
                .iter()
                .map(|installed| PackView {
                    credentials: Default::default(),
                    installed: installed.clone(),
                    content: Some(store.resolved(installed).unwrap()),
                    data_directory: Some(store.data_directory(installed).unwrap()),
                    error: None,
                })
                .collect()
        };
        child.binding.reconcile(&views(&store), &child.runtime);
        assert!(
            child.binding.errors.is_empty(),
            "{:?}",
            child.binding.errors
        );
        assert_eq!(child.runtime.views().len(), 4);
        assert_eq!(child.runtime.process_count(), 0);
        let id = store.install_integration(&source).unwrap();
        store.set_enabled(id, true).unwrap();
        let installed = store
            .packs()
            .iter()
            .find(|pack| pack.installation_id == id)
            .unwrap();
        let data = store.data_directory(installed).unwrap();
        child.binding.reconcile(&views(&store), &child.runtime);
        assert!(
            child.binding.errors.is_empty(),
            "{:?}",
            child.binding.errors
        );
        assert_eq!(child.runtime.views().len(), 5);
        assert_eq!(child.runtime.process_count(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while SettingsChild::startups(&data).is_empty() {
            child.runtime.frame(
                &[],
                None,
                false,
                &child.catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut child.animation,
                    behavior: &mut child.behavior,
                },
            );
            assert!(
                Instant::now() < deadline,
                "persistent child did not start after idle hook registrations"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(SettingsChild::startups(&data).len(), 1);
        assert_eq!(child.runtime.process_count(), 1);
    }

    #[test]
    fn credential_save_and_clear_replace_once_after_reaping_without_handshake_secrets() {
        let mut child = SettingsChild::new();
        let source = child.root.join("source");
        let path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["credentials"] = serde_json::json!([{"id":"token", "label":"Access token"}]);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut store = ExtensionStore::open(child.root.join("store")).unwrap();
        let id = store.install_integration(&source).unwrap();
        store.set_enabled(id, true).unwrap();
        let first = SettingsChild::view(&store);
        let data = first.data_directory.clone().unwrap();
        child.until(std::slice::from_ref(&first), |_| {
            SettingsChild::startups(&data).len() == 1
        });
        let installed = first.installed.clone();
        let original_session = SettingsChild::startups(&data)[0]["session_id"].clone();
        let updates = BTreeMap::from([("token".into(), Some("private-binder-fixture".into()))]);
        store.set_credentials(id, updates.clone()).unwrap();
        let configured = SettingsChild::view(&store);
        assert_ne!(configured.credentials.revision, first.credentials.revision);
        assert_eq!(configured.installed, installed);
        child
            .binding
            .reconcile(std::slice::from_ref(&configured), &child.runtime);
        assert_eq!(
            child.runtime.views().len(),
            1,
            "old owner must remain until reaped"
        );
        assert_eq!(SettingsChild::startups(&data).len(), 1);
        child.until(std::slice::from_ref(&configured), |child| {
            SettingsChild::startups(&data).len() == 2
                && child.runtime.views().iter().any(|view| view.active)
        });
        assert_ne!(
            SettingsChild::startups(&data)[1]["session_id"],
            original_session
        );
        store.set_credentials(id, updates).unwrap();
        let unchanged = SettingsChild::view(&store);
        assert_eq!(unchanged.credentials, configured.credentials);
        child.unchanged_frames(std::slice::from_ref(&unchanged));
        assert_eq!(
            SettingsChild::startups(&data).len(),
            2,
            "no-op save restarted process"
        );
        store
            .set_credentials(id, BTreeMap::from([("token".into(), None)]))
            .unwrap();
        let cleared = SettingsChild::view(&store);
        assert_ne!(
            cleared.credentials.revision,
            configured.credentials.revision
        );
        assert!(cleared.credentials.configured.is_empty());
        assert_eq!(cleared.installed, installed);
        child.until(std::slice::from_ref(&cleared), |child| {
            SettingsChild::startups(&data).len() == 3
                && child.runtime.views().iter().any(|view| view.active)
        });
        child.unchanged_frames(std::slice::from_ref(&cleared));
        let startups = SettingsChild::startups(&data);
        assert_eq!(startups.len(), 3);
        assert_ne!(startups[1]["session_id"], startups[2]["session_id"]);
        assert!(
            !serde_json::to_string(&startups)
                .unwrap()
                .contains("private-binder-fixture")
        );
    }

    #[test]
    fn settings_reverted_before_reaping_start_once_with_original_values_and_fresh_session() {
        let mut child = SettingsChild::new();
        let mut store = ExtensionStore::open(child.root.join("store")).unwrap();
        let id = store
            .install_integration(&child.root.join("source"))
            .unwrap();
        let original = serde_json::json!({
            "record_events": false, "limit": 7, "label": "original A", "group": "speech"
        });
        store
            .set_settings(id, serde_json::from_value(original.clone()).unwrap())
            .unwrap();
        store.set_enabled(id, true).unwrap();
        let first = SettingsChild::view(&store);
        let data = first.data_directory.clone().unwrap();
        child.until(std::slice::from_ref(&first), |child| {
            SettingsChild::startups(&data).len() == 1
                && child.runtime.views().iter().any(|view| view.active)
        });
        let initial = SettingsChild::startups(&data);
        assert_eq!(initial[0]["settings"], original);
        let first_session: Uuid = serde_json::from_value(initial[0]["session_id"].clone()).unwrap();

        store
            .set_settings(
                id,
                serde_json::from_value(serde_json::json!({
                    "record_events": true, "limit": 11, "label": "temporary B", "group": "all"
                }))
                .unwrap(),
            )
            .unwrap();
        let changed = SettingsChild::view(&store);
        // Reconcile B and then A without runtime frames: removal has signalled
        // shutdown, but the original manager entry cannot have been reaped.
        child
            .binding
            .reconcile(std::slice::from_ref(&changed), &child.runtime);
        assert_eq!(child.runtime.views().len(), 1);
        store
            .set_settings(id, serde_json::from_value(original.clone()).unwrap())
            .unwrap();
        let reverted = SettingsChild::view(&store);
        child
            .binding
            .reconcile(std::slice::from_ref(&reverted), &child.runtime);
        assert_eq!(child.runtime.views().len(), 1);
        assert_eq!(SettingsChild::startups(&data).len(), 1);

        child.until(std::slice::from_ref(&reverted), |child| {
            SettingsChild::startups(&data).len() == 2
                && child.runtime.views().iter().any(|view| view.active)
        });
        child.unchanged_frames(std::slice::from_ref(&reverted));
        let replacement = SettingsChild::startups(&data);
        assert_eq!(
            replacement.len(),
            2,
            "settings reversal restarted more than once"
        );
        assert_eq!(replacement[1]["settings"], original);
        let second_session: Uuid =
            serde_json::from_value(replacement[1]["session_id"].clone()).unwrap();
        assert!(!second_session.is_nil());
        assert_ne!(first_session, second_session);
    }

    #[test]
    fn saved_typed_settings_replace_once_and_survive_manual_restart_and_disabled_reopen() {
        use crate::extension_settings::Settings;
        let mut child = SettingsChild::new();
        let store_root = child.root.join("store");
        let mut store = ExtensionStore::open(store_root.clone()).unwrap();
        let id = store
            .install_integration(&child.root.join("source"))
            .unwrap();
        let original = serde_json::json!({
            "record_events": false, "limit": 7, "label": "saved 雪", "group": "speech"
        });
        let original_settings: Settings = serde_json::from_value(original.clone()).unwrap();
        store.set_settings(id, original_settings.clone()).unwrap();
        assert!(!store.packs()[0].enabled);
        child.frame(&[SettingsChild::view(&store)]);
        assert!(child.runtime.is_empty());
        store.set_enabled(id, true).unwrap();
        drop(store);

        // Read saved configuration back from disk before launching any child.
        let mut store = ExtensionStore::open(store_root.clone()).unwrap();
        let first = SettingsChild::view(&store);
        assert_eq!(first.installed.settings, original_settings);
        let revision = first.installed.revision;
        let data = first.data_directory.clone().unwrap();
        child.until(std::slice::from_ref(&first), |child| {
            SettingsChild::startups(&data).len() == 1
                && child.runtime.views().iter().any(|view| view.active)
        });
        let initial = SettingsChild::startups(&data);
        assert_eq!(initial[0]["settings"], original);
        let first_session: Uuid = serde_json::from_value(initial[0]["session_id"].clone()).unwrap();

        let changed = serde_json::json!({
            "record_events": true, "limit": 11, "label": "updated 🦊", "group": "all"
        });
        let changed_settings: Settings = serde_json::from_value(changed.clone()).unwrap();
        store.set_settings(id, changed_settings.clone()).unwrap();
        let second = SettingsChild::view(&store);
        assert_eq!(
            second.installed.revision, revision,
            "settings must not create a package revision"
        );
        assert!(second.installed.enabled);
        assert_eq!(second.installed.settings, changed_settings);
        child.until(std::slice::from_ref(&second), |child| {
            SettingsChild::startups(&data).len() == 2
                && child.runtime.views().iter().any(|view| view.active)
        });
        let replacement = SettingsChild::startups(&data);
        assert_eq!(replacement[1]["settings"], changed);
        let second_session: Uuid =
            serde_json::from_value(replacement[1]["session_id"].clone()).unwrap();
        assert_ne!(first_session, second_session);

        // Equivalent saves, reconciliation and refreshes must not restart code.
        store.set_settings(id, changed_settings.clone()).unwrap();
        drop(store);
        let mut store = ExtensionStore::open(store_root.clone()).unwrap();
        let same = SettingsChild::view(&store);
        child.unchanged_frames(std::slice::from_ref(&same));
        assert_eq!(SettingsChild::startups(&data).len(), 2);

        child.runtime.restart(id);
        child.until(std::slice::from_ref(&same), |child| {
            SettingsChild::startups(&data).len() == 3
                && child.runtime.views().iter().any(|view| view.active)
        });
        let restarted = SettingsChild::startups(&data);
        assert_eq!(restarted[2]["settings"], changed);
        let third_session: Uuid =
            serde_json::from_value(restarted[2]["session_id"].clone()).unwrap();
        assert_ne!(third_session, second_session);
        assert_ne!(third_session, first_session);
        assert!(
            restarted[2]["process_generation"].as_u64().unwrap()
                > restarted[1]["process_generation"].as_u64().unwrap()
        );
        child.unchanged_frames(std::slice::from_ref(&same));
        assert_eq!(SettingsChild::startups(&data).len(), 3);

        store.set_enabled(id, false).unwrap();
        let disabled = SettingsChild::view(&store);
        child.until(std::slice::from_ref(&disabled), |child| {
            child.runtime.is_empty()
        });
        drop(store);
        let store = ExtensionStore::open(store_root).unwrap();
        let disabled = SettingsChild::view(&store);
        assert!(!disabled.installed.enabled);
        assert_eq!(disabled.installed.settings, changed_settings);
        child.binding = InstalledRuntime::default();
        child.unchanged_frames(&[disabled]);
        assert!(child.runtime.is_empty());
        assert_eq!(
            SettingsChild::startups(&data).len(),
            3,
            "disabled saved install spawned a child"
        );
    }
    #[test]
    fn installed_disable_uninstall_and_settings_replacement_keep_distinct_stop_reasons() {
        use crate::extension_lifecycle::{ExtensionStopReason, LifecycleEvent, LifecycleJournal};
        for reason in [
            ExtensionStopReason::Disabled,
            ExtensionStopReason::Removed,
            ExtensionStopReason::Reconfigured,
        ] {
            let mut child = SettingsChild::new();
            let mut store = ExtensionStore::open(child.root.join("store")).unwrap();
            let id = store
                .install_integration(&child.root.join("source"))
                .unwrap();
            store.set_enabled(id, true).unwrap();
            let first = SettingsChild::view(&store);
            let data = first.data_directory.clone().unwrap();
            child.until(std::slice::from_ref(&first), |_| {
                !SettingsChild::startups(&data).is_empty()
            });
            let mut journal = LifecycleJournal::default();
            child.runtime.drain_observations(&mut journal);
            journal.drain();
            let desired = match reason {
                ExtensionStopReason::Disabled => {
                    store.set_enabled(id, false).unwrap();
                    vec![SettingsChild::view(&store)]
                }
                ExtensionStopReason::Removed => vec![],
                _ => {
                    store.set_settings(id,serde_json::from_value(serde_json::json!({"record_events":true,"limit":7,"label":"replacement","group":"speech"})).unwrap()).unwrap();
                    vec![SettingsChild::view(&store)]
                }
            };
            child.binding.reconcile(&desired, &child.runtime);
            child.runtime.drain_observations(&mut journal);
            let events = journal.drain();
            assert_eq!(events.iter().filter(|e| matches!(e.event,LifecycleEvent::ExtensionStopping { reason:actual,.. } if actual==reason)).count(),1,"{:?}",events);
        }
    }
}

#[derive(Default)]
pub struct InstalledRuntime {
    accepted: BTreeMap<Uuid, Accepted>,
    attempted: BTreeMap<Uuid, Accepted>,
    pub errors: BTreeMap<Uuid, String>,
}

impl InstalledRuntime {
    pub fn reconcile(&mut self, packs: &[PackView], runtime: &ExtensionRuntime) {
        let desired: BTreeMap<_, _> = packs
            .iter()
            .filter(|p| p.installed.executable && p.installed.enabled && p.error.is_none())
            .filter(|p| p.content.is_some())
            .map(|p| (p.installed.installation_id, p))
            .collect();
        self.attempted.retain(|id, _| desired.contains_key(id));
        self.errors.retain(|id, _| desired.contains_key(id));
        let views = runtime.views();
        let attempted = &mut self.attempted;
        self.accepted.retain(|id, old| {
            let keep = desired.get(id).is_some_and(|p| {
                p.installed.revision == old.revision
                    && p.installed.settings == old.settings
                    && p.credentials.revision == old.credential_revision
            });
            if !keep {
                // A successful configuration can be chosen again while its
                // former process is still terminating (A -> B -> A).
                attempted.remove(id);
                let reason = match packs
                    .iter()
                    .find(|pack| pack.installed.installation_id == *id)
                {
                    None => crate::extension_protocol::ExtensionStopReason::Removed,
                    Some(pack) if !pack.installed.enabled => {
                        crate::extension_protocol::ExtensionStopReason::Disabled
                    }
                    Some(_) => crate::extension_protocol::ExtensionStopReason::Reconfigured,
                };
                runtime.remove_with_reason(*id, reason);
            }
            keep
        });
        for (id, pack) in desired {
            let key = Accepted {
                settings: pack.installed.settings.clone(),
                revision: pack.installed.revision,
                credential_revision: pack.credentials.revision,
                grants: pack.installed.grants.iter().copied().collect(),
            };
            if let Some(old) = self.accepted.get_mut(&id) {
                if old.grants != key.grants {
                    runtime.set_grants(id, key.grants.iter().copied());
                    old.grants = key.grants.clone();
                }
                continue;
            }
            // Removed revisions must finish terminating before their ID is reused.
            if views.iter().any(|view| view.id == id) {
                continue;
            }
            if self.attempted.get(&id) == Some(&key) {
                continue;
            }
            let resolved = pack.content.as_ref().expect("filtered resolved pack");
            // Idle hook registrations do not reserve process slots. The runtime
            // enforces the separate registration bound and hook launch budget.
            if resolved.manifest.process.is_some() && runtime.process_count() >= 4 {
                self.errors.insert(
                    id,
                    "Waiting for a local integration process slot (maximum 4)".into(),
                );
                continue;
            }
            self.attempted.insert(id, key.clone());
            let result = (|| {
                let data_directory = pack
                    .data_directory
                    .clone()
                    .ok_or("Integration data directory unavailable")?;
                let spec =
                    |process: &crate::extension_manifest::ProcessMetadata, lifetime| ProcessSpec {
                        program: resolved.root.join(&process.program),
                        args: process.args.clone(),
                        working_directory: process
                            .working_directory
                            .as_ref()
                            .map_or_else(|| resolved.root.clone(), |path| resolved.root.join(path)),
                        data_directory: data_directory.clone(),
                        lifetime,
                    };
                let origin = IntegrationOrigin {
                    integration_id: resolved.manifest.id.clone(),
                    // Protocol display names have a smaller bound than pack titles.
                    display_name: resolved
                        .manifest
                        .name
                        .chars()
                        .scan(0usize, |bytes, ch| {
                            *bytes += ch.len_utf8();
                            (*bytes <= 128).then_some(ch)
                        })
                        .collect(),
                };
                if let Some(process) = &resolved.manifest.process {
                    if !resolved.manifest.hooks.is_empty() {
                        return Err(
                            "Integration cannot combine a persistent process and hooks".into()
                        );
                    }
                    runtime.start_configured(
                        id,
                        spec(process, None),
                        origin.clone(),
                        key.grants.iter().copied(),
                        key.settings.clone(),
                    )?;
                } else {
                    if resolved.manifest.hooks.is_empty() {
                        return Err("Integration has no process or hooks".into());
                    }
                    let hooks = resolved
                        .manifest
                        .hooks
                        .iter()
                        .map(|hook| {
                            let trigger: HookTrigger = serde_json::from_value(
                                serde_json::Value::String(hook.event.clone()),
                            )
                            .map_err(|_| format!("Unsupported hook event '{}'", hook.event))?;
                            Ok(HookProcess {
                                trigger,
                                process: spec(
                                    &hook.handler,
                                    Some(std::time::Duration::from_secs(10)),
                                ),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    runtime.configure_hooks(
                        id,
                        origin.clone(),
                        key.grants.iter().copied(),
                        key.settings.clone(),
                        hooks,
                    )?;
                }
                if let Some(test) = &resolved.manifest.test_process
                    && let Err(error) = runtime.configure_test(
                        id,
                        spec(test, Some(std::time::Duration::from_secs(10))),
                        origin,
                        key.grants.iter().copied(),
                        key.settings.clone(),
                    )
                {
                    runtime.remove_with_reason(
                        id,
                        crate::extension_protocol::ExtensionStopReason::Reconfigured,
                    );
                    return Err(error);
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    self.accepted.insert(id, key);
                    self.errors.remove(&id);
                }
                Err(error) => {
                    self.errors.insert(id, error);
                }
            }
        }
    }
}
