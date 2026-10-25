//! Host-owned lifecycle for explicitly configured local extension processes.
//! Executable package installation is a separate policy boundary. This manager
//! does not discover/run arbitrary programs or automatically retry failed code.
use crate::{
    extension_actions::{ActionCatalog, ActionControllers, HostActions},
    extension_connection::ExtensionConnection,
    extension_event_bridge::EventRelay,
    extension_hooks::{CapturedHookEvent, HookProcess, HookTrigger},
    extension_lifecycle::SequencedEvent,
    extension_process::ProcessSpec,
    extension_protocol::{Capability, HostEvent, IntegrationOrigin, Target},
};
use bevy::prelude::Resource;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[path = "extension_audio_playback.rs"]
mod audio_playback;
pub use audio_playback::AudioPlaybackRoute;

const MAX_EXTENSIONS: usize = 4;
const MAX_HOOK_CONFIGS: usize = 128;
const MAX_HOOK_QUEUE: usize = 32;
const MAX_HOOK_BYTES: usize = 1024 * 1024;
const HOOK_QUEUE_AGE: Duration = Duration::from_secs(5);
const HOOK_LIFETIME: Duration = Duration::from_secs(10);
const MAX_TEST_CONFIGS: usize = 128;
pub const MAX_RUNTIME_OBSERVATIONS: usize = 32;
/// Shared across every child, inside (never added to) the total shutdown budget.
pub const MAX_SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
const TEST_LIFETIME: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct TestRegistration {
    spec: ProcessSpec,
    origin: IntegrationOrigin,
    grants: BTreeSet<Capability>,
    settings: crate::extension_settings::Settings,
    active: Option<Uuid>,
    status: Option<&'static str>,
}

struct HookRegistration {
    subscribed_at: Instant,
    started_sequence_floor: u64,
    origin: IntegrationOrigin,
    grants: BTreeSet<Capability>,
    settings: crate::extension_settings::Settings,
    hooks: Vec<HookProcess>,
    enabled: bool,
    generation: u64,
    status: String,
    /// Queue overflow/expiry losses for this registration, retained across
    /// stop, restart and grant changes. Removal/reconfiguration starts at zero.
    dropped_invocations: u64,
    // Only fixed host messages are retained; never payload or process text.
    last_loss_reason: Option<&'static str>,
}
impl HookRegistration {
    fn record_loss(&mut self, reason: &'static str) {
        self.dropped_invocations = self.dropped_invocations.saturating_add(1);
        self.last_loss_reason = Some(reason);
    }

    fn status_with_losses(&self, status: &str) -> String {
        match self.last_loss_reason {
            Some(reason) => format!(
                "{status} · Dropped hook invocations: {} · Last loss: {reason}",
                self.dropped_invocations
            ),
            None => status.into(),
        }
    }
}
struct HookInvocation {
    id: Uuid,
    process: ProcessSpec,
    captured: CapturedHookEvent,
    target: Option<Target>,
    queued: Instant,
}

struct Managed {
    subscribed_at: Instant,
    started_sequence_floor: u64,
    started_observed: bool,
    stopping_observed: bool,
    stopped_observed: bool,
    terminal_loss_observed: bool,
    stop_request: Option<(crate::extension_protocol::ExtensionStopReason, Instant)>,
    test_parent: Option<Uuid>,
    /// Some(None) is a hook captured without an avatar; None is a service.
    hook_target: Option<Option<Target>>,
    /// Metadata-only selection invocation. Never bind/upgrade it to an avatar.
    selection_revision: Option<u64>,
    metadata_only: bool,
    settings: crate::extension_settings::Settings,
    spec: ProcessSpec,
    origin: IntegrationOrigin,
    grants: BTreeSet<Capability>,
    revoked_controls: BTreeSet<Capability>,
    generation: u64,
    connection: ExtensionConnection,
    relay: EventRelay,
    enabled: bool,
    restart: bool,
    remove: bool,
    sent_target: Option<Option<Target>>,
    error: Option<String>,
}

struct SpawnObservation {
    event: crate::extension_lifecycle::LifecycleEvent,
    observed_at: Instant,
}
#[derive(Default)]
struct RuntimeObservations {
    pending: VecDeque<SpawnObservation>,
    dropped: u64,
    dropped_receipts: u64,
}
impl RuntimeObservations {
    fn push(&mut self, observation: SpawnObservation) {
        if self.pending.len() == MAX_RUNTIME_OBSERVATIONS {
            self.pending.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.pending.push_back(observation);
    }
}

#[derive(Default)]
struct Inner {
    observations: RuntimeObservations,
    last_started_sequence: u64,
    source_times: BTreeMap<u64, Instant>,
    entries: BTreeMap<Uuid, Managed>,
    actions: HostActions,
    hooks: BTreeMap<Uuid, HookRegistration>,
    pending_hooks: VecDeque<HookInvocation>,
    hook_sequence: u64,
    selection_revision: u64,
    app_ready_seen: bool,
    app_run_id: Option<Uuid>,
    quiescing: bool,
    stopping_offered: bool,
    tests: BTreeMap<Uuid, TestRegistration>,
    current_target: Option<Target>,
}

/// Keep a clone outside App::run so shutdown can wait outside render teardown.
#[derive(Resource, Clone, Default)]
pub struct ExtensionRuntime(Arc<Mutex<Inner>>);

#[derive(Clone, Debug)]
pub struct RuntimeView {
    pub id: Uuid,
    /// Temporary manual-test child; the installation itself uses None.
    pub test_parent: Option<Uuid>,
    pub name: String,
    pub enabled: bool,
    pub active: bool,
    pub status: String,
    pub logs: Vec<String>,
}

impl ExtensionRuntime {
    /// Private receipt to the original incarnation only; never a broadcast or
    /// hook trigger. Original targets survive cancellation after avatar changes.
    pub fn publish_audio_completion(
        &self,
        route: &crate::extension_audio::AudioRoute,
        turn_id: String,
        outcome: crate::extension_protocol::AudioInputOutcome,
    ) -> bool {
        let mut inner = self.inner();
        let Some(entry) = inner.entries.values_mut().find(|entry| {
            entry.enabled
                && !entry.restart
                && !entry.remove
                && entry.generation == route.process_generation
                && entry.connection.session_id() == route.session
                && entry.connection.origin() == &route.origin
                && entry.connection.is_active()
                && [Capability::AudioTranscribe, Capability::MessageDeliver]
                    .iter()
                    .all(|cap| entry.grants.contains(cap) && !entry.revoked_controls.contains(cap))
        }) else {
            return false;
        };
        entry
            .connection
            .publish_event(HostEvent::AudioInputFinished {
                target: route.target,
                turn_id,
                outcome,
            })
            .is_ok()
    }
    pub fn audio_source_identity(&self, id: Uuid) -> Option<(Uuid, u64, IntegrationOrigin)> {
        let inner = self.inner();
        inner
            .entries
            .get(&id)
            .filter(|entry| {
                entry.enabled
                    && !entry.restart
                    && !entry.remove
                    && entry.test_parent.is_none()
                    && entry.hook_target.is_none()
                    && entry.connection.is_active()
                    && [Capability::AudioTranscribe, Capability::MessageDeliver]
                        .iter()
                        .all(|cap| {
                            entry.grants.contains(cap) && !entry.revoked_controls.contains(cap)
                        })
            })
            .map(|entry| {
                (
                    entry.connection.session_id(),
                    entry.generation,
                    entry.connection.origin().clone(),
                )
            })
    }
    /// Persistent intake stays valid only for the currently running incarnation.
    /// One-shot hooks use immediate delivery, not delayed chat offers.
    pub fn chat_source_generation(&self, id: Uuid) -> Option<u64> {
        let inner = self.inner();
        if inner.hooks.contains_key(&id) {
            return None;
        }
        inner
            .entries
            .get(&id)
            .filter(|entry| {
                entry.enabled
                    && !entry.restart
                    && !entry.remove
                    && entry.test_parent.is_none()
                    && entry.grants.contains(&Capability::MessageDeliver)
                    && !entry.revoked_controls.contains(&Capability::MessageDeliver)
                    && entry.connection.is_active()
            })
            .map(|entry| entry.generation)
    }
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Inputs come from host installation/configuration, never protocol actions.
    pub fn start(
        &self,
        id: Uuid,
        spec: ProcessSpec,
        origin: IntegrationOrigin,
        grants: impl IntoIterator<Item = Capability>,
    ) -> Result<(), String> {
        self.start_configured(id, spec, origin, grants, Default::default())
    }

    pub fn start_configured(
        &self,
        id: Uuid,
        spec: ProcessSpec,
        origin: IntegrationOrigin,
        grants: impl IntoIterator<Item = Capability>,
        settings: crate::extension_settings::Settings,
    ) -> Result<(), String> {
        let mut inner = self.inner();
        if inner.quiescing {
            return Err("Extension runtime is shutting down".into());
        }
        if inner.entries.contains_key(&id) || inner.hooks.contains_key(&id) {
            return Err("Extension already exists; restart its current instance".into());
        }
        if inner.entries.len() >= MAX_EXTENSIONS {
            return Err("Local extension process limit reached (4)".into());
        }
        let grants: BTreeSet<_> = grants.into_iter().collect();
        let subscribed_at = Instant::now();
        let started_sequence_floor = inner.last_started_sequence;
        let connection = ExtensionConnection::spawn_configured(
            spec.clone(),
            origin.clone(),
            grants.iter().copied(),
            1,
            settings.clone(),
        )?;
        inner.entries.insert(
            id,
            Managed {
                subscribed_at,
                started_sequence_floor,
                started_observed: false,
                stopping_observed: false,
                stopped_observed: false,
                terminal_loss_observed: false,
                stop_request: None,
                test_parent: None,
                hook_target: None,
                selection_revision: None,
                metadata_only: false,
                settings,
                spec,
                origin,
                grants,
                revoked_controls: BTreeSet::new(),
                generation: 1,
                connection,
                relay: EventRelay::default(),
                enabled: true,
                restart: false,
                remove: false,
                sent_target: None,
                error: None,
            },
        );
        Ok(())
    }
    /// Register event hooks without launching code until a matching source event.
    pub fn configure_hooks(
        &self,
        id: Uuid,
        origin: IntegrationOrigin,
        grants: impl IntoIterator<Item = Capability>,
        settings: crate::extension_settings::Settings,
        mut hooks: Vec<HookProcess>,
    ) -> Result<(), String> {
        origin.validate().map_err(|error| error.to_string())?;
        crate::extension_settings::validate_wire_values(&settings)?;
        if hooks.is_empty() || hooks.len() > crate::extension_manifest::MAX_HOOKS {
            return Err(format!(
                "An integration requires 1–{} unique lifecycle hooks",
                crate::extension_manifest::MAX_HOOKS
            ));
        }
        let mut triggers = BTreeSet::new();
        for hook in &mut hooks {
            // Enum order is not part of the external schema.
            if !triggers.insert(format!("{:?}", hook.trigger)) {
                return Err("Hook triggers must be unique".into());
            }
            hook.process.lifetime = Some(HOOK_LIFETIME);
        }
        let mut inner = self.inner();
        if inner.quiescing {
            return Err("Extension runtime is shutting down".into());
        }
        if inner.entries.contains_key(&id) || inner.hooks.contains_key(&id) {
            return Err(
                "Integration is already configured; remove its previous configuration first".into(),
            );
        }
        if inner.hooks.len() >= MAX_HOOK_CONFIGS {
            return Err("Hook registration limit reached (128)".into());
        }
        let started_sequence_floor = inner.last_started_sequence;
        inner.hooks.insert(
            id,
            HookRegistration {
                subscribed_at: Instant::now(),
                started_sequence_floor,
                origin,
                grants: grants.into_iter().collect(),
                settings,
                hooks,
                enabled: true,
                generation: 0,
                status: "Waiting for events".into(),
                dropped_invocations: 0,
                last_loss_reason: None,
            },
        );
        Ok(())
    }
    pub fn process_count(&self) -> usize {
        self.inner().entries.len()
    }

    fn test_parent_enabled(inner: &Inner, id: Uuid) -> bool {
        inner.hooks.get(&id).map_or_else(
            || {
                inner.entries.get(&id).is_some_and(|entry| {
                    entry.test_parent.is_none() && entry.enabled && !entry.remove
                })
            },
            |hook| hook.enabled,
        )
    }

    fn cancel_tests(inner: &mut Inner, id: Uuid, reason: &str) {
        for entry in inner
            .entries
            .values_mut()
            .filter(|entry| entry.test_parent == Some(id))
        {
            entry.enabled = false;
            entry.restart = false;
            entry.remove = true;
            entry.connection.stop(reason);
            entry.relay = EventRelay::default();
        }
    }

    /// Called by the installed coordinator after successful enabled parent
    /// registration. Replacing this configuration cancels its previous test.
    /// Registration itself never launches a process or reserves a process slot.
    pub fn configure_test(
        &self,
        id: Uuid,
        mut spec: ProcessSpec,
        origin: IntegrationOrigin,
        grants: impl IntoIterator<Item = Capability>,
        settings: crate::extension_settings::Settings,
    ) -> Result<(), String> {
        origin.validate().map_err(|error| error.to_string())?;
        crate::extension_settings::validate_wire_values(&settings)?;
        let grants: BTreeSet<_> = grants.into_iter().collect();
        let mut inner = self.inner();
        if inner.quiescing {
            return Err("Extension runtime is shutting down".into());
        }
        if !Self::test_parent_enabled(&inner, id) {
            return Err("Manual test requires a configured, enabled installation".into());
        }
        let (parent_origin, parent_grants) = if let Some(hook) = inner.hooks.get(&id) {
            (&hook.origin, &hook.grants)
        } else {
            let parent = &inner.entries[&id];
            (&parent.origin, &parent.grants)
        };
        if &origin != parent_origin || !grants.is_subset(parent_grants) {
            return Err("Manual test identity or grants do not match its installation".into());
        }
        if !inner.tests.contains_key(&id) && inner.tests.len() >= MAX_TEST_CONFIGS {
            return Err("Manual test registration limit reached (128)".into());
        }
        spec.lifetime = Some(spec.lifetime.unwrap_or(TEST_LIFETIME).min(TEST_LIFETIME));
        Self::cancel_tests(&mut inner, id, "Manual test configuration replaced");
        inner.tests.insert(
            id,
            TestRegistration {
                spec,
                origin,
                grants,
                settings,
                active: None,
                status: None,
            },
        );
        Ok(())
    }

