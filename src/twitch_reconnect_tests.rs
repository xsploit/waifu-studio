//! Offline preparation only: fake state channels and synthetic credential files.
//! Never construct ExtensionState::new or an auth job; no worker executes commands.
use super::*;
use std::fs;

struct Fixture {
    root: PathBuf,
    store: ExtensionStore,
    id: Uuid,
}
impl Fixture {
    fn new() -> Self {
        Self::package("studio.integration.twitch")
    }
    fn package(package: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("twitch-reconnect-offline-{}", Uuid::new_v4()));
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("run.py"),
            "#!/usr/bin/env python3\n# Never executed.\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(source.join("run.py"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(source.join("extension.json"),serde_json::to_vec(&serde_json::json!({
            "api_version":1,"id":package,"version":"1.0.0","name":"Offline reconnect fixture",
            "description":"Synthetic fixture, never executes auth or an integration",
            "provenance":{"author":"Fixture","license":"MIT"},"process":{"program":"run.py"},
            "credentials":[{"id":"client_id","label":"Application ID"},{"id":"access_token","label":"Access token"},{"id":"refresh_token","label":"Refresh token"}],
            "settings":[{"id":"events","label":"Events","type":"choice","options":["Chat and alerts","Alerts only","Chat only"],"default":"Chat and alerts"}]
        })).unwrap()).unwrap();
        let mut store = ExtensionStore::open(root.join("store")).unwrap();
        let id = store.install_integration(&source).unwrap();
        Self { root, store, id }
    }
    fn expected(&self) -> InstalledPack {
        self.store
            .packs()
            .iter()
            .find(|p| p.installation_id == self.id)
            .unwrap()
            .clone()
    }
    fn view(&self) -> PackView {
        let installed = self.expected();
        let content = self.store.resolved(&installed).unwrap();
        let data = self.store.data_directory(&installed).unwrap();
        let credentials =
            crate::extension_credentials::status(&data, &content.manifest.credentials).unwrap();
        PackView {
            installed,
            content: Some(content),
            data_directory: Some(data),
            credentials,
            error: None,
        }
    }
    fn revision(&self) -> Option<Uuid> {
        self.view().credentials.revision
    }
    fn save_client(&mut self, client: &str) {
        self.store
            .set_credentials(
                self.id,
                BTreeMap::from([("client_id".into(), Some(client.into()))]),
            )
            .unwrap();
    }
    fn inputs(&self, override_id: Option<&str>) -> Result<(String, Vec<String>), String> {
        device_auth_inputs(
            &self.store,
            &self.expected(),
            self.revision(),
            &AtomicU8::new(0),
            override_id.map(str::to_owned),
        )
    }
    fn events(&mut self, value: &str) {
        let mut settings = self.expected().settings;
        settings.insert(
            "events".into(),
            crate::extension_settings::SettingValue::Text(value.into()),
        );
        self.store.set_settings(self.id, settings).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Idle {
    state: ExtensionState,
    commands: Receiver<Command>,
    events: SyncSender<Outcome>,
}
impl Idle {
    fn new(pack: PackView) -> Self {
        let (sender, commands) = mpsc::sync_channel(1);
        let (events, receiver) = mpsc::sync_channel(1);
        Self {
            state: ExtensionState {
                packs: vec![pack],
                busy: false,
                status: String::new(),
                error: None,
                generation: 0,
                initial_load_completed: true,
                twitch_auth: None,
                oauth_schedule: BTreeMap::new(),
                refresh_seen: BTreeMap::new(),
                refresh_not_before: None,
                cancelled: Arc::new(AtomicBool::new(false)),
                sender,
                receiver: Mutex::new(receiver),
            },
            commands,
            events,
        }
    }
    fn prepare(&self, id: Uuid, client: Option<&str>) -> (Uuid, Arc<AtomicU8>) {
        match self
            .commands
            .try_recv()
            .expect("preparation was not queued")
        {
            Command::PrepareAuth {
                attempt,
                expected,
                credential_revision,
                gate,
                client_id,
            } => {
                assert_eq!(expected.installation_id, id);
                assert_eq!(*expected, self.state.packs[0].installed);
                assert_eq!(
                    credential_revision,
                    self.state.packs[0].credentials.revision
                );
                assert_eq!(client_id.as_deref(), client);
                assert_eq!(gate.load(Ordering::Acquire), 0);
                (attempt, gate)
            }
            _ => panic!("reconnect must queue preparation, not refresh/credential commit"),
        }
    }
}

#[test]
fn reconnect_saved_and_override_inputs_use_captured_scopes_without_persistence_changes() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let expected = f.expected();
    let revision = f.revision();
    let data = f.view().data_directory.unwrap();
    let bytes = fs::read(data.join("credentials.json")).unwrap();
    let (client, scopes) = f.inputs(None).unwrap();
    assert_eq!(client, "SyntheticSavedClient123");
    assert_eq!(
        scopes,
        crate::twitch_worker::required_scopes(&expected.settings)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        f.inputs(Some("SyntheticOverride456")).unwrap().0,
        "SyntheticOverride456"
    );
    assert_eq!(f.expected(), expected);
    assert_eq!(f.revision(), revision);
    assert_eq!(fs::read(data.join("credentials.json")).unwrap(), bytes);
    assert!(!expected.enabled);
    assert!(expected.grants.is_empty());
    f.events("Chat only");
    assert_eq!(f.inputs(None).unwrap().1, vec!["user:read:chat".to_owned()]);
}

#[test]
fn reconnect_missing_invalid_saved_and_invalid_explicit_inputs_never_fallback() {
    let mut f = Fixture::new();
    assert!(f.inputs(None).is_err());
    assert_eq!(
        f.inputs(Some("SyntheticOverride456")).unwrap().0,
        "SyntheticOverride456"
    );
    f.save_client("SyntheticSavedClient123");
    for invalid in ["", " ", "bad-client", "bad_client", "bad\nclient", "clïent"] {
        assert!(
            f.inputs(Some(invalid)).is_err(),
            "invalid explicit ID fell back"
        );
    }
    assert!(f.inputs(Some(&"x".repeat(129))).is_err());
    for invalid in ["bad-client", " ", "clïent"] {
        f.save_client(invalid);
        assert!(f.inputs(None).is_err());
        assert_eq!(
            f.inputs(Some("SyntheticOverride456")).unwrap().0,
            "SyntheticOverride456"
        );
    }
}

#[test]
fn reconnect_inputs_reject_stale_config_credentials_and_removed_installation() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let old = f.expected();
    let revision = f.revision();
    f.save_client("SyntheticRotatedClient789");
    for client in [None, Some("SyntheticOverride456".into())] {
        assert!(device_auth_inputs(&f.store, &old, revision, &AtomicU8::new(0), client).is_err());
    }
    let old = f.expected();
    let revision = f.revision();
    f.events("Alerts only");
    assert!(device_auth_inputs(&f.store, &old, revision, &AtomicU8::new(0), None).is_err());
    let old = f.expected();
    let revision = f.revision();
    f.store.set_enabled(f.id, true).unwrap();
    assert!(device_auth_inputs(&f.store, &old, revision, &AtomicU8::new(0), None).is_err());
    let old = f.expected();
    let revision = f.revision();
    f.store.remove(f.id).unwrap();
    assert!(device_auth_inputs(&f.store, &old, revision, &AtomicU8::new(0), None).is_err());
}

#[test]
fn reconnect_inputs_reject_wrong_package_and_closed_gates() {
    let mut other = Fixture::package("org.example.not-twitch");
    other.save_client("SyntheticSavedClient123");
    assert!(other.inputs(None).is_err());
    assert!(other.inputs(Some("SyntheticOverride456")).is_err());
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    for gate in [1, 2, 3, 4] {
        for client in [None, Some("SyntheticOverride456".into())] {
            assert!(
                device_auth_inputs(
                    &f.store,
                    &f.expected(),
                    f.revision(),
                    &AtomicU8::new(gate),
                    client
                )
                .is_err()
            );
        }
    }
}

#[test]
fn reconnect_public_admission_queues_saved_or_explicit_without_an_auth_job() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    for (public, queued) in [
        ("", None),
        ("SyntheticOverride456", Some("SyntheticOverride456")),
    ] {
        let mut idle = Idle::new(f.view());
        let expected = idle.state.packs[0].installed.clone();
        let attempt = idle.state.start_twitch_auth(f.id, public.into()).unwrap();
        let (queued_attempt, gate) = idle.prepare(f.id, queued);
        assert_eq!(attempt, queued_attempt);
        assert!(idle.state.busy);
        let auth = idle.state.twitch_auth.as_ref().unwrap();
        assert!(auth.preparing && auth.job.is_none() && auth.bundle.is_none());
        assert_eq!(auth.id, attempt);
        assert_eq!(auth.expected, expected);
        assert!(Arc::ptr_eq(&gate, &auth.gate));
        assert_eq!(idle.state.packs[0].installed, expected);
        assert!(idle.commands.try_recv().is_err());
    }
}