    /// Includes stopped children awaiting their authoritative terminal receipt.
    pub fn test_active(&self, id: Uuid) -> bool {
        self.inner()
            .entries
            .values()
            .any(|entry| entry.test_parent == Some(id))
    }

    /// Fixed host classification retained after reaping, until the next start,
    /// configuration replacement, disable or removal. Never an action receipt.
    pub fn test_status(&self, id: Uuid) -> Option<String> {
        self.inner().tests.get(&id)?.status.map(str::to_owned)
    }

    /// Start one separate child, pinned to the most recent frame's loaded
    /// target. Failures are never queued/retried; ordinary host actions still
    /// enforce target generation, grants, and manual/busy ownership.
    pub fn start_test(&self, id: Uuid) -> Result<Uuid, String> {
        let mut inner = self.inner();
        if inner.quiescing {
            return Err("Extension runtime is shutting down".into());
        }
        if !Self::test_parent_enabled(&inner, id) {
            return Err("Manual test requires a configured, enabled installation".into());
        }
        let registration = inner
            .tests
            .get(&id)
            .ok_or("Installation has no configured manual test")?
            .clone();
        let target = inner
            .current_target
            .ok_or("Load a character before starting a manual test")?;
        if inner
            .entries
            .values()
            .any(|entry| entry.test_parent == Some(id))
        {
            return Err("A manual test for this installation is running or still stopping".into());
        }
        if inner.entries.len() >= MAX_EXTENSIONS {
            return Err("Local extension process limit reached (4)".into());
        }
        let invocation = Uuid::new_v4();
        if inner.entries.contains_key(&invocation)
            || inner.hooks.contains_key(&invocation)
            || inner.tests.contains_key(&invocation)
        {
            return Err("Manual test invocation identity collision".into());
        }
        let connection = ExtensionConnection::spawn_configured(
            registration.spec.clone(),
            registration.origin.clone(),
            registration.grants.iter().copied(),
            1,
            registration.settings.clone(),
        )?;
        inner.entries.insert(
            invocation,
            Managed {
                subscribed_at: Instant::now(),
                started_sequence_floor: 0,
                started_observed: false,
                stopping_observed: false,
                stopped_observed: false,
                terminal_loss_observed: false,
                stop_request: None,
                test_parent: Some(id),
                hook_target: Some(Some(target)),
                selection_revision: None,
                metadata_only: false,
                settings: registration.settings,
                spec: registration.spec,
                origin: registration.origin,
                grants: registration.grants,
                revoked_controls: BTreeSet::new(),
                generation: 1,
                connection,
                relay: EventRelay::default(),
                enabled: true,
                restart: false,
                remove: true,
                sent_target: None,
                error: None,
            },
        );
        let registration = inner.tests.get_mut(&id).expect("test registration exists");
        registration.active = Some(invocation);
        registration.status = Some("Test process starting");
        Ok(invocation)
    }