#[test]
fn reconnect_public_missing_saved_and_invalid_overrides_do_not_admit_preparation() {
    let mut f = Fixture::new();
    let mut idle = Idle::new(f.view());
    assert!(idle.state.start_twitch_auth(f.id, String::new()).is_err());
    assert!(idle.commands.try_recv().is_err());
    f.save_client("SyntheticSavedClient123");
    let mut idle = Idle::new(f.view());
    for invalid in [" ", "bad-client", "bad_client", "clïent"] {
        assert!(idle.state.start_twitch_auth(f.id, invalid.into()).is_err());
        assert!(!idle.state.busy);
        assert!(idle.state.twitch_auth.is_none());
        assert!(idle.commands.try_recv().is_err());
    }
}

#[test]
fn reconnect_busy_cancel_retains_preparation_slot_until_matching_receipt() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let mut idle = Idle::new(f.view());
    let attempt = idle.state.start_twitch_auth(f.id, String::new()).unwrap();
    let (_, gate) = idle.prepare(f.id, None);
    assert!(
        idle.state
            .start_twitch_auth(f.id, "SyntheticOverride456".into())
            .is_err()
    );
    idle.state.cancel_twitch_auth(Uuid::new_v4());
    assert_eq!(gate.load(Ordering::Acquire), 0);
    idle.state.cancel_twitch_auth(attempt);
    assert_eq!(gate.load(Ordering::Acquire), 1);
    assert!(idle.state.busy);
    assert!(idle.state.twitch_auth.as_ref().unwrap().job.is_none());
    assert!(idle.state.start_twitch_auth(f.id, String::new()).is_err());
    assert!(idle.commands.try_recv().is_err());
    idle.events
        .try_send(Outcome {
            initial_load: false,
            auth_attempt: Some(attempt),
            auth_job: None,
            oauth_schedule: BTreeMap::new(),
            packs: idle.state.packs.clone(),
            result: Err("Twitch authorization cancelled".into()),
        })
        .unwrap_or_else(|_| panic!("fake outcome queue full"));
    idle.state.poll();
    assert!(!idle.state.busy);
    assert_eq!(gate.load(Ordering::Acquire), 1);
    let next = idle.state.start_twitch_auth(f.id, String::new()).unwrap();
    assert_ne!(attempt, next);
    let (queued, new_gate) = idle.prepare(f.id, None);
    assert_eq!(queued, next);
    idle.state.cancel_twitch_auth(attempt);
    assert_eq!(new_gate.load(Ordering::Acquire), 0);
}

#[test]
fn reconnect_full_or_disconnected_command_queue_does_not_leave_phantom_auth() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let mut idle = Idle::new(f.view());
    idle.state
        .sender
        .try_send(Command::Refresh)
        .unwrap_or_else(|_| panic!("fake queue unexpectedly full"));
    assert!(idle.state.start_twitch_auth(f.id, String::new()).is_err());
    assert!(!idle.state.busy);
    assert!(idle.state.twitch_auth.is_none());
    assert!(matches!(
        idle.commands.try_recv().unwrap(),
        Command::Refresh
    ));
    assert!(idle.state.start_twitch_auth(f.id, String::new()).is_ok());
    idle.prepare(f.id, None);
    let Idle {
        mut state,
        commands,
        events: _events,
    } = Idle::new(f.view());
    drop(commands);
    assert!(state.start_twitch_auth(f.id, String::new()).is_err());
    assert!(!state.busy);
    assert!(state.twitch_auth.is_none());
}

#[test]
fn reconnect_outcome_disconnect_terminalizes_preparation_without_an_auth_job() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let mut idle = Idle::new(f.view());
    let attempt = idle.state.start_twitch_auth(f.id, String::new()).unwrap();
    let (_, gate) = idle.prepare(f.id, None);
    assert!(idle.state.twitch_auth.as_ref().unwrap().preparing);
    let Idle {
        mut state,
        commands,
        events,
    } = idle;
    drop(events); // No preparation outcome, and no real store/auth worker exists.
    state.poll();
    let view = state.twitch_auth_status(f.id).unwrap();
    assert_eq!(view.attempt_id, attempt);
    assert!(!view.active);
    assert!(view.challenge.is_none());
    assert_eq!(
        view.status,
        "Authorization preparation stopped; restart the app to retry"
    );
    assert_eq!(gate.load(Ordering::Acquire), 1);
    assert!(!state.busy);
    let auth = state.twitch_auth.as_ref().unwrap();
    assert!(!auth.preparing);
    assert!(auth.job.is_none() && auth.bundle.is_none());
    assert!(
        commands.try_recv().is_err(),
        "disconnect must not queue another authorization"
    );
    state.poll();
    assert_eq!(state.twitch_auth_status(f.id).unwrap().status, view.status);
    assert!(!state.twitch_auth_status(f.id).unwrap().active);
}