    pub fn stop(&self, id: Uuid) {
        let mut inner = self.inner();
        Self::request_stop(
            &mut inner,
            id,
            crate::extension_protocol::ExtensionStopReason::Disabled,
        );
        inner.tests.remove(&id);
        Self::cancel_tests(&mut inner, id, "Installation disabled by host");
        if let Some(hook) = inner.hooks.get_mut(&id) {
            hook.enabled = false;
            hook.status = "Disabled by host".into();
        }
        inner.pending_hooks.retain(|invocation| invocation.id != id);
        if let Some(entry) = inner.entries.get_mut(&id) {
            entry.enabled = false;
            entry.restart = false;
            entry.connection.stop("Disabled by host");
            entry.relay = EventRelay::default();
        }
    }
    pub fn remove(&self, id: Uuid) {
        self.remove_with_reason(id, crate::extension_protocol::ExtensionStopReason::Removed);
    }
    pub(crate) fn remove_with_reason(
        &self,
        id: Uuid,
        reason: crate::extension_protocol::ExtensionStopReason,
    ) {
        let mut inner = self.inner();
        Self::request_stop(&mut inner, id, reason);
        inner.tests.remove(&id);
        Self::cancel_tests(&mut inner, id, "Installation removed by host");
        inner.hooks.remove(&id);
        inner.pending_hooks.retain(|invocation| invocation.id != id);
        if let Some(entry) = inner.entries.get_mut(&id) {
            entry.remove = true;
            entry.enabled = false;
            entry.restart = false;
            entry.connection.stop("Removed by host");
            entry.relay = EventRelay::default();
        }
    }
    /// Hook restart discards queued invocations and waits for a new event.
    /// Queue overflow/expiry diagnostics survive reset; removing the registration
    /// clears them. Explicit host cancellation is not counted as a queue loss.
    pub fn restart(&self, id: Uuid) {
        let mut inner = self.inner();
        Self::request_stop(
            &mut inner,
            id,
            crate::extension_protocol::ExtensionStopReason::Restarted,
        );
        if inner.quiescing {
            return;
        }
        Self::cancel_tests(&mut inner, id, "Installation restart requested");
        if let Some(entry) = inner.entries.get_mut(&id)
            && entry.test_parent.is_some()
        {
            entry.enabled = false;
            entry.restart = false;
            entry.remove = true;
            entry
                .connection
                .stop("Manual test cannot restart automatically");
            entry.relay = EventRelay::default();
            return;
        }
        let started_sequence_floor = inner.last_started_sequence;
        if let Some(hook) = inner.hooks.get_mut(&id) {
            hook.subscribed_at = Instant::now();
            hook.started_sequence_floor = started_sequence_floor;
            hook.enabled = true;
            hook.status = "Waiting for new events".into();
            inner.pending_hooks.retain(|invocation| invocation.id != id);
            if let Some(entry) = inner.entries.get_mut(&id) {
                entry.enabled = false;
                entry.remove = true;
                entry.connection.stop("Hook reset by host");
                entry.relay = EventRelay::default();
            }
            return;
        }
        if let Some(entry) = inner.entries.get_mut(&id)
            && !entry.remove
        {
            entry.subscribed_at = Instant::now();
            entry.started_sequence_floor = started_sequence_floor;
            entry.restart = true;
            entry.enabled = true;
            entry.connection.stop("Restart requested");
            entry.relay = EventRelay::default();
        }
    }
    pub fn set_grants(&self, id: Uuid, grants: impl IntoIterator<Item = Capability>) {
        let mut inner = self.inner();
        let grants: BTreeSet<_> = grants.into_iter().collect();
        if inner.quiescing {
            if let Some(entry) = inner.entries.get_mut(&id) {
                entry.connection.set_grants(grants);
            }
            return;
        }
        if let Some(test) = inner.tests.get_mut(&id) {
            test.grants = grants.clone();
        }
        for child in inner
            .entries
            .values_mut()
            .filter(|entry| entry.test_parent == Some(id))
        {
            // Regrant cannot expand the authority captured by this invocation.
            let restricted = child.grants.intersection(&grants).copied().collect();
            if child.grants != restricted {
                child.grants = restricted;
                child.connection.set_grants(child.grants.iter().copied());
                child.enabled = false;
                child.restart = false;
                child.remove = true;
                child.connection.stop("Manual test permission revoked");
                child.relay = EventRelay::default();
            }
        }
        let started_sequence_floor = inner.last_started_sequence;
        if let Some(hook) = inner.hooks.get_mut(&id) {
            if !hook.grants.contains(&Capability::LifecycleObserve)
                && grants.contains(&Capability::LifecycleObserve)
            {
                hook.subscribed_at = Instant::now();
                hook.started_sequence_floor = started_sequence_floor;
            }
            hook.grants = grants.clone();
            if hook.enabled {
                hook.status = if grants.contains(&Capability::LifecycleObserve) {
                    "Waiting for new events"
                } else {
                    "Waiting for observation permission"
                }
                .into();
            }
        }
        inner
            .pending_hooks
            .retain_mut(|invocation| invocation.id != id || invocation.captured.restrict(&grants));
        if let Some(entry) = inner.entries.get_mut(&id) {
            // An invocation cannot gain a capability absent at capture/launch.
            let grants = if entry.hook_target.is_some() {
                entry.grants.intersection(&grants).copied().collect()
            } else {
                grants
            };
            entry
                .revoked_controls
                .extend(
                    entry
                        .grants
                        .difference(&grants)
                        .copied()
                        .filter(|capability| {
                            matches!(
                                capability,
                                Capability::MotionPlay
                                    | Capability::AudioPlayback
                                    | Capability::AudioTranscribe
                                    | Capability::ExpressionApply
                                    | Capability::MessageDeliver
                            )
                        }),
                );
            if !entry.grants.contains(&Capability::LifecycleObserve)
                && grants.contains(&Capability::LifecycleObserve)
            {
                entry.subscribed_at = Instant::now();
                entry.started_sequence_floor = started_sequence_floor;
            }
            entry.grants = grants;
            entry.sent_target = None;
            entry
                .relay
                .set_grants(&mut entry.connection, entry.grants.iter().copied());
            if entry.hook_target.is_some() && !entry.grants.contains(&Capability::LifecycleObserve)
            {
                entry.enabled = false;
                entry.remove = true;
                entry.connection.stop("Hook observation permission revoked");
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        let inner = self.inner();
        inner.entries.is_empty() && inner.hooks.is_empty() && inner.tests.is_empty()
    }
    pub fn views(&self) -> Vec<RuntimeView> {
        let inner = self.inner();
        let mut views: Vec<_> = inner
            .entries
            .iter()
            .map(|(id, entry)| RuntimeView {
                logs: entry
                    .connection
                    .logs()
                    .iter()
                    .rev()
                    .take(8)
                    .rev()
                    .cloned()
                    .collect(),
                id: *id,
                test_parent: entry.test_parent,
                name: if entry.test_parent.is_some() {
                    format!("{} · Manual test", entry.origin.display_name)
                } else {
                    entry.origin.display_name.clone()
                },
                enabled: entry.enabled,
                active: entry.connection.is_active(),
                status: match inner.hooks.get(id) {
                    Some(hook) => {
                        hook.status_with_losses(&entry.error.clone().unwrap_or_else(|| {
                            format!("{} · {}", entry.connection.status(), hook.status)
                        }))
                    }
                    None => entry
                        .error
                        .clone()
                        .unwrap_or_else(|| entry.connection.status().into()),
                },
            })
            .collect();
        views.extend(
            inner
                .hooks
                .iter()
                .filter(|(id, _)| !inner.entries.contains_key(id))
                .map(|(id, hook)| RuntimeView {
                    logs: Vec::new(),
                    id: *id,
                    test_parent: None,
                    name: hook.origin.display_name.clone(),
                    enabled: hook.enabled,
                    active: false,
                    status: hook.status_with_losses(&hook.status),
                }),
        );
        views
    }
    fn invocation_bytes(invocation: &HookInvocation) -> usize {
        std::mem::size_of::<HookInvocation>()
            + invocation.captured.bytes()
            + invocation
                .process
                .args
                .iter()
                .map(String::len)
                .sum::<usize>()
            + invocation.process.program.as_os_str().len()
            + invocation.process.working_directory.as_os_str().len()
            + invocation.process.data_directory.as_os_str().len()
    }

    fn capture_hooks(
        inner: &mut Inner,
        events: &[SequencedEvent],
        target: Option<Target>,
        now: Instant,
    ) {
        if inner.quiescing {
            return;
        }
        Self::observe_run_identity(inner, events);
        // Process the latest revision in the whole batch before admission. A→B→A
        // remains three source records, but only the newest selection may launch
        // a handler. Replayed source sequences/revisions cannot launch it again.
        let previous_revision = inner.selection_revision;
        let hook_sequence = inner.hook_sequence;
        for source in events
            .iter()
            .filter(|source| source.sequence > hook_sequence)
        {
            if let crate::extension_lifecycle::LifecycleEvent::CharacterSelected {
                selection_revision,
                ..
            } = &source.event
            {
                inner.selection_revision = inner.selection_revision.max(*selection_revision);
            }
        }
        inner.pending_hooks.retain_mut(|invocation| {
            let Some(hook) = inner.hooks.get_mut(&invocation.id) else {
                return false;
            };
            if !hook.enabled || !invocation.captured.restrict(&hook.grants) {
                return false;
            }
            let obsolete = match invocation.captured.selection_revision() {
                Some(revision) => revision != inner.selection_revision,
                None => !invocation.captured.metadata_only() && invocation.target != target,
            };
            if obsolete {
                hook.status = "Queued hook discarded: target changed".into();
                return false;
            }
            if now.saturating_duration_since(invocation.queued) >= HOOK_QUEUE_AGE {
                hook.record_loss("Queued hook expired after 5 seconds");
                hook.status = "Queued hook expired after 5 seconds".into();
                return false;
            }
            true
        });
        for source in events {
            if source.sequence <= inner.hook_sequence {
                continue;
            }
            inner.hook_sequence = source.sequence;
            if matches!(
                source.event,
                crate::extension_lifecycle::LifecycleEvent::AppReady { .. }
            ) {
                if inner.app_ready_seen {
                    continue;
                }
                // Consume the startup edge even with no enabled/matching hooks.
                // Later registration/restart is not historical subscription.
                inner.app_ready_seen = true;
            }
            if let crate::extension_lifecycle::LifecycleEvent::CharacterSelected {
                selection_revision,
                ..
            } = &source.event
                && (*selection_revision != inner.selection_revision
                    || *selection_revision <= previous_revision)
            {
                continue;
            }
            for (id, hook) in &mut inner.hooks {
                if matches!(
                    source.event,
                    crate::extension_lifecycle::LifecycleEvent::ExtensionStarted { .. }
                        | crate::extension_lifecycle::LifecycleEvent::MotionPlaybackStarted { .. }
                        | crate::extension_lifecycle::LifecycleEvent::MotionPlaybackEnded { .. }
                        | crate::extension_lifecycle::LifecycleEvent::ExtensionStopping { .. }
                        | crate::extension_lifecycle::LifecycleEvent::ExtensionStopped { .. }
                ) && !Self::source_eligible(
                    &inner.source_times,
                    inner.last_started_sequence,
                    source.sequence,
                    hook.subscribed_at,
                    hook.started_sequence_floor,
                ) {
                    continue;
                }
                if !hook.enabled || !hook.grants.contains(&Capability::LifecycleObserve) {
                    continue;
                }
                let trigger = HookTrigger::of(&source.event);
                // Gaps must invalidate subscriptions even without a matching trigger.
                if trigger.is_some_and(|trigger| {
                    !hook.hooks.iter().any(|handler| handler.trigger == trigger)
                }) {
                    continue;
                }
                if trigger.is_some() && inner.pending_hooks.len() >= MAX_HOOK_QUEUE {
                    hook.record_loss("Hook event dropped: queue limit reached");
                    hook.status = "Hook event dropped: queue limit reached".into();
                    continue;
                }
                let captured = match CapturedHookEvent::capture(&source.event, &hook.grants) {
                    Ok(Some(captured)) => captured,
                    Ok(None) => continue,
                    Err(error) => {
                        hook.status = format!("Hook subscription stopped: {error}");
                        hook.enabled = false;
                        inner
                            .pending_hooks
                            .retain(|invocation| invocation.id != *id);
                        if let Some(entry) = inner.entries.get_mut(id) {
                            entry.enabled = false;
                            entry.remove = true;
                            entry.connection.stop("Hook source stream lost");
                            entry.relay = EventRelay::default();
                        }
                        continue;
                    }
                };
                let Some(handler) = hook
                    .hooks
                    .iter()
                    .find(|handler| handler.trigger == captured.trigger())
                else {
                    continue;
                };
                let invocation = HookInvocation {
                    id: *id,
                    process: handler.process.clone(),
                    target: if captured.metadata_only() {
                        None
                    } else {
                        target
                    },
                    captured,
                    queued: now,
                };
                let bytes = Self::invocation_bytes(&invocation);
                let queued_bytes: usize =
                    inner.pending_hooks.iter().map(Self::invocation_bytes).sum();
                if inner.pending_hooks.len() >= MAX_HOOK_QUEUE
                    || bytes > MAX_HOOK_BYTES.saturating_sub(queued_bytes)
                {
                    hook.record_loss("Hook event dropped: queue limit reached");
                    hook.status = "Hook event dropped: queue limit reached".into();
                    continue;
                }
                inner.pending_hooks.push_back(invocation);
                hook.status = "Hook event queued".into();
            }
        }
    }

    fn launch_hook(inner: &mut Inner, target: Option<Target>, now: Instant) {
        if inner.quiescing {
            return;
        }
        if inner.entries.len() >= MAX_EXTENSIONS {
            return;
        }
        let Some(index) = inner
            .pending_hooks
            .iter()
            .position(|invocation| !inner.entries.contains_key(&invocation.id))
        else {
            return;
        };
        let Some(mut invocation) = inner.pending_hooks.remove(index) else {
            return;
        };
        let Some(hook) = inner.hooks.get_mut(&invocation.id) else {
            return;
        };
        let obsolete = match invocation.captured.selection_revision() {
            Some(revision) => revision != inner.selection_revision,
            None => !invocation.captured.metadata_only() && invocation.target != target,
        };
        if !hook.enabled || obsolete || !invocation.captured.restrict(&hook.grants) {
            return;
        }
        if now.saturating_duration_since(invocation.queued) >= HOOK_QUEUE_AGE {
            hook.record_loss("Queued hook expired after 5 seconds");
            hook.status = "Queued hook expired after 5 seconds".into();
            return;
        }
        let Some(generation) = hook.generation.checked_add(1) else {
            hook.enabled = false;
            hook.status = "Hook process generation exhausted".into();
            return;
        };
        hook.generation = generation;
        invocation.process.lifetime = Some(HOOK_LIFETIME);
        let grants = invocation.captured.grants().clone();
        let mut connection = match ExtensionConnection::spawn_configured(
            invocation.process.clone(),
            hook.origin.clone(),
            grants.iter().copied(),
            generation,
            hook.settings.clone(),
        ) {
            Ok(connection) => connection,
            Err(error) => {
                hook.status = format!("Hook launch failed: {error}");
                return;
            }
        };
        let mut relay = EventRelay::default();
        let failed = relay
            .enqueue(invocation.captured.event(), &mut connection)
            .is_err();
        hook.status = if failed {
            "Hook event delivery failed"
        } else {
            "Invocation started"
        }
        .into();
        inner.entries.insert(
            invocation.id,
            Managed {
                subscribed_at: Instant::now(),
                started_sequence_floor: 0,
                started_observed: false,
                stopping_observed: false,
                stopped_observed: false,
                terminal_loss_observed: false,
                stop_request: None,
                test_parent: None,
                hook_target: Some(invocation.target),
                selection_revision: invocation.captured.selection_revision(),
                metadata_only: invocation.captured.metadata_only(),
                settings: hook.settings.clone(),
                spec: invocation.process,
                origin: hook.origin.clone(),
                grants,
                revoked_controls: BTreeSet::new(),
                generation,
                connection,
                relay,
                enabled: !failed,
                restart: false,
                remove: failed,
                sent_target: None,
                error: None,
            },
        );
    }

    /// Ordinary Update work; process pipes and joins remain on their supervisor.
    pub fn frame(
        &self,
        events: &[SequencedEvent],
        target: Option<Target>,
        desktop_owned: bool,
        catalog: &ActionCatalog,
        controllers: ActionControllers<'_>,
    ) {
        let ActionControllers {
            animation,
            behavior,
            mut external_messages,
        } = controllers;
        let mut inner = self.inner();
        if inner.quiescing {
            inner.actions.cancel_all(animation, behavior);
            for entry in inner.entries.values_mut() {
                entry.connection.drain_target(None);
            }
            return;
        }
        inner.current_target = target;
        Self::capture_hooks(&mut inner, events, target, Instant::now());
        let disabled_tests: Vec<_> = inner
            .tests
            .keys()
            .copied()
            .filter(|id| !Self::test_parent_enabled(&inner, *id))
            .collect();
        for id in disabled_tests {
            inner.tests.remove(&id);
            Self::cancel_tests(
                &mut inner,
                id,
                "Manual test installation is no longer enabled",
            );
        }
        let selection_revision = inner.selection_revision;
        let last_started_sequence = inner.last_started_sequence;
        let Inner {
            entries,
            actions,
            hooks,
            tests,
            observations,
            source_times,
            ..
        } = &mut *inner;
        for (id, entry) in entries.iter_mut() {
            Self::collect_started_entry(*id, entry, observations);
            // Startup/selection hooks never gain avatar authority. Only selection
            // hooks are superseded by a new selection; startup survives it.
            let obsolete = match entry.selection_revision {
                Some(revision) => revision != selection_revision,
                None => {
                    !entry.metadata_only
                        && entry.hook_target.is_some_and(|captured| captured != target)
                }
            };
            let action_target = if entry.metadata_only { None } else { target };
            if obsolete {
                entry.enabled = false;
                entry.remove = true;
                entry.connection.stop(if entry.test_parent.is_some() {
                    "Manual test target changed"
                } else {
                    "Hook target changed"
                });
                entry.relay = EventRelay::default();
            }
            if !entry.revoked_controls.is_empty() {
                let restricted: BTreeSet<_> = entry
                    .grants
                    .difference(&entry.revoked_controls)
                    .copied()
                    .collect();
                actions.reconcile(
                    &crate::extension_actions::ActionContext {
                        session_id: entry.connection.session_id(),
                        process_generation: entry.generation,
                        target: action_target,
                        grants: &restricted,
                        origin: &entry.origin,
                        desktop_owned,
                    },
                    animation,
                    behavior,
                );
                entry.revoked_controls.clear();
            }
            if !entry.enabled || !entry.connection.is_active() {
                actions.cancel(entry.connection.session_id(), animation, behavior);
                entry.relay = EventRelay::default();
                entry.connection.drain_target(None);
                if entry.hook_target.is_some() {
                    entry.remove = true;
                    if let Some(hook) = hooks.get_mut(id) {
                        hook.status = entry.connection.status().into();
                    }
                }
            }
            if entry.restart && entry.connection.termination().is_some() {
                // Terminal evidence and spawn evidence are independent of the
                // bounded output queue. Preserve the old identity before swap.
                Self::collect_started_entry(*id, entry, observations);
                entry.restart = false;
                if let Some(generation) = entry.generation.checked_add(1) {
                    let subscribed_at = Instant::now();
                    match ExtensionConnection::spawn_configured(
                        entry.spec.clone(),
                        entry.origin.clone(),
                        entry.grants.iter().copied(),
                        generation,
                        entry.settings.clone(),
                    ) {
                        Ok(connection) => {
                            entry.connection = connection;
                            entry.subscribed_at = subscribed_at;
                            entry.started_sequence_floor = last_started_sequence;
                            entry.started_observed = false;
                            entry.stopping_observed = false;
                            entry.stopped_observed = false;
                            entry.terminal_loss_observed = false;
                            entry.stop_request = None;
                            entry.generation = generation;
                            entry.relay = EventRelay::default();
                            entry.sent_target = None;
                            entry.error = None;
                        }
                        Err(error) => {
                            entry.enabled = false;
                            entry.error = Some(error);
                        }
                    }
                } else {
                    entry.enabled = false;
                    entry.error = Some("Process generation exhausted".into());
                }
            }
            if !entry.enabled || !entry.connection.is_active() {
                continue;
            }
            if (entry.grants.contains(&Capability::LifecycleObserve)
                || audio_playback::playback_allowed(entry))
                && entry.sent_target != Some(action_target)
            {
                let published = if entry.grants.contains(&Capability::LifecycleObserve) {
                    entry
                        .connection
                        .publish_lifecycle(HostEvent::TargetChanged {
                            target: action_target,
                        })
                } else {
                    entry.connection.publish_audio_target(action_target)
                };
                if published.is_err() {
                    actions.cancel(entry.connection.session_id(), animation, behavior);
                    continue;
                }
                entry.sent_target = Some(action_target);
            }
            for event in events.iter().filter(|_| entry.hook_target.is_none()) {
                if matches!(
                    event.event,
                    crate::extension_lifecycle::LifecycleEvent::ExtensionStarted { .. }
                        | crate::extension_lifecycle::LifecycleEvent::MotionPlaybackStarted { .. }
                        | crate::extension_lifecycle::LifecycleEvent::MotionPlaybackEnded { .. }
                        | crate::extension_lifecycle::LifecycleEvent::ExtensionStopping { .. }
                        | crate::extension_lifecycle::LifecycleEvent::ExtensionStopped { .. }
                ) && !Self::source_eligible(
                    source_times,
                    last_started_sequence,
                    event.sequence,
                    entry.subscribed_at,
                    entry.started_sequence_floor,
                ) {
                    continue;
                }
                if entry
                    .relay
                    .enqueue(&event.event, &mut entry.connection)
                    .is_err()
                {
                    break;
                }
            }
            let _ = entry.relay.pump(&mut entry.connection);
            actions.poll_connection(
                &mut entry.connection,
                action_target,
                desktop_owned,
                catalog,
                ActionControllers {
                    animation,
                    behavior,
                    external_messages: match external_messages {
                        Some(ref mut sink) => Some(&mut **sink),
                        None => None,
                    },
                },
            );
        }
        entries.retain(|id, entry| {
            Self::collect_started_entry(*id, entry, observations);
            let Some(receipt) = entry.connection.termination() else {
                return true;
            };
            if !entry.remove {
                return true;
            }
            if let Some(registration) = entry.test_parent.and_then(|parent| tests.get_mut(&parent))
                && registration.active == Some(*id)
            {
                // Only an authoritative supervisor receipt makes this terminal.
                // An obsolete child cannot overwrite a replacement registration.
                registration.active = None;
                registration.status = Some(match (receipt.code, receipt.reason.as_str()) {
                    (Some(0), "Completed") => "Test process completed",
                    (_, "Hook timed out") => "Test process timed out",
                    (_, "Stopped" | "Stopped before startup") if !entry.enabled => {
                        "Test process cancelled"
                    }
                    (None, "Process failed") => "Test process terminated by signal",
                    _ => "Test process failed",
                });
            }
            false
        });
        Self::launch_hook(&mut inner, target, Instant::now());
    }
    fn collect_started_entry(
        id: Uuid,
        entry: &mut Managed,
        observations: &mut RuntimeObservations,
    ) {
        use crate::extension_lifecycle::LifecycleEvent;
        if entry.hook_target.is_some() || entry.test_parent.is_some() {
            return;
        }
        let Some(started) = entry.connection.spawn_evidence() else {
            return;
        };
        let mut terminal = entry.connection.terminal_evidence();
        // Worker completion alone is not an exit receipt. A real incarnation
        // whose supervisor finished without retained evidence contributes one
        // source loss, never an invented stopped event or arbitrary error text.
        if terminal.is_none() && entry.connection.supervisor_finished() {
            // Re-read after completion: publication may race the first read.
            terminal = entry.connection.terminal_evidence();
            if terminal.is_none() && !entry.terminal_loss_observed {
                entry.terminal_loss_observed = true;
                observations.dropped = observations.dropped.saturating_add(1);
            }
        }
        let request = entry
            .stop_request
            .filter(|(_, at)| terminal.as_ref().is_none_or(|end| *at <= end.observed_at));
        let installation_id = id;
        let session_id = entry.connection.session_id();
        let process_generation = entry.generation;
        if !entry.started_observed {
            entry.started_observed = true;
            observations.push(SpawnObservation {
                event: LifecycleEvent::ExtensionStarted {
                    installation_id,
                    session_id,
                    process_generation,
                },
                observed_at: started.observed_at,
            });
        }
        if !entry.stopping_observed
            && let Some((reason, at)) = request
        {
            entry.stopping_observed = true;
            observations.push(SpawnObservation {
                event: LifecycleEvent::ExtensionStopping {
                    installation_id,
                    session_id,
                    process_generation,
                    reason,
                },
                observed_at: at.max(started.observed_at),
            });
        }
        if !entry.stopped_observed
            && let Some(end) = terminal
        {
            entry.stopped_observed = true;
            observations.push(SpawnObservation {
                event: LifecycleEvent::ExtensionStopped {
                    installation_id,
                    session_id,
                    process_generation,
                    exit_code: end.code,
                    requested_reason: request.map(|(reason, _)| reason),
                },
                observed_at: end.observed_at,
            });
        }
    }

    /// Retained terminal evidence takes precedence over a late host request.
    /// A request before asynchronous spawn is held as intent only: if no OS
    /// spawn ever occurs it produces no incarnation events. Otherwise stopping
    /// follows started and retains the first request's reason.
    fn request_stop(
        inner: &mut Inner,
        id: Uuid,
        reason: crate::extension_protocol::ExtensionStopReason,
    ) {
        let Inner {
            entries,
            observations,
            ..
        } = inner;
        let Some(entry) = entries.get_mut(&id) else {
            return;
        };
        Self::collect_started_entry(id, entry, observations);
        if entry.hook_target.is_some()
            || entry.test_parent.is_some()
            || entry.stop_request.is_some()
        {
            return;
        }
        if let Some(at) = entry.connection.stop_request_boundary() {
            entry.stop_request = Some((reason, at));
        }
        Self::collect_started_entry(id, entry, observations);
    }

    fn collect_all_observations(inner: &mut Inner) {
        let Inner {
            entries,
            observations,
            ..
        } = inner;
        for (id, entry) in entries {
            Self::collect_started_entry(*id, entry, observations);
        }
    }

    /// Transfer bounded generic animation receipts to the same source journal
    /// path and subscription epochs as retained process observations.
    pub fn collect_motion_observations(&self, source: &crate::animation::MotionObservations) {
        let (events, lost) = source.take_events();
        let mut inner = self.inner();
        inner.observations.dropped_receipts =
            inner.observations.dropped_receipts.saturating_add(lost);
        for (event, observed_at) in events {
            inner
                .observations
                .push(SpawnObservation { event, observed_at });
        }
    }

    /// Drain actual persistent OS-spawn observations into the host-owned journal.
    /// This never assigns sequences itself and never infers a launch from enabled
    /// state or handshake admission. Fixed metadata is bounded to 32 records.
    pub fn drain_observations(&self, journal: &mut crate::extension_lifecycle::LifecycleJournal) {
        let mut inner = self.inner();
        let Inner {
            entries,
            observations,
            ..
        } = &mut *inner;
        for (id, entry) in entries.iter_mut() {
            Self::collect_started_entry(*id, entry, observations);
        }
        let mut pending: Vec<_> = inner.observations.pending.drain(..).collect();
        pending.sort_by_key(|observation| observation.observed_at);
        journal.note_source_loss(std::mem::take(&mut inner.observations.dropped));
        journal.note_receipt_loss(std::mem::take(&mut inner.observations.dropped_receipts));
        for observation in pending {
            if let Some(sequence) = journal.record(observation.event) {
                inner.last_started_sequence = inner.last_started_sequence.max(sequence);
                // Eligibility is per observation, not a retroactive sequence
                // floor: another collector may drain an older event later.
                inner.source_times.insert(sequence, observation.observed_at);
                while inner.source_times.len() > crate::extension_lifecycle::MAX_JOURNAL_RECORDS {
                    inner.source_times.pop_first();
                }
            }
        }
    }

    fn source_eligible(
        times: &BTreeMap<u64, Instant>,
        last_source: u64,
        sequence: u64,
        subscribed_at: Instant,
        floor: u64,
    ) -> bool {
        sequence > floor
            && match times.get(&sequence) {
                Some(at) => *at > subscribed_at,
                // Unknown historical records fail closed; synthetic/new sources
                // beyond the published watermark retain the existing test API.
                None => sequence > last_source,
            }
    }

    fn observe_run_identity(inner: &mut Inner, events: &[SequencedEvent]) {
        if inner.quiescing || inner.app_run_id.is_some() {
            return;
        }
        inner.app_run_id = events.iter().find_map(|source| match source.event {
            crate::extension_lifecycle::LifecycleEvent::AppReady { app_run_id }
                if !app_run_id.is_nil() =>
            {
                Some(app_run_id)
            }
            _ => None,
        });
    }

    /// Retain the real source identity even when no integration is installed yet.
    /// This observes metadata only; it never synthesizes ready or replays hooks.
    pub fn observe_app_run(&self, events: &[SequencedEvent]) {
        Self::observe_run_identity(&mut self.inner(), events);
    }

    fn quiesce(inner: &mut Inner) {
        if inner.quiescing {
            return;
        }
        let ids: Vec<_> = inner.entries.keys().copied().collect();
        for id in ids {
            Self::request_stop(
                inner,
                id,
                crate::extension_protocol::ExtensionStopReason::HostShutdown,
            );
        }
        inner.quiescing = true;
        inner.tests.clear();
        inner.current_target = None;
        inner.pending_hooks.clear();
        for hook in inner.hooks.values_mut() {
            hook.enabled = false;
        }
        for entry in inner.entries.values_mut() {
            let persistent = inner.app_run_id.is_some()
                && entry.enabled
                && !entry.restart
                && !entry.remove
                && entry.hook_target.is_none()
                && entry.test_parent.is_none();
            entry.enabled = false;
            entry.restart = false;
            entry.relay = EventRelay::default();
            entry.connection.begin_quiesce(persistent);
        }
    }

    /// Nonblocking world-accessible authority/effect revocation. Call before
    /// shutdown while controllers are live; tokens are cancelled, not forgotten.
    pub fn begin_shutdown(
        &self,
        animation: &mut crate::animation::AnimationState,
        behavior: &mut crate::behavior::BehaviorController,
    ) {
        let mut inner = self.inner();
        Self::quiesce(&mut inner);
        inner.actions.cancel_all(animation, behavior);
    }

    /// Transport teardown only: call after the app runner destroyed its world,
    /// OR after begin_shutdown cancelled live controller contributions. This
    /// method alone cannot cancel effects in controllers owned by a live world.
    /// One grace interval and force-stop-all share the supplied total deadline.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner();
        Self::quiesce(&mut inner);
        let mut grace_deadline = Instant::now();
        if !inner.stopping_offered {
            inner.stopping_offered = true;
            if let Some(app_run_id) = inner.app_run_id {
                grace_deadline = (Instant::now() + MAX_SHUTDOWN_GRACE).min(deadline);
                for entry in inner.entries.values_mut() {
                    entry.connection.offer_stopping(app_run_id, grace_deadline);
                }
            }
        }
        drop(inner);
        while Instant::now() < grace_deadline {
            let mut inner = self.inner();
            let mut complete = true;
            for entry in inner.entries.values_mut() {
                // Reap every child, even when an earlier one is still running.
                complete &= entry.connection.reap_shutdown();
            }
            Self::collect_all_observations(&mut inner);
            if complete {
                return true;
            }
            drop(inner);
            std::thread::sleep(
                Duration::from_millis(1)
                    .min(grace_deadline.saturating_duration_since(Instant::now())),
            );
        }
        // Signal every survivor BEFORE waiting for any single supervisor.
        let mut inner = self.inner();
        for entry in inner.entries.values_mut() {
            entry.connection.stop("Host shutdown cutoff");
        }
        drop(inner);
        loop {
            let mut inner = self.inner();
            let mut complete = true;
            for entry in inner.entries.values_mut() {
                // Reap every child, even when an earlier one is still running.
                complete &= entry.connection.reap_shutdown();
            }
            Self::collect_all_observations(&mut inner);
            if complete || Instant::now() >= deadline {
                return complete;
            }
            drop(inner);
            std::thread::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "extension_started_tests.rs"]
mod started_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "extension_shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn selection(sequence: u64, previous: Option<Uuid>, new: Option<Uuid>) -> SequencedEvent {
        SequencedEvent {
            sequence,
            event: crate::extension_lifecycle::LifecycleEvent::CharacterSelected {
                previous,
                new,
                selection_revision: sequence,
            },
        }
    }

    fn register_selection(runtime: &ExtensionRuntime, id: Uuid) {
        register_metadata_hook(runtime, id, HookTrigger::CharacterSelected);
    }

    fn register_metadata_hook(runtime: &ExtensionRuntime, id: Uuid, trigger: HookTrigger) {
        runtime
            .configure_hooks(
                id,
                IntegrationOrigin {
                    integration_id: "org.example.selection-test".into(),
                    display_name: "Selection test".into(),
                },
                [
                    Capability::LifecycleObserve,
                    Capability::ExpressionApply,
                    Capability::ReplyRead,
                ],
                Default::default(),
                vec![HookProcess {
                    trigger,
                    // Capture-only tests never launch this process.
                    process: ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec![],
                        working_directory: std::env::temp_dir(),
                        data_directory: std::env::temp_dir(),
                        lifetime: None,
                    },
                }],
            )
            .unwrap();
    }

    fn incarnation_source(trigger: HookTrigger) -> crate::extension_lifecycle::LifecycleEvent {
        use crate::extension_lifecycle::{ExtensionStopReason, LifecycleEvent};
        let installation_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        match trigger {
            HookTrigger::MotionPlaybackEnded => LifecycleEvent::MotionPlaybackEnded {
                playback_id: Uuid::new_v4(),
                source_target: None,
                motion_id: None,
                source: crate::extension_protocol::MotionPlaybackSource::Host,
                outcome: crate::extension_protocol::MotionOutcome::Cancelled,
            },
            HookTrigger::ExtensionStopping => LifecycleEvent::ExtensionStopping {
                installation_id,
                session_id,
                process_generation: 1,
                reason: ExtensionStopReason::Removed,
            },
            HookTrigger::ExtensionStopped => LifecycleEvent::ExtensionStopped {
                installation_id,
                session_id,
                process_generation: 1,
                exit_code: Some(7),
                requested_reason: None,
            },
            _ => started_source(),
        }
    }

    fn started_source() -> crate::extension_lifecycle::LifecycleEvent {
        crate::extension_lifecycle::LifecycleEvent::ExtensionStarted {
            installation_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            process_generation: 1,
        }
    }

    #[test]
    fn actual_motion_terminals_do_not_replay_to_late_hooks_or_launch_after_quiescence() {
        let runtime = ExtensionRuntime::default();
        let mut animation = crate::animation::AnimationState::default();
        let source = animation.motion_observations();
        let clip = std::sync::Arc::new(
            crate::animation::parse_bvh(include_str!(
                "../assets/animations/companion/talking-nod.bvh"
            ))
            .unwrap(),
        );
        animation.select_clip(clip.clone(), None);
        animation.stop();
        let id = Uuid::new_v4();
        register_metadata_hook(&runtime, id, HookTrigger::MotionPlaybackEnded);
        let mut journal = crate::extension_lifecycle::LifecycleJournal::default();
        runtime.collect_motion_observations(&source);
        runtime.drain_observations(&mut journal);
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            &journal.drain(),
            None,
            Instant::now(),
        );
        assert!(
            runtime.inner().pending_hooks.is_empty(),
            "historical terminal replayed to late subscription"
        );
        animation.select_clip(clip.clone(), None);
        animation.stop();
        runtime.collect_motion_observations(&source);
        runtime.drain_observations(&mut journal);
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            &journal.drain(),
            None,
            Instant::now(),
        );
        assert_eq!(runtime.inner().pending_hooks.len(), 1);
        let mut behavior = crate::behavior::BehaviorController::default();
        runtime.begin_shutdown(&mut animation, &mut behavior);
        animation.select_clip(clip, None);
        animation.stop();
        runtime.collect_motion_observations(&source);
        runtime.drain_observations(&mut journal);
        runtime.frame(
            &journal.drain(),
            None,
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        assert!(runtime.inner().pending_hooks.is_empty());
        assert_eq!(runtime.process_count(), 0);
    }