#[test]
fn reconnect_outcome_disconnect_preserves_commit_gate_and_reports_unknown_not_cancelled() {
    for committed_gate in [2, 3] {
        let mut f = Fixture::new();
        f.save_client("SyntheticSavedClient123");
        let mut idle = Idle::new(f.view());
        idle.state.start_twitch_auth(f.id, String::new()).unwrap();
        let (_, gate) = idle.prepare(f.id, None);
        gate.store(committed_gate, Ordering::Release);
        let auth = idle.state.twitch_auth.as_mut().unwrap();
        auth.preparing = false;
        // Synthetic tokens only; this factory constructs data, not an auth job.
        auth.bundle = Some(crate::twitch_auth::TokenBundle::fixture(
            "SyntheticSavedClient123",
        ));
        let Idle {
            mut state,
            commands,
            events,
        } = idle;
        drop(events);
        state.poll();
        assert_eq!(gate.load(Ordering::Acquire), committed_gate);
        assert!(!state.busy);
        let view = state.twitch_auth_status(f.id).unwrap();
        assert!(!view.active);
        assert_eq!(
            view.status,
            "Authorization save outcome unavailable; restart the app to check credentials"
        );
        assert!(!view.status.to_lowercase().contains("cancel"));
        let auth = state.twitch_auth.as_ref().unwrap();
        assert!(!auth.preparing);
        assert!(auth.job.is_none() && auth.bundle.is_none());
        assert!(
            commands.try_recv().is_err(),
            "unknown commit must not be retried"
        );
        state.poll();
        assert_eq!(gate.load(Ordering::Acquire), committed_gate);
        assert_eq!(state.twitch_auth_status(f.id).unwrap().status, view.status);
    }
}

#[test]
fn reconnect_outcome_disconnect_preserves_existing_cancelled_status() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let mut idle = Idle::new(f.view());
    let attempt = idle.state.start_twitch_auth(f.id, String::new()).unwrap();
    let (_, gate) = idle.prepare(f.id, None);
    idle.state.cancel_twitch_auth(attempt);
    let before = idle.state.twitch_auth_status(f.id).unwrap();
    assert_eq!(before.status, "Authorization cancelled");
    let Idle {
        mut state,
        commands,
        events,
    } = idle;
    drop(events);
    for _ in 0..2 {
        state.poll();
        let after = state.twitch_auth_status(f.id).unwrap();
        assert_eq!(after.status, before.status);
        assert!(!after.active);
        assert_eq!(gate.load(Ordering::Acquire), 1);
        let auth = state.twitch_auth.as_ref().unwrap();
        assert!(!auth.preparing);
        assert!(auth.job.is_none() && auth.bundle.is_none());
        assert!(!state.busy);
        assert!(commands.try_recv().is_err());
    }
}

#[test]
fn reconnect_poisoned_outcome_receiver_terminalizes_preparation_without_a_worker() {
    let mut f = Fixture::new();
    f.save_client("SyntheticSavedClient123");
    let mut idle = Idle::new(f.view());
    idle.state.start_twitch_auth(f.id, String::new()).unwrap();
    let (_, gate) = idle.prepare(f.id, None);
    // Exercise receiver-lock failure separately from channel disconnection.
    // The outcome sender stays alive; this caught panic creates no worker.
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = idle.state.receiver.lock().unwrap();
        panic!("synthetic outcome receiver poisoning");
    }));
    assert!(poisoned.is_err());
    idle.state.poll();
    assert_eq!(gate.load(Ordering::Acquire), 1);
    assert!(!idle.state.busy);
    let view = idle.state.twitch_auth_status(f.id).unwrap();
    assert!(!view.active);
    assert_eq!(
        view.status,
        "Authorization preparation stopped; restart the app to retry"
    );
    let auth = idle.state.twitch_auth.as_ref().unwrap();
    assert!(!auth.preparing);
    assert!(auth.job.is_none() && auth.bundle.is_none());
    assert!(idle.commands.try_recv().is_err());
}