    #[test]
    fn actual_motion_activations_do_not_replay_to_late_hooks_or_launch_after_quiescence() {
        let runtime = ExtensionRuntime::default();
        let mut animation = crate::animation::AnimationState::default();
        let source = animation.motion_observations();
        let clip = std::sync::Arc::new(
            crate::animation::parse_bvh(include_str!(
                "../assets/animations/companion/talking-nod.bvh"
            ))
            .unwrap(),
        );
        animation.select_clip(clip.clone(), None);
        animation.stop();
        let id = Uuid::new_v4();
        register_metadata_hook(&runtime, id, HookTrigger::MotionPlaybackStarted);
        let mut journal = crate::extension_lifecycle::LifecycleJournal::default();
        runtime.collect_motion_observations(&source);
        runtime.drain_observations(&mut journal);
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            &journal.drain(),
            None,
            Instant::now(),
        );
        assert!(
            runtime.inner().pending_hooks.is_empty(),
            "historical activation replayed to late subscription"
        );
        animation.select_clip(clip.clone(), None);
        animation.stop();
        runtime.collect_motion_observations(&source);
        runtime.drain_observations(&mut journal);
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            &journal.drain(),
            None,
            Instant::now(),
        );
        assert_eq!(runtime.inner().pending_hooks.len(), 1);
        let mut behavior = crate::behavior::BehaviorController::default();
        runtime.begin_shutdown(&mut animation, &mut behavior);
        animation.select_clip(clip, None);
        animation.stop();
        runtime.collect_motion_observations(&source);
        runtime.drain_observations(&mut journal);
        runtime.frame(
            &journal.drain(),
            None,
            false,
            &ActionCatalog::from_packs([], &[]),
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        assert!(runtime.inner().pending_hooks.is_empty());
        assert_eq!(runtime.process_count(), 0);
    }

    #[test]
    fn mixed_source_delayed_old_event_cannot_hide_newer_queued_eligible_event() {
        for newer_motion in [false, true] {
            let runtime = ExtensionRuntime::default();
            let old = Instant::now();
            let id = Uuid::new_v4();
            register_metadata_hook(&runtime, id, HookTrigger::ExtensionStarted);
            {
                let mut inner = runtime.inner();
                let hook = inner.hooks.get_mut(&id).unwrap();
                let mut handler = hook.hooks[0].clone();
                handler.trigger = HookTrigger::MotionPlaybackEnded;
                hook.hooks.push(handler);
            }
            let new_trigger = if newer_motion {
                HookTrigger::MotionPlaybackEnded
            } else {
                HookTrigger::ExtensionStarted
            };
            let old_trigger = if newer_motion {
                HookTrigger::ExtensionStarted
            } else {
                HookTrigger::MotionPlaybackEnded
            };
            let eligible = incarnation_source(new_trigger);
            runtime.inner().observations.push(SpawnObservation {
                event: eligible.clone(),
                observed_at: Instant::now() + Duration::from_millis(1),
            });
            let mut journal = crate::extension_lifecycle::LifecycleJournal::default();
            runtime.drain_observations(&mut journal);
            // Deliberately delay delivery and drain a historical other-source
            // observation AFTER the eligible record already owns sequence 1.
            runtime.inner().observations.push(SpawnObservation {
                event: incarnation_source(old_trigger),
                observed_at: old,
            });
            runtime.drain_observations(&mut journal);
            let events = journal.drain();
            assert_eq!(events.len(), 2);
            ExtensionRuntime::capture_hooks(&mut runtime.inner(), &events, None, Instant::now());
            let inner = runtime.inner();
            assert_eq!(inner.pending_hooks.len(), 1);
            assert_eq!(inner.pending_hooks[0].captured.event(), &eligible);
        }
    }

    #[test]
    fn started_subscription_reset_rejects_delayed_sources_across_two_drains() {
        for trigger in [
            HookTrigger::MotionPlaybackEnded,
            HookTrigger::ExtensionStarted,
            HookTrigger::ExtensionStopping,
            HookTrigger::ExtensionStopped,
        ] {
            for restart in [false, true] {
                let runtime = ExtensionRuntime::default();
                let id = Uuid::new_v4();
                register_metadata_hook(&runtime, id, trigger);
                let mut journal = crate::extension_lifecycle::LifecycleJournal::default();
                runtime.inner().observations.push(SpawnObservation {
                    event: incarnation_source(trigger),
                    observed_at: Instant::now(),
                });
                runtime.drain_observations(&mut journal);
                if restart {
                    runtime.stop(id);
                    runtime.restart(id);
                } else {
                    runtime.set_grants(id, []);
                    runtime.set_grants(id, [Capability::LifecycleObserve]);
                }
                // Another delayed receipt predating the refreshed eligibility epoch.
                let old = runtime.inner().hooks[&id].subscribed_at - Duration::from_millis(1);
                runtime.inner().observations.push(SpawnObservation {
                    event: incarnation_source(trigger),
                    observed_at: old,
                });
                runtime.drain_observations(&mut journal);
                let events = journal.drain();
                ExtensionRuntime::capture_hooks(
                    &mut runtime.inner(),
                    &events,
                    None,
                    Instant::now(),
                );
                assert!(runtime.inner().pending_hooks.is_empty());
                runtime.inner().observations.push(SpawnObservation {
                    event: incarnation_source(trigger),
                    observed_at: Instant::now() + Duration::from_millis(1),
                });
                runtime.drain_observations(&mut journal);
                ExtensionRuntime::capture_hooks(
                    &mut runtime.inner(),
                    &journal.drain(),
                    None,
                    Instant::now(),
                );
                assert_eq!(runtime.inner().pending_hooks.len(), 1);
                assert!(runtime.inner().pending_hooks[0].captured.metadata_only());
                assert_eq!(runtime.inner().pending_hooks[0].target, None);
            }
        }
    }

    #[test]
    fn started_source_buffer_is_bounded_and_reports_loss_in_host_journal() {
        let runtime = ExtensionRuntime::default();
        for index in 0..MAX_RUNTIME_OBSERVATIONS + 3 {
            runtime.inner().observations.push(SpawnObservation {
                event: incarnation_source(if index % 2 == 0 {
                    HookTrigger::ExtensionStopping
                } else {
                    HookTrigger::ExtensionStopped
                }),
                observed_at: Instant::now(),
            });
        }
        assert_eq!(
            runtime.inner().observations.pending.len(),
            MAX_RUNTIME_OBSERVATIONS
        );
        let mut journal = crate::extension_lifecycle::LifecycleJournal::default();
        runtime.drain_observations(&mut journal);
        let events = journal.drain();
        assert_eq!(events.len(), MAX_RUNTIME_OBSERVATIONS + 1);
        assert!(matches!(
            events.last().unwrap().event,
            crate::extension_lifecycle::LifecycleEvent::Gap {
                dropped_records: 3,
                ..
            }
        ));
        runtime.drain_observations(&mut journal);
        assert!(journal.drain().is_empty());
    }

    #[test]
    fn app_ready_queue_survives_selection_and_avatar_changes_but_never_replays() {
        let runtime = ExtensionRuntime::default();
        let id = Uuid::new_v4();
        register_metadata_hook(&runtime, id, HookTrigger::AppReady);
        let event = SequencedEvent {
            sequence: 1,
            event: crate::extension_lifecycle::LifecycleEvent::AppReady {
                app_run_id: Uuid::new_v4(),
            },
        };
        let target = Some(Target {
            uuid: Uuid::new_v4(),
            generation: 1,
        });
        let now = Instant::now();
        {
            let mut inner = runtime.inner();
            ExtensionRuntime::capture_hooks(&mut inner, std::slice::from_ref(&event), target, now);
            assert_eq!(inner.pending_hooks.len(), 1);
            assert_eq!(inner.pending_hooks[0].target, None);
            assert!(inner.pending_hooks[0].captured.metadata_only());
            assert_eq!(
                inner.pending_hooks[0].captured.grants(),
                &BTreeSet::from([Capability::LifecycleObserve])
            );
            ExtensionRuntime::capture_hooks(
                &mut inner,
                &[selection(2, None, Some(Uuid::new_v4()))],
                None,
                now,
            );
            assert_eq!(
                inner.pending_hooks.len(),
                1,
                "selection discarded startup metadata"
            );
            ExtensionRuntime::capture_hooks(&mut inner, &[], target, now);
            assert_eq!(inner.pending_hooks.len(), 1);
            inner.pending_hooks.clear();
        }
        runtime.restart(id);
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            std::slice::from_ref(&event),
            target,
            now,
        );
        assert!(runtime.inner().pending_hooks.is_empty());
        let late = Uuid::new_v4();
        register_metadata_hook(&runtime, late, HookTrigger::AppReady);
        // A fresh source sequence cannot turn historical startup into a new edge.
        let mut replay = event;
        replay.sequence = 3;
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[replay], target, now);
        assert!(runtime.inner().pending_hooks.is_empty());
    }

    #[test]
    fn app_ready_admission_retains_expiry_grant_and_queue_bounds() {
        for revoke in [false, true] {
            let runtime = ExtensionRuntime::default();
            let id = Uuid::new_v4();
            register_metadata_hook(&runtime, id, HookTrigger::AppReady);
            let now = Instant::now();
            let event = SequencedEvent {
                sequence: 1,
                event: crate::extension_lifecycle::LifecycleEvent::AppReady {
                    app_run_id: Uuid::new_v4(),
                },
            };
            ExtensionRuntime::capture_hooks(
                &mut runtime.inner(),
                std::slice::from_ref(&event),
                None,
                now,
            );
            if revoke {
                runtime.set_grants(id, []);
                runtime.set_grants(id, [Capability::LifecycleObserve]);
            } else {
                ExtensionRuntime::capture_hooks(
                    &mut runtime.inner(),
                    &[],
                    None,
                    now + HOOK_QUEUE_AGE,
                );
            }
            assert!(runtime.inner().pending_hooks.is_empty());
            ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[event], None, now);
            assert!(
                runtime.inner().pending_hooks.is_empty(),
                "lost startup was replayed"
            );
        }
        let runtime = ExtensionRuntime::default();
        for _ in 0..=MAX_HOOK_QUEUE {
            register_metadata_hook(&runtime, Uuid::new_v4(), HookTrigger::AppReady);
        }
        let now = Instant::now();
        let event = SequencedEvent {
            sequence: 1,
            event: crate::extension_lifecycle::LifecycleEvent::AppReady {
                app_run_id: Uuid::new_v4(),
            },
        };
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[event], None, now);
        let inner = runtime.inner();
        assert_eq!(inner.pending_hooks.len(), MAX_HOOK_QUEUE);
        assert_eq!(
            inner
                .hooks
                .values()
                .map(|hook| hook.dropped_invocations)
                .sum::<u64>(),
            1
        );
    }

    #[test]
    fn selection_queue_is_metadata_only_survives_target_changes_and_supersedes_without_replay() {
        let runtime = ExtensionRuntime::default();
        let id = Uuid::new_v4();
        register_selection(&runtime, id);
        let a_id = Uuid::from_u128(1);
        let a = Some(a_id);
        let b = Some(Uuid::from_u128(2));
        let old_target = Some(Target {
            uuid: a_id,
            generation: 1,
        });
        let now = Instant::now();
        let mut inner = runtime.inner();
        ExtensionRuntime::capture_hooks(&mut inner, &[selection(1, None, a)], old_target, now);
        assert_eq!(inner.pending_hooks.len(), 1);
        assert_eq!(inner.pending_hooks[0].target, None);
        assert_eq!(
            inner.pending_hooks[0].captured.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        ExtensionRuntime::capture_hooks(&mut inner, &[], None, now);
        assert_eq!(
            inner.pending_hooks.len(),
            1,
            "avatar loss must not discard selection metadata"
        );
        let latest = selection(3, b, a);
        ExtensionRuntime::capture_hooks(
            &mut inner,
            &[selection(2, a, b), latest.clone()],
            old_target,
            now,
        );
        assert_eq!(inner.pending_hooks.len(), 1);
        assert_eq!(
            inner.pending_hooks[0].captured.selection_revision(),
            Some(3)
        );
        inner.pending_hooks.clear();
        ExtensionRuntime::capture_hooks(&mut inner, &[latest], old_target, now);
        assert!(
            inner.pending_hooks.is_empty(),
            "replayed source sequence launched a hook"
        );
        // Even a fresh source sequence cannot replay an already-seen revision.
        let mut repeated = selection(3, b, a);
        repeated.sequence = 4;
        ExtensionRuntime::capture_hooks(&mut inner, &[repeated], old_target, now);
        assert!(inner.pending_hooks.is_empty());
        drop(inner);
        runtime.restart(id);
        assert!(runtime.inner().pending_hooks.is_empty());
    }

    #[test]
    fn selection_queue_revocation_expiry_and_overflow_keep_existing_bounds() {
        let runtime = ExtensionRuntime::default();
        let id = Uuid::new_v4();
        register_selection(&runtime, id);
        let event = selection(1, None, Some(Uuid::from_u128(1)));
        let now = Instant::now();
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[event], None, now);
        runtime.set_grants(id, []);
        runtime.set_grants(id, [Capability::LifecycleObserve]);
        assert!(runtime.inner().pending_hooks.is_empty());
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            &[selection(2, Some(Uuid::from_u128(1)), None)],
            None,
            now,
        );
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[], None, now + HOOK_QUEUE_AGE);
        assert!(runtime.inner().pending_hooks.is_empty());
        for _ in 0..MAX_HOOK_QUEUE {
            register_selection(&runtime, Uuid::new_v4());
        }
        ExtensionRuntime::capture_hooks(
            &mut runtime.inner(),
            &[selection(3, None, Some(Uuid::from_u128(2)))],
            None,
            now,
        );
        let inner = runtime.inner();
        assert_eq!(inner.pending_hooks.len(), MAX_HOOK_QUEUE);
        assert_eq!(
            inner
                .hooks
                .values()
                .map(|hook| hook.dropped_invocations)
                .sum::<u64>(),
            2
        );
    }

    #[test]
    fn running_selection_hook_never_gains_avatar_authority_and_new_revision_cancels_session() {
        let runtime = ExtensionRuntime::default();
        let root = std::env::temp_dir().join(format!("selection-authority-{}", Uuid::new_v4()));
        struct Cleanup(ExtensionRuntime, std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown(Duration::from_secs(2));
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _cleanup = Cleanup(runtime.clone(), root.clone());
        let id = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let target = Target {
            uuid: a,
            generation: 1,
        };
        let script = r#"
import json, os, sys
from pathlib import Path
h = json.loads(sys.stdin.readline())
assert h['message']['capabilities'] == ['lifecycle.observe']
data = Path(os.environ['WAIFU_EXTENSION_DATA'])
event = None
for line in sys.stdin:
    packet = json.loads(line)
    m = packet['message']
    if m['type'] == 'event':
        e = m['event']
        if e['type'] == 'target.changed':
            assert e['target'] is None
        else:
            assert e['type'] == 'character.selected'
            event = e
            print(json.dumps({'api_version':1,'type':'action','id':'forbidden','sequence':1,
                'session_id':h['session_id'],'process_generation':h['process_generation'],
                'target':__TARGET__, 'action':{'type':'expression.apply','name':'happy','strength':0.5,'duration_ms':1000}}), flush=True)
    elif m['type'] == 'result':
        assert m['result']['status'] == 'error'
        path = data / ('receipt-' + str(h['process_generation']) + '.json')
        temp = path.with_suffix('.tmp')
        temp.write_text(json.dumps(event))
        temp.replace(path)
"#.replace("__TARGET__", &serde_json::to_string(&target).unwrap());
        runtime
            .configure_hooks(
                id,
                IntegrationOrigin {
                    integration_id: "org.example.selection-authority".into(),
                    display_name: "Selection authority test".into(),
                },
                [
                    Capability::LifecycleObserve,
                    Capability::ExpressionApply,
                    Capability::ReplyRead,
                ],
                Default::default(),
                vec![HookProcess {
                    trigger: HookTrigger::CharacterSelected,
                    process: ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec!["-u".into(), "-c".into(), script],
                        working_directory: root.clone(),
                        data_directory: root.join("data"),
                        lifetime: None,
                    },
                }],
            )
            .unwrap();
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        let names = vec!["happy".into()];
        behavior
            .select(Some(a), &BehaviorConfig::default(), &names)
            .unwrap();
        let catalog = ActionCatalog::from_packs([], &names);
        let mut frame = |events: &[SequencedEvent], target| {
            runtime.frame(
                events,
                target,
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
        };
        frame(&[selection(1, Some(a), Some(b))], Some(target));
        let old_session = runtime.inner().entries[&id].connection.session_id();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut available = false;
        for (generation, expected_revision) in [(1, 1), (2, 2)] {
            let receipt = root.join(format!("data/receipt-{generation}.json"));
            loop {
                available = !available;
                frame(&[], if available { Some(target) } else { None });
                if let Some(value) = std::fs::read(&receipt)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                {
                    assert_eq!(value["selection_revision"], expected_revision);
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "metadata hook failed: {:?}",
                    runtime.views()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            if generation == 1 {
                frame(&[selection(2, Some(b), None)], Some(target));
                let inner = runtime.inner();
                let entry = &inner.entries[&id];
                if entry.connection.session_id() == old_session {
                    assert!(
                        !entry.enabled,
                        "obsolete running selection was not cancelled"
                    );
                    assert!(!entry.connection.is_active());
                } else {
                    // A fast supervisor may already have reaped/replaced it in
                    // this frame; only the latest selection may own that slot.
                    assert_eq!(entry.selection_revision, Some(2));
                }
            }
        }
        assert_ne!(
            runtime.inner().entries[&id].connection.session_id(),
            old_session
        );
        assert_eq!(runtime.inner().entries[&id].selection_revision, Some(2));
        assert_eq!(runtime.inner().entries[&id].sent_target, Some(None));
        assert!(
            behavior
                .weights_for(Some(a), &names, &BTreeMap::new(), "")
                .is_empty()
        );
    }

    struct ManualTestFixture {
        runtime: ExtensionRuntime,
        root: std::path::PathBuf,
        parent: Uuid,
        target: Target,
        origin: IntegrationOrigin,
    }

    impl ManualTestFixture {
        fn new() -> Self {
            let fixture = Self {
                runtime: ExtensionRuntime::default(),
                root: std::env::temp_dir().join(format!("waifu-manual-test-{}", Uuid::new_v4())),
                parent: Uuid::new_v4(),
                target: Target {
                    uuid: Uuid::new_v4(),
                    generation: 1,
                },
                origin: IntegrationOrigin {
                    integration_id: "org.example.manual-test".into(),
                    display_name: "Manual test fixture".into(),
                },
            };
            std::fs::create_dir(&fixture.root).unwrap();
            fixture
                .runtime
                .configure_hooks(
                    fixture.parent,
                    fixture.origin.clone(),
                    [Capability::LifecycleObserve, Capability::ExpressionApply],
                    Default::default(),
                    vec![HookProcess {
                        trigger: HookTrigger::ReplyFinished,
                        process: fixture.spec("import sys\nfor line in sys.stdin: pass"),
                    }],
                )
                .unwrap();
            fixture
        }

        fn spec(&self, script: &str) -> ProcessSpec {
            ProcessSpec {
                program: "/usr/bin/python3".into(),
                args: vec!["-u".into(), "-c".into(), script.into()],
                working_directory: self.root.clone(),
                data_directory: self.root.join("data"),
                lifetime: None,
            }
        }

        fn configure(&self, script: &str) {
            self.runtime
                .configure_test(
                    self.parent,
                    self.spec(script),
                    self.origin.clone(),
                    [Capability::LifecycleObserve, Capability::ExpressionApply],
                    Default::default(),
                )
                .unwrap();
        }

        fn frame(&self, target: Option<Target>) {
            self.runtime.frame(
                &[],
                target,
                false,
                &ActionCatalog::from_packs([], &[]),
                ActionControllers {
                    animation: &mut AnimationState::default(),
                    behavior: &mut BehaviorController::default(),
                    external_messages: None,
                },
            );
        }

        fn reap(&self) {
            let deadline = Instant::now() + Duration::from_secs(4);
            while self.runtime.test_active(self.parent) {
                self.frame(Some(self.target));
                assert!(Instant::now() < deadline, "manual test did not reap");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    impl Drop for ManualTestFixture {
        fn drop(&mut self) {
            self.runtime.shutdown(Duration::from_secs(2));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn manual_test_requires_registration_enabled_parent_and_latest_loaded_target() {
        let fixture = ManualTestFixture::new();
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        fixture.configure("import sys\nfor line in sys.stdin: pass");
        assert_eq!(
            fixture.runtime.process_count(),
            0,
            "registration spawned code"
        );
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        assert!(fixture.runtime.test_status(fixture.parent).is_none());
        fixture.frame(Some(fixture.target));
        let child = fixture.runtime.start_test(fixture.parent).unwrap();
        assert_ne!(child, fixture.parent);
        assert_eq!(fixture.runtime.views()[0].test_parent, Some(fixture.parent));
        assert_eq!(
            fixture.runtime.inner().entries[&child].hook_target,
            Some(Some(fixture.target))
        );
        assert_eq!(
            fixture.runtime.inner().entries[&child].spec.lifetime,
            Some(TEST_LIFETIME)
        );
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        fixture.runtime.stop(fixture.parent);
        assert!(
            fixture.runtime.test_active(fixture.parent),
            "stop must await reaping"
        );
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        assert!(fixture.runtime.test_status(fixture.parent).is_none());
        fixture.reap();
        assert!(
            fixture
                .runtime
                .configure_test(
                    fixture.parent,
                    fixture.spec("pass"),
                    fixture.origin.clone(),
                    [],
                    Default::default(),
                )
                .is_err()
        );
    }

    #[test]
    fn manual_test_retains_authoritative_process_result_after_reaping() {
        let fixture = ManualTestFixture::new();
        fixture.frame(Some(fixture.target));
        for (script, expected) in [
            (
                "import sys\nsys.stdin.readline()\nsys.exit(0)",
                "Test process completed",
            ),
            (
                "import sys\nsys.stdin.readline()\nsys.exit(7)",
                "Test process failed",
            ),
            (
                "import os, signal, sys\nsys.stdin.readline()\nos.kill(os.getpid(), signal.SIGTERM)",
                "Test process terminated by signal",
            ),
        ] {
            fixture.configure(script);
            assert!(fixture.runtime.test_status(fixture.parent).is_none());
            fixture.runtime.start_test(fixture.parent).unwrap();
            // Starting or an action ACK cannot be mistaken for a terminal result.
            assert_eq!(
                fixture.runtime.test_status(fixture.parent).as_deref(),
                Some("Test process starting")
            );
            fixture.reap();
            assert_eq!(fixture.runtime.process_count(), 0);
            assert_eq!(
                fixture.runtime.test_status(fixture.parent).as_deref(),
                Some(expected)
            );
            fixture.frame(Some(fixture.target));
            assert_eq!(
                fixture.runtime.test_status(fixture.parent).as_deref(),
                Some(expected)
            );
        }
        let mut spec = fixture.spec("import sys\nfor line in sys.stdin: pass");
        spec.lifetime = Some(Duration::from_millis(50));
        fixture
            .runtime
            .configure_test(
                fixture.parent,
                spec,
                fixture.origin.clone(),
                [],
                Default::default(),
            )
            .unwrap();
        fixture.runtime.start_test(fixture.parent).unwrap();
        fixture.reap();
        assert_eq!(
            fixture.runtime.test_status(fixture.parent).as_deref(),
            Some("Test process timed out")
        );
        fixture.runtime.remove(fixture.parent);
        assert!(fixture.runtime.test_status(fixture.parent).is_none());
    }

    #[test]
    fn manual_test_restart_replacement_revocation_and_target_changes_do_not_retry() {
        let fixture = ManualTestFixture::new();
        let hold = "import sys\nfor line in sys.stdin: pass";
        fixture.configure(hold);
        fixture.frame(Some(fixture.target));
        let first = fixture.runtime.start_test(fixture.parent).unwrap();
        let session = fixture.runtime.inner().entries[&first]
            .connection
            .session_id();
        fixture.runtime.restart(fixture.parent);
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        fixture.reap();
        assert_eq!(
            fixture.runtime.test_status(fixture.parent).as_deref(),
            Some("Test process cancelled")
        );
        let second = fixture.runtime.start_test(fixture.parent).unwrap();
        assert_ne!(
            fixture.runtime.inner().entries[&second]
                .connection
                .session_id(),
            session
        );
        fixture
            .runtime
            .set_grants(fixture.parent, [Capability::LifecycleObserve]);
        fixture.runtime.set_grants(
            fixture.parent,
            [Capability::LifecycleObserve, Capability::ExpressionApply],
        );
        assert!(
            !fixture.runtime.inner().entries[&second]
                .grants
                .contains(&Capability::ExpressionApply)
        );
        fixture.reap();
        assert_eq!(
            fixture.runtime.test_status(fixture.parent).as_deref(),
            Some("Test process cancelled")
        );

        fixture.runtime.start_test(fixture.parent).unwrap();
        fixture.configure(hold);
        assert!(fixture.runtime.test_status(fixture.parent).is_none());
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        fixture.reap();
        assert!(
            fixture.runtime.test_status(fixture.parent).is_none(),
            "old child overwrote new configuration"
        );
        fixture.runtime.start_test(fixture.parent).unwrap();
        fixture.frame(Some(Target {
            generation: 2,
            ..fixture.target
        }));
        fixture.reap();
        assert_eq!(
            fixture.runtime.test_status(fixture.parent).as_deref(),
            Some("Test process cancelled")
        );
        assert_eq!(
            fixture.runtime.process_count(),
            0,
            "cancelled test retried automatically"
        );
        fixture.runtime.remove(fixture.parent);
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
    }

    #[test]
    fn manual_tests_share_four_process_budget_with_persistent_parent() {
        let fixture = ManualTestFixture::new();
        fixture.runtime.remove(fixture.parent);
        let hold = "import sys\nfor line in sys.stdin: pass";
        fixture
            .runtime
            .start(
                fixture.parent,
                fixture.spec(hold),
                fixture.origin.clone(),
                [Capability::LifecycleObserve, Capability::ExpressionApply],
            )
            .unwrap();
        fixture.configure(hold);
        fixture.frame(Some(fixture.target));
        for _ in 0..3 {
            fixture
                .runtime
                .start(
                    Uuid::new_v4(),
                    fixture.spec(hold),
                    fixture.origin.clone(),
                    [],
                )
                .unwrap();
        }
        assert!(fixture.runtime.start_test(fixture.parent).is_err());
        assert!(fixture.runtime.test_status(fixture.parent).is_none());
        let other = *fixture
            .runtime
            .inner()
            .entries
            .keys()
            .find(|id| **id != fixture.parent)
            .unwrap();
        fixture.runtime.remove(other);
        let deadline = Instant::now() + Duration::from_secs(4);
        while fixture.runtime.process_count() == MAX_EXTENSIONS {
            fixture.frame(Some(fixture.target));
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !fixture.runtime.test_active(fixture.parent),
            "failed start was queued"
        );
        fixture.runtime.start_test(fixture.parent).unwrap();
        assert_eq!(fixture.runtime.process_count(), MAX_EXTENSIONS);
        assert!(fixture.runtime.inner().entries[&fixture.parent].enabled);
    }

    #[test]
    fn public_frame_keeps_overflow_visible_after_launch_and_reset_until_removal() {
        let runtime = ExtensionRuntime::default();
        let root = std::env::temp_dir().join(format!("waifu-hook-loss-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        struct Cleanup(ExtensionRuntime, std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown(Duration::from_secs(2));
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _cleanup = Cleanup(runtime.clone(), root.clone());
        let id = Uuid::new_v4();
        let origin = IntegrationOrigin {
            integration_id: "org.example.loss".into(),
            display_name: "Loss test".into(),
        };
        let hooks = vec![HookProcess {
            trigger: HookTrigger::ReplyFinished,
            process: ProcessSpec {
                program: "/usr/bin/python3".into(),
                args: vec![
                    "-u".into(),
                    "-c".into(),
                    "import sys\nfor line in sys.stdin: pass".into(),
                ],
                working_directory: root.clone(),
                data_directory: root.join("data"),
                lifetime: None,
            },
        }];
        runtime
            .configure_hooks(
                id,
                origin.clone(),
                [Capability::LifecycleObserve],
                Default::default(),
                hooks.clone(),
            )
            .unwrap();
        let events: Vec<_> = (1..=33)
            .map(|sequence| SequencedEvent {
                sequence,
                event: LifecycleEvent::ReplyFinished {
                    reply_id: Uuid::new_v4(),
                    character: None,
                    outcome: ReplyOutcome::Completed {
                        text: String::new(),
                    },
                },
            })
            .collect();
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        let catalog = ActionCatalog::from_packs([], &[]);
        runtime.frame(
            &events,
            None,
            false,
            &catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        assert_eq!(runtime.process_count(), 1, "free slot must launch a hook");
        let view = runtime.views().remove(0);
        assert!(
            view.status.contains("Invocation started"),
            "{}",
            view.status
        );
        assert!(
            view.status.contains("Dropped hook invocations: 1"),
            "{}",
            view.status
        );
        assert!(
            view.status
                .contains("Last loss: Hook event dropped: queue limit reached"),
            "{}",
            view.status
        );

        // Reset cancels the remaining queue and active child without clearing
        // or inflating the recorded overflow count.
        runtime.restart(id);
        let deadline = Instant::now() + Duration::from_secs(4);
        while runtime.process_count() != 0 {
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
            assert!(Instant::now() < deadline, "hook did not terminate");
            std::thread::sleep(Duration::from_millis(2));
        }
        let view = runtime.views().remove(0);
        assert!(!view.active);
        assert!(
            view.status.contains("Dropped hook invocations: 1"),
            "{}",
            view.status
        );
        assert!(
            view.status.contains("queue limit reached"),
            "{}",
            view.status
        );
        runtime.remove(id);
        runtime
            .configure_hooks(
                id,
                origin,
                [Capability::LifecycleObserve],
                Default::default(),
                hooks,
            )
            .unwrap();
        assert!(
            !runtime.views()[0]
                .status
                .contains("Dropped hook invocations")
        );
    }

    #[test]
    fn queued_hooks_are_bounded_expire_and_never_regain_revoked_payload_or_control() {
        let runtime = ExtensionRuntime::default();
        let id = Uuid::new_v4();
        runtime
            .configure_hooks(
                id,
                IntegrationOrigin {
                    integration_id: "org.example.queued".into(),
                    display_name: "Queue test".into(),
                },
                [
                    Capability::LifecycleObserve,
                    Capability::ReplyRead,
                    Capability::MotionPlay,
                ],
                Default::default(),
                vec![HookProcess {
                    trigger: HookTrigger::ReplyFinished,
                    process: ProcessSpec {
                        program: "unstarted".into(),
                        args: vec![],
                        working_directory: ".".into(),
                        data_directory: ".".into(),
                        lifetime: None,
                    },
                }],
            )
            .unwrap();
        let now = Instant::now();
        let events: Vec<_> = (1..=40)
            .map(|sequence| SequencedEvent {
                sequence,
                event: LifecycleEvent::ReplyFinished {
                    reply_id: Uuid::new_v4(),
                    character: None,
                    outcome: ReplyOutcome::Completed {
                        text: "private queued text".into(),
                    },
                },
            })
            .collect();
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &events, None, now);
        assert_eq!(runtime.inner().pending_hooks.len(), MAX_HOOK_QUEUE);
        assert_eq!(runtime.inner().hooks[&id].dropped_invocations, 8);
        assert!(
            runtime
                .inner()
                .pending_hooks
                .iter()
                .map(ExtensionRuntime::invocation_bytes)
                .sum::<usize>()
                <= MAX_HOOK_BYTES
        );
        runtime.set_grants(id, [Capability::LifecycleObserve]);
        runtime.set_grants(
            id,
            [
                Capability::LifecycleObserve,
                Capability::ReplyRead,
                Capability::MotionPlay,
            ],
        );
        for invocation in &runtime.inner().pending_hooks {
            assert!(
                !invocation
                    .captured
                    .grants()
                    .contains(&Capability::ReplyRead)
            );
            assert!(
                !invocation
                    .captured
                    .grants()
                    .contains(&Capability::MotionPlay)
            );
            assert!(
                matches!(invocation.captured.event(), LifecycleEvent::ReplyFinished { outcome: ReplyOutcome::Completed { text }, .. } if text.is_empty())
            );
        }
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &events, None, now);
        assert_eq!(
            runtime.inner().pending_hooks.len(),
            MAX_HOOK_QUEUE,
            "replayed source events queued again"
        );
        assert_eq!(runtime.inner().hooks[&id].dropped_invocations, 8);
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[], None, now + HOOK_QUEUE_AGE);
        assert!(runtime.inner().pending_hooks.is_empty());
        assert_eq!(runtime.inner().hooks[&id].dropped_invocations, 40);
        assert!(runtime.views()[0].status.contains("expired"));
        assert!(
            runtime.views()[0]
                .status
                .contains("Dropped hook invocations: 40")
        );

        // Saturation is exercised through expiry of real queued invocations.
        let fresh = [
            SequencedEvent {
                sequence: 41,
                event: events[0].event.clone(),
            },
            SequencedEvent {
                sequence: 42,
                event: events[1].event.clone(),
            },
        ];
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &fresh, None, now + HOOK_QUEUE_AGE);
        assert_eq!(runtime.inner().pending_hooks.len(), 2);
        runtime
            .inner()
            .hooks
            .get_mut(&id)
            .unwrap()
            .dropped_invocations = u64::MAX - 1;
        ExtensionRuntime::capture_hooks(&mut runtime.inner(), &[], None, now + HOOK_QUEUE_AGE * 2);
        assert!(runtime.inner().pending_hooks.is_empty());
        assert_eq!(runtime.inner().hooks[&id].dropped_invocations, u64::MAX);
    }
    #[test]
    fn installed_reply_gesture_example_waits_for_actual_motion_terminal_receipt() {
        gesture_roundtrip(false, false);
    }
    #[test]
    fn installed_one_shot_hook_waits_for_motion_then_releases_process_slot() {
        gesture_roundtrip(true, false);
    }
    #[test]
    fn installed_twitch_replay_persists_attribution_then_waits_for_real_motion() {
        gesture_roundtrip(false, true);
    }
    fn gesture_roundtrip(one_shot_hook: bool, twitch_replay: bool) {
        // These three real-import fixtures each model one application with one
        // process-wide loader. Give each a fresh harness, as the OS-resource
        // stress test below already does. Unrelated unit-test imports must not
        // turn their required successful completion into a legitimate Busy.
        const ISOLATED: &str = "WAIFU_GESTURE_ROUNDTRIP_ISOLATED";
        let case = if twitch_replay {
            "installed_twitch_replay_persists_attribution_then_waits_for_real_motion"
        } else if one_shot_hook {
            "installed_one_shot_hook_waits_for_motion_then_releases_process_slot"
        } else {
            "installed_reply_gesture_example_waits_for_actual_motion_terminal_receipt"
        };
        if std::env::var(ISOLATED).as_deref() != Ok(case) {
            let module = module_path!().split_once("::").unwrap().1;
            let name = format!("{module}::{case}");
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--test-threads=1", "--nocapture"])
                .env(ISOLATED, case)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            // The inner playback and terminal-receipt deadlines below are
            // unchanged; this outer bound also covers launching the harness.
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut timed_out = false;
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !timed_out
                    && output.status.success()
                    && stdout.contains(&name)
                    && stdout.contains("test result: ok. 1 passed;"),
                "isolated gesture roundtrip failed or ran no matching test: {stdout} {stderr}"
            );
            return;
        }
        use crate::{
            animation::BvhAnimationPlugin,
            extension_installed::InstalledRuntime,
            extension_lifecycle::{LifecycleEvent, ReplyOutcome, SequencedEvent},
            extension_store::ExtensionStore,
            extensions::PackView,
            loading::AvatarLoadState,
        };
        use bevy::{prelude::*, time::TimeUpdateStrategy};
        use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;
        let root = std::env::temp_dir().join(format!("waifu-reply-gesture-{}", Uuid::new_v4()));
        let mut store = ExtensionStore::open(root.clone()).unwrap();
        let folder = if twitch_replay {
            "twitch-replay"
        } else if one_shot_hook {
            "hook-gesture"
        } else {
            "reply-gesture"
        };
        let id = store
            .install_integration(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/extensions")
                    .join(folder),
            )
            .unwrap();
        let mut grants = vec![Capability::LifecycleObserve, Capability::MotionPlay];
        if twitch_replay {
            grants.push(Capability::MessageDeliver);
        }
        store.set_grants(id, grants).unwrap();
        store.set_enabled(id, true).unwrap();
        let installed = store.packs()[0].clone();
        let data = store.data_directory(&installed).unwrap();
        let pack = store.resolved(&installed).unwrap();
        let catalog = ActionCatalog::from_packs([&pack], &[]);
        let view = PackView {
            credentials: Default::default(),
            installed,
            content: Some(pack),
            data_directory: Some(data.clone()),
            error: None,
        };
        let runtime = ExtensionRuntime::default();
        struct Cleanup(ExtensionRuntime, std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown(Duration::from_secs(2));
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        struct SavedMessages(crate::data::Store, usize);
        impl crate::extension_actions::ExternalMessageSink for SavedMessages {
            fn deliver(
                &mut self,
                target: Target,
                origin: &IntegrationOrigin,
                text: &str,
            ) -> Result<Uuid, (crate::extension_protocol::ErrorCode, String)> {
                let id = Uuid::new_v4();
                let mut history = self.0.history(target.uuid).unwrap();
                history.push(
                    crate::data::Turn::external(
                        text,
                        crate::data::ExternalMessage {
                            message_id: id,
                            origin: origin.clone(),
                        },
                    )
                    .unwrap(),
                );
                self.0
                    .save_external_history(target.uuid, &history, id)
                    .map_err(|error| (crate::extension_protocol::ErrorCode::Unavailable, error))?;
                self.1 += 1;
                Ok(id)
            }
        }
        let mut saved = SavedMessages(
            crate::data::Store::open(root.join("conversation-store"))
                .unwrap()
                .0,
            0,
        );
        let _cleanup = Cleanup(runtime.clone(), root);
        let mut binder = InstalledRuntime::default();
        binder.reconcile(&[view], &runtime);
        assert!(binder.errors.is_empty(), "{:?}", binder.errors);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1. / 60.,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let avatar = app
            .world_mut()
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(avatar);
        app.world_mut().spawn((
            HumanBoneName::Hips,
            Transform::from_xyz(0., 1., 0.),
            GlobalTransform::default(),
            ChildOf(avatar),
        ));
        app.world_mut().spawn((
            HumanBoneName::Head,
            Transform::from_xyz(0., 1.6, 0.),
            GlobalTransform::default(),
            ChildOf(avatar),
        ));
        let character = Uuid::new_v4();
        let target = Some(Target {
            uuid: character,
            generation: 1,
        });
        let mut behavior = crate::behavior::BehaviorController::default();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut sent_reply = false;
        let receipt = data.join(if twitch_replay {
            "latest-replay.json"
        } else if one_shot_hook {
            "latest-hook.json"
        } else {
            "latest.json"
        });
        loop {
            let mut events = Vec::new();
            if !twitch_replay && !sent_reply && (one_shot_hook || receipt.exists()) {
                events.push(SequencedEvent {
                    sequence: 1,
                    event: LifecycleEvent::ReplyFinished {
                        reply_id: Uuid::new_v4(),
                        character: Some(character),
                        outcome: ReplyOutcome::Completed {
                            text: "private text is not granted".into(),
                        },
                    },
                });
                sent_reply = true;
            }
            runtime.frame(
                &events,
                target,
                false,
                &catalog,
                ActionControllers {
                    external_messages: if twitch_replay {
                        Some(&mut saved)
                    } else {
                        None
                    },
                    animation: &mut app.world_mut().resource_mut::<AnimationState>(),
                    behavior: &mut behavior,
                },
            );
            app.update();
            if let Some(value) = std::fs::read(&receipt)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                && (value["type"] == "motion.finished" || value["state"] == "finished")
            {
                if twitch_replay {
                    let history = saved.0.history(character).unwrap();
                    assert_eq!(saved.1, 1);
                    assert_eq!(history.len(), 1);
                    assert_eq!(history[0].role, "external");
                    assert!(history[0].content.contains("Synthetic Twitch subscription"));
                    let attribution = history[0].external.as_ref().unwrap();
                    assert_eq!(
                        attribution.origin.integration_id,
                        "studio.example.twitch-replay"
                    );
                    assert_eq!(attribution.origin.display_name, "Twitch replay");
                    assert!(!attribution.message_id.is_nil());
                }
                let value = if twitch_replay {
                    &value["motion"]
                } else {
                    &value
                };
                assert_eq!(value["outcome"], "completed");
                assert_eq!(
                    value["action_id"],
                    if twitch_replay {
                        "replay-nod"
                    } else if one_shot_hook {
                        "hook-nod"
                    } else {
                        "gesture-1"
                    }
                );
                assert_eq!(value["motion_id"], format!("studio.example.{folder}/nod"));
                assert_eq!(value["target"]["uuid"], character.to_string());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no completed motion receipt: {:?}, {}",
                runtime.views(),
                std::fs::read_to_string(&receipt).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        if one_shot_hook {
            while runtime.process_count() != 0 {
                runtime.frame(
                    &[],
                    target,
                    false,
                    &catalog,
                    ActionControllers {
                        external_messages: if twitch_replay {
                            Some(&mut saved)
                        } else {
                            None
                        },
                        animation: &mut app.world_mut().resource_mut::<AnimationState>(),
                        behavior: &mut behavior,
                    },
                );
                app.update();
                assert!(
                    Instant::now() < deadline,
                    "hook did not terminate: {:?}",
                    runtime.views()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(runtime.views().len(), 1);
            assert!(runtime.views()[0].enabled);
            assert!(!runtime.views()[0].active);
        }
    }
    use crate::{
        animation::AnimationState,
        behavior::{BehaviorConfig, BehaviorController},
        extension_lifecycle::{LifecycleEvent, ReplyOutcome},
    };

    #[test]
    fn repeated_restart_disable_and_remove_reclaim_children_queues_and_os_resources() {
        // The default cargo test runner is parallel. Measure process-wide OS
        // resources only in a fresh harness running this exact test alone.
        const ISOLATED: &str = "WAIFU_LIFECYCLE_STRESS_ISOLATED";
        if std::env::var(ISOLATED).as_deref() != Ok("1") {
            let name = concat!(
                module_path!(),
                "::repeated_restart_disable_and_remove_reclaim_children_queues_and_os_resources"
            );
            let name = name.split_once("::").unwrap().1;
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--test-threads=1", "--nocapture"])
                .env(ISOLATED, "1")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(75);
            let mut timed_out = false;
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !timed_out && output.status.success(),
                "isolated stress failed: {stdout} {stderr}"
            );
            let metric = stderr
                .lines()
                .find(|line| {
                    line.starts_with("EXTENSION_LIFECYCLE_STRESS cycles=20 incarnations=40")
                })
                .expect("isolated harness did not execute the stress scenario");
            eprintln!("{metric}");
            return;
        }
        fn resources() -> (usize, usize) {
            (
                std::fs::read_dir("/proc/self/fd").unwrap().count(),
                std::fs::read_dir("/proc/self/task").unwrap().count(),
            )
        }
        fn process_stamp(pid: u32) -> Option<String> {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            stat.rsplit_once(')')?
                .1
                .split_whitespace()
                .nth(19)
                .map(str::to_owned)
        }
        fn ready(
            runtime: &ExtensionRuntime,
            path: &std::path::Path,
            id: Uuid,
            generation: u64,
            deadline: Instant,
            mut pump: impl FnMut() -> bool,
        ) -> (u32, String) {
            loop {
                let applied = pump();
                let session = runtime.inner().entries[&id]
                    .connection
                    .session_id()
                    .to_string();
                if let Some(value) = std::fs::read(path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    && value["generation"] == generation
                    && value["session"] == session
                    && value["accepted"] == true
                {
                    assert!(
                        applied,
                        "accepted fixture expression must reach the real controller"
                    );
                    assert_eq!(
                        value["private_bytes"], 0,
                        "restart replayed an earlier session's private backlog"
                    );
                    let pid = value["pid"].as_u64().unwrap() as u32;
                    return (pid, process_stamp(pid).expect("accepted child must exist"));
                }
                assert!(
                    Instant::now() < deadline,
                    "stress child did not acknowledge: {:?}",
                    runtime.views()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        struct Cleanup(ExtensionRuntime, std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown(Duration::from_secs(2));
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let root = std::env::temp_dir().join(format!("waifu-lifecycle-stress-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let runtime = ExtensionRuntime::default();
        let _cleanup = Cleanup(runtime.clone(), root.clone());
        let script = r#"
import json, os, sys
from pathlib import Path
path = Path(os.environ['WAIFU_EXTENSION_DATA']) / 'stress.json'
h = json.loads(sys.stdin.readline())
state = {'session':h['session_id'], 'generation':h['process_generation'], 'pid':os.getpid(), 'accepted':False, 'private_bytes':0}
def record():
    temp = path.with_suffix('.tmp')
    temp.write_text(json.dumps(state))
    temp.replace(path)
record()
sent = False
for line in sys.stdin:
    m = json.loads(line)['message']
    if m['type'] == 'event':
        e = m['event']
        if e['type'] == 'reply.text':
            state['private_bytes'] += len(e['text'].encode())
            record()
        if e['type'] == 'target.changed' and e['target'] is not None and not sent:
            print(json.dumps({'api_version':1,'type':'action','id':'pose','sequence':1,'session_id':h['session_id'],'process_generation':h['process_generation'],'target':e['target'],'action':{'type':'expression.apply','name':'happy','strength':0.5,'duration_ms':10000}}), flush=True)
            sent = True
    elif m['type'] == 'result':
        assert m['result']['status'] == 'ok'
        state['accepted'] = True
        record()
"#;
        let character = Uuid::new_v4();
        let names = vec!["happy".into()];
        let catalog = ActionCatalog::from_packs([], &names);
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        behavior
            .select(Some(character), &BehaviorConfig::default(), &names)
            .unwrap();
        let mut frame = |events: &[SequencedEvent], target| {
            runtime.frame(
                events,
                target,
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
            !behavior
                .weights_for(Some(character), &names, &BTreeMap::new(), "")
                .is_empty()
        };
        let grants = [
            Capability::LifecycleObserve,
            Capability::ReplyRead,
            Capability::ExpressionApply,
            Capability::MessageDeliver,
        ];
        let receipt = root.join("data/stress.json");
        let baseline = resources();
        let mut peak = baseline;
        let deadline = Instant::now() + Duration::from_secs(60);
        // Forty actual incarnations exceed the current owner-table capacity;
        // stale ownership must not accumulate until admission stops working.
        for cycle in 0..20 {
            let id = Uuid::new_v4();
            let mut target = Target {
                uuid: character,
                generation: cycle * 2 + 1,
            };
            runtime
                .start(
                    id,
                    ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec!["-u".into(), "-c".into(), script.into()],
                        working_directory: root.clone(),
                        data_directory: root.join("data"),
                        lifetime: Some(Duration::from_secs(10)),
                    },
                    IntegrationOrigin {
                        integration_id: "org.example.lifecycle-stress".into(),
                        display_name: "Lifecycle stress fixture".into(),
                    },
                    grants,
                )
                .unwrap();
            let first = ready(&runtime, &receipt, id, 1, deadline, || {
                frame(&[], Some(target))
            });
            let active = resources();
            peak.0 = peak.0.max(active.0);
            peak.1 = peak.1.max(active.1);
            if cycle % 2 == 0 {
                target.generation += 1;
            } else {
                runtime.set_grants(id, [Capability::LifecycleObserve, Capability::ReplyRead]);
                runtime.set_grants(id, grants);
                assert_eq!(
                    runtime.chat_source_generation(id),
                    None,
                    "transient revocation must invalidate intake"
                );
            }
            assert!(
                !frame(&[], Some(target)),
                "takeover must release old owned expressions"
            );
            let backlog = [SequencedEvent {
                sequence: cycle + 1,
                event: LifecycleEvent::ReplyFinished {
                    reply_id: Uuid::new_v4(),
                    character: Some(character),
                    outcome: ReplyOutcome::Completed {
                        text: "synthetic backlog ".repeat(10_000),
                    },
                },
            }];
            frame(&backlog, Some(target));
            assert!(runtime.inner().entries[&id].relay.queued_bytes() > 0);
            runtime.stop(id);
            assert_eq!(runtime.inner().entries[&id].relay.queued_bytes(), 0);
            assert_eq!(runtime.chat_source_generation(id), None);
            runtime.restart(id);
            let second = ready(&runtime, &receipt, id, 2, deadline, || {
                frame(&[], Some(target))
            });
            assert_ne!(
                first, second,
                "restart must have a fresh PID/start-time identity"
            );
            runtime.remove(id);
            let settle = (Instant::now() + Duration::from_secs(2)).min(deadline);
            loop {
                let applied = frame(&[], Some(target));
                let counts = resources();
                let children_gone = [first.clone(), second.clone()]
                    .iter()
                    .all(|(pid, stamp)| process_stamp(*pid).as_ref() != Some(stamp));
                if runtime.process_count() == 0
                    && children_gone
                    && counts.0 <= baseline.0 + 2
                    && counts.1 <= baseline.1 + 2
                {
                    assert!(!applied);
                    break;
                }
                assert!(
                    Instant::now() < settle,
                    "cycle {cycle} did not reclaim resources: baseline={baseline:?}, now={counts:?}, children_gone={children_gone}"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(runtime.views().is_empty());
            assert!(runtime.inner().pending_hooks.is_empty());
        }
        let final_counts = resources();
        assert!(runtime.shutdown(Duration::from_secs(2)));
        eprintln!(
            "EXTENSION_LIFECYCLE_STRESS cycles=20 incarnations=40 baseline_fds={} peak_fds={} final_fds={} baseline_threads={} peak_threads={} final_threads={}",
            baseline.0, peak.0, final_counts.0, baseline.1, peak.1, final_counts.1
        );
    }

    #[test]
    fn app_runtime_delivers_without_avatar_then_controls_restarts_and_releases() {
        let root = std::env::temp_dir().join(format!("waifu-runtime-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let runtime = ExtensionRuntime::default();
        let mut app = bevy::prelude::App::new();
        app.insert_resource(runtime.clone());
        let id = Uuid::new_v4();
        let character = Uuid::new_v4();
        let reply = Uuid::new_v4();
        let script = r#"
import json, os, sys
from pathlib import Path
path = Path(os.environ['WAIFU_EXTENSION_DATA']) / 'receipt'
def record(text):
    with path.open('a') as out:
        out.write(text + '\n')
h = json.loads(sys.stdin.readline())
record('start:' + str(h['process_generation']))
sent = False
for line in sys.stdin:
    m = json.loads(line)['message']
    if m['type'] == 'event':
        e = m['event']
        if e['type'] == 'reply.finished': record('reply:' + e['reply_id'])
        if e['type'] == 'target.changed' and e['target'] is not None and not sent:
            a = {'api_version':1, 'type':'action', 'id':'smile', 'sequence':1,
                 'session_id':h['session_id'], 'process_generation':h['process_generation'],
                 'target':e['target'], 'action':{'type':'expression.apply','name':'happy','strength':0.5,'duration_ms':10000}}
            print(json.dumps(a), flush=True)
            sent = True
    if m['type'] == 'result':
        assert m['result']['status'] == 'ok'
        record('accepted:' + str(h['process_generation']))
"#;
        runtime
            .start(
                id,
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-u".into(), "-c".into(), script.into()],
                    working_directory: root.clone(),
                    data_directory: root.join("data"),
                    lifetime: Some(Duration::from_secs(10)),
                },
                IntegrationOrigin {
                    integration_id: "org.example.runtime".into(),
                    display_name: "Runtime fixture".into(),
                },
                [
                    Capability::LifecycleObserve,
                    Capability::ExpressionApply,
                    Capability::MessageDeliver,
                ],
            )
            .unwrap();
        let names = vec!["happy".into()];
        let catalog = ActionCatalog::from_packs([], &names);
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        behavior
            .select(Some(character), &BehaviorConfig::default(), &names)
            .unwrap();
        let events = [SequencedEvent {
            sequence: 1,
            event: LifecycleEvent::ReplyFinished {
                reply_id: reply,
                character: Some(character),
                outcome: ReplyOutcome::Completed {
                    text: "private text is not granted".into(),
                },
            },
        }];
        runtime.frame(
            &events,
            None,
            false,
            &catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        let path = root.join("data/receipt");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut drive = |target, expected: &str| {
            loop {
                runtime.frame(
                    &[],
                    target,
                    false,
                    &catalog,
                    ActionControllers {
                        external_messages: None,
                        animation: &mut animation,
                        behavior: &mut behavior,
                    },
                );
                if std::fs::read_to_string(&path)
                    .unwrap_or_default()
                    .contains(expected)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "runtime receipt missing: {expected}: {:?}",
                    runtime.views()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        drive(None, &format!("reply:{reply}"));
        let target = Target {
            uuid: character,
            generation: 1,
        };
        drive(Some(target), "accepted:1");
        assert_eq!(runtime.chat_source_generation(id), Some(1));
        runtime.restart(id);
        assert_eq!(
            runtime.chat_source_generation(id),
            None,
            "restart immediately invalidates delayed intake"
        );
        drive(Some(target), "accepted:2");
        assert_eq!(runtime.chat_source_generation(id), Some(2));
        assert!(
            !behavior
                .weights_for(Some(character), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        // A transient revoke must be observed even if restored before Update.
        runtime.set_grants(id, [Capability::LifecycleObserve]);
        runtime.set_grants(
            id,
            [
                Capability::LifecycleObserve,
                Capability::ExpressionApply,
                Capability::ReplyRead,
                Capability::MessageDeliver,
            ],
        );
        assert_eq!(
            runtime.chat_source_generation(id),
            None,
            "transient delivery revocation invalidates intake even if restored before Update"
        );
        runtime.frame(
            &[],
            Some(target),
            false,
            &catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        assert!(
            behavior
                .weights_for(Some(character), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        let large_reply = [SequencedEvent {
            sequence: 2,
            event: LifecycleEvent::ReplyFinished {
                reply_id: Uuid::new_v4(),
                character: Some(character),
                outcome: ReplyOutcome::Completed {
                    text: "private backlog".repeat(10_000),
                },
            },
        }];
        runtime.frame(
            &large_reply,
            Some(target),
            false,
            &catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        assert!(runtime.inner().entries[&id].relay.queued_bytes() > 0);
        runtime.stop(id);
        assert_eq!(runtime.inner().entries[&id].relay.queued_bytes(), 0);
        runtime.frame(
            &[],
            Some(target),
            false,
            &catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        assert!(
            behavior
                .weights_for(Some(character), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        assert!(!runtime.views()[0].enabled);
        drop(app);
        assert!(runtime.shutdown(Duration::from_secs(2)));
        let receipts = std::fs::read_to_string(path).unwrap();
        assert_eq!(receipts.matches("accepted:1").count(), 1);
        assert_eq!(receipts.matches("accepted:2").count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
