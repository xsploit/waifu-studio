//! Bounded background pack worker and cached host catalog. Executable packages
//! have a separate explicit install path; this worker never starts a process.
use crate::{
    extension_credentials::CredentialStatus,
    extension_manifest::ResolvedPack,
    extension_protocol::Capability,
    extension_settings::{Settings, validate_wire_values},
    extension_store::{ExtensionStore, InstalledPack},
};
use bevy::prelude::Resource;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PackView {
    pub installed: InstalledPack,
    pub content: Option<ResolvedPack>,
    /// Prepared by the store worker, stable across revisions of one install.
    /// Only executable integrations have a writable data directory.
    pub data_directory: Option<PathBuf>,
    /// Private storage revision and configured IDs only; never credential values.
    pub credentials: CredentialStatus,
    pub error: Option<String>,
}

enum Command {
    Install(PathBuf),
    PickInstall,
    InstallIntegration(PathBuf),
    PickInstallIntegration,
    Enabled(Uuid, bool),
    Grants(Uuid, Vec<Capability>),
    Settings(Uuid, Settings),
    Credentials(Uuid, BTreeMap<String, Option<String>>),
    Remove(Uuid),
    Authorized {
        attempt: Uuid,
        expected: Box<InstalledPack>,
        credential_revision: Option<Uuid>,
        gate: Arc<AtomicU8>,
        bundle: crate::twitch_auth::TokenBundle,
    },
    PrepareAuth {
        attempt: Uuid,
        expected: Box<InstalledPack>,
        credential_revision: Option<Uuid>,
        gate: Arc<AtomicU8>,
        client_id: Option<String>,
    },
    PrepareRefresh {
        attempt: Uuid,
        expected: Box<InstalledPack>,
        credential_revision: Option<Uuid>,
        gate: Arc<AtomicU8>,
    },
    Refresh,
}

#[derive(Clone, Copy)]
enum InstallKind {
    Content,
    Integration,
}

struct Outcome {
    initial_load: bool,
    auth_attempt: Option<Uuid>,
    auth_job: Option<crate::twitch_auth::TwitchAuth>,
    oauth_schedule: BTreeMap<Uuid, u64>,
    packs: Vec<PackView>,
    result: Result<String, String>,
}

/// Transient authorization UI data; never contains access/refresh/device tokens.
#[derive(Clone)]
pub struct TwitchAuthView {
    pub attempt_id: Uuid,
    pub status: String,
    pub challenge: Option<crate::twitch_auth::DeviceChallenge>,
    pub active: bool,
}

struct AuthAttempt {
    preparing: bool,
    id: Uuid,
    expected: InstalledPack,
    credential_revision: Option<Uuid>,
    job: Option<crate::twitch_auth::TwitchAuth>,
    bundle: Option<crate::twitch_auth::TokenBundle>,
    // 0 pending, 1 cancelled, 2 committing, 3 saved, 4 failed.
    gate: Arc<AtomicU8>,
    terminal: Option<String>,
}

#[derive(Resource)]
pub struct ExtensionState {
    pub packs: Vec<PackView>,
    pub busy: bool,
    pub status: String,
    pub error: Option<String>,
    pub generation: u64,
    initial_load_completed: bool,
    twitch_auth: Option<AuthAttempt>,
    oauth_schedule: BTreeMap<Uuid, u64>,
    refresh_seen: BTreeMap<Uuid, Uuid>,
    refresh_not_before: Option<std::time::Instant>,
    cancelled: Arc<AtomicBool>,
    sender: SyncSender<Command>,
    receiver: Mutex<Receiver<Outcome>>,
}

impl ExtensionState {
    pub fn new(root: PathBuf) -> Self {
        let (sender, commands) = mpsc::sync_channel(1);
        let (events, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let spawned = std::thread::Builder::new()
            .name("waifu-extensions".into())
            .spawn(move || {
                if worker_cancelled.load(Ordering::Acquire) {
                    return;
                }
                let mut store = ExtensionStore::open(root.clone());
                let mut initial = outcome(&store, Ok("Extensions loaded".into()));
                initial.initial_load = true;
                if events.send(initial).is_err() {
                    return;
                }
                while let Ok(command) = commands.recv() {
                    if worker_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let auth_attempt = match &command {
                        Command::Authorized { attempt, .. }
                        | Command::PrepareAuth { attempt, .. }
                        | Command::PrepareRefresh { attempt, .. } => Some(*attempt),
                        _ => None,
                    };
                    let mut auth_job = None;
                    let result = if matches!(command, Command::Refresh) {
                        drop(store);
                        store = ExtensionStore::open(root.clone());
                        Ok("Extensions refreshed".into())
                    } else {
                        match store.as_mut() {
                            Err(error) => Err(error.clone()),
                            Ok(store) => match command {
                                Command::Install(path) => {
                                    install_folder(store, &path, InstallKind::Content)
                                }
                                Command::InstallIntegration(path) => {
                                    install_folder(store, &path, InstallKind::Integration)
                                }
                                Command::PickInstall => install_picked(
                                    store,
                                    &worker_cancelled,
                                    InstallKind::Content,
                                    || {
                                        rfd::FileDialog::new()
                                            .set_title("Choose a content-pack folder")
                                            .pick_folder()
                                    },
                                ),
                                Command::PickInstallIntegration => install_picked(
                                    store,
                                    &worker_cancelled,
                                    InstallKind::Integration,
                                    || {
                                        rfd::FileDialog::new()
                                            .set_title("Choose an executable integration folder")
                                            .pick_folder()
                                    },
                                ),
                                Command::Enabled(id, enabled) => {
                                    store.set_enabled(id, enabled).map(|_| {
                                        if enabled {
                                            "Pack enabled"
                                        } else {
                                            "Pack disabled"
                                        }
                                        .into()
                                    })
                                }
                                Command::Remove(id) => {
                                    store.remove(id).map(|_| "Pack removed".into())
                                }
                                Command::Grants(id, grants) => store
                                    .set_grants(id, grants)
                                    .map(|_| "Integration permissions updated".into()),
                                Command::Settings(id, settings) => store
                                    .set_settings(id, settings)
                                    .map(|_| "Extension settings saved".into()),
                                Command::Credentials(id, updates) => store
                                    .set_credentials(id, updates)
                                    .map(|_| "Integration credentials updated".into()),
                                Command::Authorized {
                                    expected,
                                    credential_revision,
                                    gate,
                                    bundle,
                                    ..
                                } => {
                                    if gate
                                        .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                                        .is_err()
                                    {
                                        Err("Twitch authorization cancelled".into())
                                    } else {
                                        let result = oauth_metadata(&bundle).and_then(|metadata| {
                                            store.set_oauth_credentials_if_current(
                                                &expected,
                                                credential_revision,
                                                bundle.into_credential_updates(),
                                                &metadata,
                                            )
                                        });
                                        gate.store(
                                            if result.is_ok() { 3 } else { 4 },
                                            Ordering::Release,
                                        );
                                        result.map(|_| "Twitch authorization saved".into())
                                    }
                                }
                                Command::PrepareAuth {
                                    expected,
                                    credential_revision,
                                    gate,
                                    client_id,
                                    ..
                                } => prepare_device_auth(
                                    store,
                                    &expected,
                                    credential_revision,
                                    &gate,
                                    client_id,
                                )
                                .map(|job| {
                                    auth_job = Some(job);
                                    "Waiting for Twitch authorization".into()
                                }),
                                Command::PrepareRefresh {
                                    expected,
                                    credential_revision,
                                    gate,
                                    ..
                                } => prepare_refresh(store, &expected, credential_revision, &gate)
                                    .map(|job| {
                                        auth_job = Some(job);
                                        "Refreshing Twitch authorization".into()
                                    }),
                                Command::Refresh => unreachable!(),
                            },
                        }
                    };
                    if worker_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let mut response = outcome(&store, result);
                    response.auth_attempt = auth_attempt;
                    response.auth_job = auth_job;
                    if events.send(response).is_err() {
                        break;
                    }
                }
            });
        let error = spawned
            .err()
            .map(|e| format!("Cannot start extension worker: {e}"));
        let initial_load_completed = error.is_some();
        Self {
            packs: vec![],
            busy: error.is_none(),
            status: "Loading extensions…".into(),
            error,
            generation: 0,
            initial_load_completed,
            twitch_auth: None,
            oauth_schedule: BTreeMap::new(),
            refresh_seen: BTreeMap::new(),
            refresh_not_before: None,
            cancelled,
            sender,
            receiver: Mutex::new(receiver),
        }
    }

    /// Sticky local initialization outcome, independent of later OAuth/install
    /// work and its generic busy flag. Errors also finish the initial attempt.
    pub fn initial_load_completed(&self) -> bool {
        self.initial_load_completed
    }

    pub fn poll(&mut self) {
        let next = self.receiver.lock().map(|r| r.try_recv()).map_err(|_| ());
        if let Ok(Ok(mut next)) = next {
            self.initial_load_completed |= next.initial_load;
            let prepared = next.auth_job.is_some();
            if let Some(id) = next.auth_attempt
                && let Some(auth) = self.twitch_auth.as_mut().filter(|auth| auth.id == id)
                && let Some(job) = next.auth_job.take()
            {
                if auth.gate.load(Ordering::Acquire) != 0 {
                    job.cancel();
                }
                auth.preparing = false;
                auth.job = Some(job);
            }
            if let Some(id) = next.auth_attempt
                && let Some(auth) = self.twitch_auth.as_mut().filter(|auth| auth.id == id)
                && auth.gate.load(Ordering::Acquire) != 1
                && !prepared
            {
                let preparing = auth.preparing;
                auth.preparing = false;
                auth.terminal = Some(if next.result.is_ok() {
                    "Authorization saved".into()
                } else if preparing {
                    "Authorization could not start; check setup".into()
                } else {
                    "Authorization save needs attention; check credential status".into()
                });
            }
            self.oauth_schedule = next.oauth_schedule;
            self.packs = next.packs;
            self.busy = false;
            self.generation = self.generation.wrapping_add(1);
            match next.result {
                Ok(message) => {
                    self.status = message;
                    self.error = None;
                }
                Err(error) => {
                    self.status = "Extension needs attention".into();
                    self.error = Some(error);
                }
            }
        } else if matches!(next, Err(_) | Ok(Err(mpsc::TryRecvError::Disconnected))) {
            self.initial_load_completed = true;
            self.busy = false;
            if let Some(auth) = &mut self.twitch_auth {
                let preparing = auth.preparing;
                // Cancel only work that has not crossed the store's commit
                // boundary. A missing outcome cannot establish commit rollback.
                let _ = auth
                    .gate
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
                if let Some(job) = &auth.job {
                    job.cancel();
                }
                auth.preparing = false;
                auth.bundle = None;
                if auth.terminal.is_none() {
                    auth.terminal = Some(if matches!(auth.gate.load(Ordering::Acquire), 2 | 3) {
                        "Authorization save outcome unavailable; restart the app to check credentials".into()
                    } else if preparing {
                        "Authorization preparation stopped; restart the app to retry".into()
                    } else {
                        "Authorization worker stopped; restart the app to retry".into()
                    });
                }
            }
            if self.error.is_none() {
                self.error = Some("Extension worker stopped. Restart the app to retry.".into());
                self.status = "Extensions unavailable".into();
            }
        }
        self.poll_twitch_auth();
        self.schedule_twitch_refresh();
    }

    fn submit(&mut self, command: Command) {
        if self.busy {
            self.error =
                Some("Extensions are busy; wait for the current operation and retry.".into());
            return;
        }
        if let Some(id) = self.twitch_auth.as_ref().map(|auth| auth.id) {
            self.cancel_twitch_auth(id);
        }
        match self.sender.try_send(command) {
            Ok(()) => {
                self.busy = true;
                self.status = "Updating extensions…".into();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Extension worker is unavailable: {error}")),
        }
    }
    pub fn install(&mut self, folder: PathBuf) {
        self.submit(Command::Install(folder));
    }
    pub fn pick_install(&mut self) {
        self.submit(Command::PickInstall);
    }
    /// Explicit executable installation; the store initializes new installs
    /// disabled with no grants. Enabling and granting are separate commands.
    pub fn install_integration(&mut self, folder: PathBuf) {
        self.submit(Command::InstallIntegration(folder));
    }
    pub fn pick_install_integration(&mut self) {
        self.submit(Command::PickInstallIntegration);
    }
    pub fn set_enabled(&mut self, id: Uuid, enabled: bool) {
        self.submit(Command::Enabled(id, enabled));
    }
    pub fn set_grants(&mut self, id: Uuid, grants: Vec<Capability>) {
        self.submit(Command::Grants(id, grants));
    }
    /// Persist a complete settings draft through the bounded worker queue.
    /// The store validates it against the current installed schema. Runtime
    /// reconciliation restarts enabled integrations after successful changes.
    pub fn set_settings(&mut self, id: Uuid, settings: Settings) {
        if let Err(error) = validate_wire_values(&settings) {
            self.error = Some(error);
            return;
        }
        self.submit(Command::Settings(id, settings));
    }
    /// Submit a bounded private credential draft for worker-only persistence.
    /// None clears a declared credential; missing keys are unchanged. Only
    /// status/revision returns from the worker, never any saved secret value.
    pub fn set_credentials(&mut self, id: Uuid, updates: BTreeMap<String, Option<String>>) {
        let Some(pack) = self
            .packs
            .iter()
            .find(|pack| pack.installed.installation_id == id)
        else {
            self.error = Some("Extension is no longer installed".into());
            return;
        };
        if !pack.installed.executable {
            self.error = Some("Content-only packs cannot receive integration credentials".into());
            return;
        }
        let Some(content) = &pack.content else {
            self.error = Some("Integration credentials are unavailable".into());
            return;
        };
        if let Err(error) = validate_credential_updates(&content.manifest.credentials, &updates) {
            self.error = Some(error);
            return;
        }
        self.submit(Command::Credentials(id, updates));
    }
    pub fn remove(&mut self, id: Uuid) {
        self.submit(Command::Remove(id));
    }
    pub fn refresh(&mut self) {
        self.submit(Command::Refresh);
    }

    /// Explicit consent only: does not enable the package or add host grants.
    pub fn start_twitch_auth(&mut self, id: Uuid, client_id: String) -> Result<Uuid, String> {
        if self.busy
            || self.twitch_auth.as_ref().is_some_and(|auth| {
                auth.job.as_ref().is_some_and(|job| !job.is_finished())
                    || (auth.terminal.is_none() && auth.gate.load(Ordering::Acquire) != 1)
            })
        {
            return Err("Twitch authorization is busy; wait or cancel the current attempt".into());
        }
        let pack = self
            .packs
            .iter()
            .find(|pack| pack.installed.installation_id == id)
            .ok_or("Extension is no longer installed")?;
        let content = pack
            .content
            .as_ref()
            .ok_or("Twitch integration is unavailable")?;
        if pack.error.is_some()
            || !pack.installed.executable
            || pack.installed.package_id != "studio.integration.twitch"
            || !["client_id", "access_token", "refresh_token"]
                .iter()
                .all(|id| {
                    content
                        .manifest
                        .credentials
                        .iter()
                        .any(|definition| definition.id == *id)
                })
        {
            return Err("Update the Twitch package before connecting".into());
        }
        let client_id = if client_id.is_empty() {
            if !pack.credentials.configured.contains("client_id") {
                return Err("Enter a Twitch application ID before connecting".into());
            }
            None
        } else {
            if !crate::twitch_auth::valid_client_id(&client_id) {
                return Err("The application ID must contain 1–128 ASCII letters or digits".into());
            }
            Some(client_id)
        };
        let attempt = Uuid::new_v4();
        let gate = Arc::new(AtomicU8::new(0));
        self.sender
            .try_send(Command::PrepareAuth {
                attempt,
                expected: Box::new(pack.installed.clone()),
                credential_revision: pack.credentials.revision,
                gate: gate.clone(),
                client_id,
            })
            .map_err(|_| "Twitch authorization preparation is unavailable".to_owned())?;
        self.twitch_auth = Some(AuthAttempt {
            preparing: true,
            id: attempt,
            expected: pack.installed.clone(),
            credential_revision: pack.credentials.revision,
            job: None,
            bundle: None,
            gate,
            terminal: None,
        });
        self.busy = true;
        self.status = "Preparing Twitch sign-in…".into();
        Ok(attempt)
    }

    pub fn cancel_twitch_auth(&mut self, attempt: Uuid) {
        let Some(auth) = self.twitch_auth.as_mut().filter(|auth| auth.id == attempt) else {
            return;
        };
        // Once the store worker starts an atomic commit, cancellation cannot
        // retroactively undo it. UI reports Saving until its receipt arrives.
        if auth
            .gate
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            if let Some(job) = &auth.job {
                job.cancel();
            }
            auth.bundle = None;
            auth.terminal = Some("Authorization cancelled".into());
        }
    }

    pub fn twitch_auth_status(&self, id: Uuid) -> Option<TwitchAuthView> {
        let auth = self
            .twitch_auth
            .as_ref()
            .filter(|auth| auth.expected.installation_id == id)?;
        let running = auth.job.as_ref().map(|job| job.status());
        let status = auth.terminal.clone().unwrap_or_else(|| {
            match running.as_ref().map(|status| status.phase) {
                Some(crate::twitch_auth::AuthPhase::Starting) => "Starting authorization".into(),
                Some(crate::twitch_auth::AuthPhase::AwaitingAuthorization) => {
                    "Approve access in your browser".into()
                }
                Some(crate::twitch_auth::AuthPhase::Validating) => {
                    "Validating authorization".into()
                }
                Some(crate::twitch_auth::AuthPhase::Failed) => running
                    .as_ref()
                    .and_then(|status| status.error)
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "Authorization failed".into()),
                Some(crate::twitch_auth::AuthPhase::Cancelled) => "Authorization cancelled".into(),
                _ if auth.preparing => "Preparing authorization".into(),
                _ => "Saving authorization".into(),
            }
        });
        Some(TwitchAuthView {
            attempt_id: auth.id,
            status,
            challenge: if auth.terminal.is_none() {
                running.and_then(|status| status.challenge)
            } else {
                None
            },
            active: auth.terminal.is_none()
                || auth.job.as_ref().is_some_and(|job| !job.is_finished()),
        })
    }

    fn poll_twitch_auth(&mut self) {
        let Some(mut auth) = self.twitch_auth.take() else {
            return;
        };
        let current = self
            .packs
            .iter()
            .find(|pack| pack.installed.installation_id == auth.expected.installation_id);
        let stale = !current.is_some_and(|pack| {
            pack.installed == auth.expected
                && pack.credentials.revision == auth.credential_revision
                && pack.error.is_none()
        });
        if stale
            && auth
                .gate
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            if let Some(job) = &auth.job {
                job.cancel();
            }
            auth.bundle = None;
            auth.terminal = Some("Integration changed; connect again".into());
        }
        if auth.job.as_ref().is_some_and(|job| job.is_finished()) {
            if let Some(result) = auth.job.as_mut().and_then(|job| job.take_completion()) {
                match result {
                    Ok(bundle) if auth.gate.load(Ordering::Acquire) == 0 => {
                        auth.bundle = Some(bundle)
                    }
                    Ok(_) => {}
                    Err(error) if auth.terminal.is_none() => {
                        auth.terminal = Some(error.to_string())
                    }
                    Err(_) => {}
                }
            }
            auth.job = None;
        }
        if !self.busy
            && auth.gate.load(Ordering::Acquire) == 0
            && let Some(bundle) = auth.bundle.take()
        {
            let command = Command::Authorized {
                attempt: auth.id,
                expected: Box::new(auth.expected.clone()),
                credential_revision: auth.credential_revision,
                gate: auth.gate.clone(),
                bundle,
            };
            if self.sender.try_send(command).is_ok() {
                self.busy = true;
                self.status = "Saving Twitch authorization…".into();
            } else {
                auth.terminal = Some("Authorization could not be saved; connect again".into());
                auth.gate.store(4, Ordering::Release);
            }
        }
        self.twitch_auth = Some(auth);
    }

    fn schedule_twitch_refresh(&mut self) {
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
        if self.busy
            || self
                .refresh_not_before
                .is_some_and(|at| Instant::now() < at)
            || self.twitch_auth.as_ref().is_some_and(|auth| {
                auth.terminal.is_none() || auth.job.as_ref().is_some_and(|job| !job.is_finished())
            })
        {
            return;
        }
        self.refresh_seen.retain(|id, _| {
            self.packs
                .iter()
                .any(|pack| pack.installed.installation_id == *id)
        });
        let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return;
        };
        let candidate = self.packs.iter().find(|pack| {
            let id = pack.installed.installation_id;
            pack.installed.package_id == "studio.integration.twitch"
                && pack.installed.enabled
                && pack.error.is_none()
                && pack.credentials.configured.contains("refresh_token")
                && pack
                    .credentials
                    .revision
                    .is_some_and(|revision| self.refresh_seen.get(&id) != Some(&revision))
                && self
                    .oauth_schedule
                    .get(&id)
                    .is_some_and(|expires| *expires <= now.as_secs().saturating_add(300))
        });
        let Some(pack) = candidate else {
            return;
        };
        let expected = pack.installed.clone();
        let credential_revision = pack.credentials.revision;
        let id = expected.installation_id;
        let attempt = Uuid::new_v4();
        let gate = Arc::new(AtomicU8::new(0));
        let command = Command::PrepareRefresh {
            attempt,
            expected: Box::new(expected.clone()),
            credential_revision,
            gate: gate.clone(),
        };
        // A rotating refresh token is attempted once per credential revision per host session.
        // Unknown network/persistence outcomes require explicit sign-in, no retry.
        if let Some(revision) = credential_revision {
            self.refresh_seen.insert(id, revision);
        }
        self.refresh_not_before = Some(Instant::now() + Duration::from_secs(60));
        let sent = self.sender.try_send(command).is_ok();
        self.twitch_auth = Some(AuthAttempt {
            preparing: true,
            id: attempt,
            expected,
            credential_revision,
            job: None,
            bundle: None,
            gate,
            terminal: (!sent).then(|| "Refresh could not start; connect again".into()),
        });
        if sent {
            self.busy = true;
            self.status = "Preparing Twitch refresh…".into();
        }
    }

    pub fn active(&self) -> impl Iterator<Item = &ResolvedPack> {
        self.packs
            .iter()
            .filter(|p| p.installed.enabled && p.error.is_none())
            .filter_map(|p| p.content.as_ref())
    }
}

/// Bound drafts before moving secrets into the worker channel. The worker
/// credential store revalidates the merged saved values before committing.
fn validate_credential_updates(
    defs: &[crate::extension_credentials::CredentialDefinition],
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), String> {
    crate::extension_credentials::validate_definitions(defs)?;
    if updates.len() > defs.len() {
        return Err("Too many credential updates".into());
    }
    let mut bytes = 0usize;
    for (id, value) in updates {
        if !defs.iter().any(|definition| definition.id == *id) {
            return Err("Unknown credential ID".into());
        }
        if let Some(value) = value {
            if value.len() > 4096 || value.contains('\0') {
                return Err("Invalid credential value or value exceeds size limit".into());
            }
            bytes += value.len();
            if bytes > 16 * 1024 {
                return Err("Credentials exceed total size limit".into());
            }
        }
    }
    Ok(())
}

impl Drop for ExtensionState {
    fn drop(&mut self) {
        // Do not join a worker blocked in a native picker or filesystem copy.
        // Its store lock protects against replacement workers until it exits.
        if let Some(id) = self.twitch_auth.as_ref().map(|auth| auth.id) {
            self.cancel_twitch_auth(id);
        }
        self.cancelled.store(true, Ordering::Release);
    }
}

fn install_picked(
    store: &mut ExtensionStore,
    cancelled: &AtomicBool,
    kind: InstallKind,
    pick: impl FnOnce() -> Option<PathBuf>,
) -> Result<String, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Extension operation cancelled".into());
    }
    let path = pick();
    if cancelled.load(Ordering::Acquire) {
        return Err("Extension operation cancelled".into());
    }
    match path {
        Some(path) => install_folder(store, &path, kind),
        None => Ok("Installation cancelled".into()),
    }
}

fn install_folder(
    store: &mut ExtensionStore,
    path: &std::path::Path,
    kind: InstallKind,
) -> Result<String, String> {
    match kind {
        InstallKind::Content => store.install(path).map(|_| "Pack installed".into()),
        InstallKind::Integration => store
            .install_integration(path)
            .map(|_| "Integration installed".into()),
    }
}

fn oauth_metadata(
    bundle: &crate::twitch_auth::TokenBundle,
) -> Result<crate::extension_credentials::OAuthMetadata, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Cannot determine OAuth expiry")?
        .as_secs()
        .checked_add({
            let remaining = bundle
                .expires_at()
                .saturating_duration_since(std::time::Instant::now())
                .as_secs();
            if remaining == 0 {
                return Err("OAuth authorization expired before saving".into());
            }
            remaining
        })
        .ok_or("Invalid OAuth expiry")?;
    Ok(crate::extension_credentials::OAuthMetadata {
        client_id: bundle.client_id().into(),
        user_id: bundle.user_id().into(),
        scopes: bundle.scopes().iter().cloned().collect(),
        expires_at,
    })
}

/// Resolve only this installation's ID on the store worker. Never send saved
/// credential values to UI state, logs, ordinary settings or the process env.
fn device_auth_inputs(
    store: &ExtensionStore,
    expected: &InstalledPack,
    credential_revision: Option<Uuid>,
    gate: &AtomicU8,
    client_id: Option<String>,
) -> Result<(String, Vec<String>), String> {
    if gate.load(Ordering::Acquire) != 0 {
        return Err("Twitch authorization cancelled".into());
    }
    if expected.package_id != "studio.integration.twitch" || !expected.executable {
        return Err("Twitch integration is unavailable".into());
    }
    if client_id
        .as_ref()
        .is_some_and(|value| !crate::twitch_auth::valid_client_id(value))
    {
        return Err("The application ID must contain 1–128 ASCII letters or digits".into());
    }
    let (data, definitions) = store.credential_target_if_current(expected, credential_revision)?;
    if !["client_id", "access_token", "refresh_token"]
        .iter()
        .all(|id| definitions.iter().any(|definition| definition.id == *id))
    {
        return Err("Update the Twitch package before connecting".into());
    }
    let client_id = match client_id {
        Some(value) => value,
        None => crate::extension_credentials::read(&data, &definitions)?
            .remove("client_id")
            .ok_or("Saved Twitch application ID is missing; enter a new application ID")?,
    };
    if !crate::twitch_auth::valid_client_id(&client_id) {
        return Err("Saved Twitch application ID is invalid; enter a new application ID".into());
    }
    if gate.load(Ordering::Acquire) != 0 {
        return Err("Twitch authorization cancelled".into());
    }
    Ok((
        client_id,
        crate::twitch_worker::required_scopes(&expected.settings)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    ))
}

fn prepare_device_auth(
    store: &ExtensionStore,
    expected: &InstalledPack,
    credential_revision: Option<Uuid>,
    gate: &AtomicU8,
    client_id: Option<String>,
) -> Result<crate::twitch_auth::TwitchAuth, String> {
    let (client_id, scopes) =
        device_auth_inputs(store, expected, credential_revision, gate, client_id)?;
    if gate.load(Ordering::Acquire) != 0 {
        return Err("Twitch authorization cancelled".into());
    }
    let job = crate::twitch_auth::TwitchAuth::begin(client_id, scopes)
        .map_err(|error| error.to_string())?;
    if gate.load(Ordering::Acquire) != 0 {
        job.cancel();
    }
    Ok(job)
}

fn prepare_refresh(
    store: &ExtensionStore,
    expected: &InstalledPack,
    credential_revision: Option<Uuid>,
    gate: &AtomicU8,
) -> Result<crate::twitch_auth::TwitchAuth, String> {
    if gate.load(Ordering::Acquire) != 0 {
        return Err("Twitch authorization cancelled".into());
    }
    if !expected.enabled || expected.package_id != "studio.integration.twitch" {
        return Err("Twitch integration is not enabled".into());
    }
    let (data, definitions) = store.credential_target_if_current(expected, credential_revision)?;
    let metadata = crate::extension_credentials::read_oauth_metadata(&data, &definitions)?
        .ok_or("Connect with Twitch before automatic refresh")?;
    let mut values = crate::extension_credentials::read(&data, &definitions)?;
    let client_id = values
        .remove("client_id")
        .ok_or("Twitch application ID is missing")?;
    let refresh_token = values
        .remove("refresh_token")
        .ok_or("Twitch refresh token is missing")?;
    if client_id != metadata.client_id {
        return Err("Twitch account metadata changed; connect again".into());
    }
    let job = crate::twitch_auth::TwitchAuth::refresh(
        client_id,
        refresh_token,
        metadata.user_id,
        crate::twitch_worker::required_scopes(&expected.settings)
            .into_iter()
            .map(str::to_owned),
    )
    .map_err(|error| error.to_string())?;
    if gate.load(Ordering::Acquire) != 0 {
        job.cancel();
    }
    Ok(job)
}

fn outcome(store: &Result<ExtensionStore, String>, result: Result<String, String>) -> Outcome {
    let mut output = match store {
        Err(error) => Outcome {
            initial_load: false,
            auth_attempt: None,
            auth_job: None,
            oauth_schedule: BTreeMap::new(),
            packs: vec![],
            result: Err(error.clone()),
        },
        Ok(store) => Outcome {
            initial_load: false,
            auth_attempt: None,
            auth_job: None,
            oauth_schedule: BTreeMap::new(),
            packs: store
                .packs()
                .iter()
                .map(|installed| {
                    let resolved = store.resolved(installed).and_then(|content| {
                        let data_directory = installed
                            .executable
                            .then(|| store.data_directory(installed))
                            .transpose()?;
                        let credentials = data_directory
                            .as_ref()
                            .map(|data| {
                                crate::extension_credentials::status(
                                    data,
                                    &content.manifest.credentials,
                                )
                                .map_err(|_| {
                                    "Integration credential status is unavailable".to_string()
                                })
                            })
                            .transpose()?
                            .unwrap_or_default();
                        Ok((content, data_directory, credentials))
                    });
                    let (content, data_directory, credentials, error) = match resolved {
                        Ok((content, data_directory, credentials)) => {
                            (Some(content), data_directory, credentials, None)
                        }
                        Err(error) => (None, None, CredentialStatus::default(), Some(error)),
                    };
                    PackView {
                        installed: installed.clone(),
                        content,
                        data_directory,
                        credentials,
                        error,
                    }
                })
                .collect(),
            result,
        },
    };
    for pack in &output.packs {
        if pack.installed.package_id == "studio.integration.twitch"
            && let (Some(data), Some(content)) = (&pack.data_directory, &pack.content)
            && let Ok(Some(metadata)) = crate::extension_credentials::read_oauth_metadata(
                data,
                &content.manifest.credentials,
            )
        {
            output
                .oauth_schedule
                .insert(pack.installed.installation_id, metadata.expires_at);
        }
    }
    output
}

#[cfg(test)]
#[path = "twitch_reconnect_tests.rs"]
mod reconnect_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_settings::{
        MAX_STRING_BYTES, SettingDefinition, SettingKind, SettingValue,
    };
    use std::collections::BTreeSet;

    fn idle_state() -> (ExtensionState, Receiver<Command>) {
        let (sender, commands) = mpsc::sync_channel(1);
        let (_events, receiver) = mpsc::sync_channel(1);
        (
            ExtensionState {
                packs: vec![],
                busy: false,
                status: String::new(),
                error: None,
                generation: 0,
                initial_load_completed: false,
                twitch_auth: None,
                oauth_schedule: BTreeMap::new(),
                refresh_seen: BTreeMap::new(),
                refresh_not_before: None,
                cancelled: Arc::new(AtomicBool::new(false)),
                sender,
                receiver: Mutex::new(receiver),
            },
            commands,
        )
    }

    #[test]
    fn app_ready_initial_load_signal_survives_immediate_oauth_refresh_busy() {
        use crate::extension_lifecycle::{AppReadySource, LifecycleEvent, LifecycleJournal};
        let (mut state, commands) = idle_state();
        let (events, receiver) = mpsc::sync_channel(1);
        state.receiver = Mutex::new(receiver);
        state.busy = true;
        let id = Uuid::new_v4();
        let pack = PackView {
            installed: InstalledPack {
                installation_id: id,
                revision: Uuid::new_v4(),
                package_id: "studio.integration.twitch".into(),
                name: "Twitch".into(),
                version: "1.0.0".into(),
                enabled: true,
                executable: true,
                grants: vec![],
                settings: Settings::new(),
            },
            content: None,
            data_directory: None,
            error: None,
            credentials: CredentialStatus {
                revision: Some(Uuid::new_v4()),
                configured: BTreeSet::from(["refresh_token".into()]),
            },
        };
        let mut startup = AppReadySource::default();
        let mut journal = LifecycleJournal::default();
        startup.after_reconcile(state.initial_load_completed(), &mut journal);
        assert!(journal.drain().is_empty());
        events
            .send(Outcome {
                initial_load: true,
                auth_attempt: None,
                auth_job: None,
                oauth_schedule: BTreeMap::from([(id, 0)]),
                packs: vec![pack],
                result: Ok("Extensions loaded".into()),
            })
            .unwrap();
        state.poll();
        assert!(state.initial_load_completed());
        assert!(state.busy, "fixture must queue OAuth prep in the same poll");
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::PrepareRefresh { .. })
        ));
        // No OAuth worker is executed: only the bounded local command is queued.
        startup.after_reconcile(state.initial_load_completed(), &mut journal);
        assert!(matches!(
            journal.drain()[0].event,
            LifecycleEvent::AppReady { .. }
        ));
        startup.after_reconcile(state.initial_load_completed(), &mut journal);
        assert!(journal.drain().is_empty());
    }

    #[test]
    fn initial_load_failure_completes_startup_but_ordinary_receipt_is_not_initial_load() {
        use crate::extension_lifecycle::{AppReadySource, LifecycleEvent, LifecycleJournal};
        let (mut state, _commands) = idle_state();
        let (events, receiver) = mpsc::sync_channel(1);
        state.receiver = Mutex::new(receiver);
        for initial_load in [false, true] {
            events
                .send(Outcome {
                    initial_load,
                    auth_attempt: None,
                    auth_job: None,
                    oauth_schedule: BTreeMap::new(),
                    packs: vec![],
                    result: Err("Synthetic local load failure".into()),
                })
                .unwrap();
            state.poll();
            assert_eq!(state.initial_load_completed(), initial_load);
        }
        // Another local operation may already be busy after the load receipt.
        state.refresh();
        assert!(state.busy);
        let mut startup = AppReadySource::default();
        let mut journal = LifecycleJournal::default();
        startup.after_reconcile(state.initial_load_completed(), &mut journal);
        assert!(matches!(
            journal.drain()[0].event,
            LifecycleEvent::AppReady { .. }
        ));
    }

    #[test]
    fn authorized_bundle_commits_privately_once_without_enabling_or_granting() {
        use crate::twitch_auth::TokenBundle;
        let root = std::env::temp_dir().join(format!("waifu-auth-state-{}", Uuid::new_v4()));
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("run.py"), "#!/usr/bin/env python3\npass\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                source.join("run.py"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        std::fs::write(source.join("extension.json"), serde_json::to_vec(&serde_json::json!({
            "api_version":1,"id":"studio.integration.twitch","version":"1.0.0",
            "name":"Offline auth fixture","description":"No network or process launch",
            "provenance":{"author":"Fixture","license":"MIT"},"process":{"program":"run.py"},
            "credentials":[{"id":"client_id","label":"Application ID"},
                {"id":"access_token","label":"Access token"},{"id":"refresh_token","label":"Refresh token"}]
        })).unwrap()).unwrap();
        let mut state = ExtensionState::new(root.join("store"));
        settle(&mut state);
        state.install_integration(source);
        settle(&mut state);
        assert!(state.error.is_none());
        let pack = state.packs[0].clone();
        let id = pack.installed.installation_id;
        let attempt = Uuid::new_v4();
        state.twitch_auth = Some(AuthAttempt {
            preparing: false,
            id: attempt,
            expected: pack.installed.clone(),
            credential_revision: None,
            job: None,
            bundle: Some(TokenBundle::fixture("offlineClient")),
            gate: Arc::new(AtomicU8::new(0)),
            terminal: None,
        });
        state.poll_twitch_auth();
        assert!(state.busy);
        settle(&mut state);
        assert_eq!(
            state.twitch_auth_status(id).unwrap().status,
            "Authorization saved"
        );
        let saved = state.packs[0].credentials.revision;
        assert!(saved.is_some());
        assert!(!state.packs[0].installed.enabled);
        assert!(state.packs[0].installed.grants.is_empty());
        state.poll_twitch_auth();
        assert!(!state.busy);
        assert_eq!(state.packs[0].credentials.revision, saved);
        let index = std::fs::read_to_string(root.join("store/installed.json")).unwrap();
        assert!(!index.contains("offline-only-access-token"));
        assert!(!index.contains("offline-only-refresh-token"));
        // A cancelled validated completion must never replace the saved bundle.
        state.twitch_auth = Some(AuthAttempt {
            preparing: false,
            id: Uuid::new_v4(),
            expected: pack.installed.clone(),
            credential_revision: saved,
            job: None,
            bundle: Some(TokenBundle::fixture("replacementClient")),
            gate: Arc::new(AtomicU8::new(0)),
            terminal: None,
        });
        let cancelled = state.twitch_auth.as_ref().unwrap().id;
        state.cancel_twitch_auth(cancelled);
        state.poll_twitch_auth();
        assert!(!state.busy);
        assert_eq!(state.packs[0].credentials.revision, saved);
        assert_eq!(
            state.twitch_auth_status(id).unwrap().status,
            "Authorization cancelled"
        );
        // A late store rejection for a cancelled queued commit must retain the
        // cancellation receipt rather than suggest a failed/uncertain save.
        let (events, receiver) = mpsc::sync_channel(1);
        state.receiver = Mutex::new(receiver);
        events
            .send(Outcome {
                initial_load: false,
                auth_attempt: Some(cancelled),
                auth_job: None,
                oauth_schedule: BTreeMap::new(),
                packs: state.packs.clone(),
                result: Err("Twitch authorization cancelled".into()),
            })
            .unwrap();
        state.poll();
        assert_eq!(
            state.twitch_auth_status(id).unwrap().status,
            "Authorization cancelled"
        );
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_cannot_claim_to_undo_an_admitted_credential_write() {
        let (mut state, _commands) = idle_state();
        let installation_id = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        let gate = Arc::new(AtomicU8::new(2));
        state.twitch_auth = Some(AuthAttempt {
            preparing: false,
            id: attempt,
            expected: InstalledPack {
                installation_id,
                revision: Uuid::new_v4(),
                package_id: "studio.integration.twitch".into(),
                name: "Twitch".into(),
                version: "1.0.0".into(),
                enabled: false,
                executable: true,
                grants: vec![],
                settings: Settings::new(),
            },
            credential_revision: None,
            job: None,
            bundle: None,
            gate: gate.clone(),
            terminal: None,
        });
        state.cancel_twitch_auth(attempt);
        assert_eq!(gate.load(Ordering::Acquire), 2);
        let view = state.twitch_auth_status(installation_id).unwrap();
        assert_eq!(view.status, "Saving authorization");
        assert!(view.active);
        // A pending write is still cancellable and releases no token bundle.
        gate.store(0, Ordering::Release);
        state.cancel_twitch_auth(attempt);
        assert_eq!(gate.load(Ordering::Acquire), 1);
        let view = state.twitch_auth_status(installation_id).unwrap();
        assert_eq!(view.status, "Authorization cancelled");
        assert!(!view.active);
    }

    #[test]
    fn refresh_admission_is_enabled_due_once_per_revision_and_target_snapshot_bound() {
        let (mut state, commands) = idle_state();
        let installation_id = Uuid::new_v4();
        let revision = Uuid::new_v4();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        state.packs.push(PackView {
            installed: InstalledPack {
                installation_id,
                revision: Uuid::new_v4(),
                package_id: "studio.integration.twitch".into(),
                name: "Twitch".into(),
                version: "1.0.0".into(),
                enabled: false,
                executable: true,
                grants: vec![],
                settings: Settings::new(),
            },
            content: None,
            data_directory: None,
            error: None,
            credentials: CredentialStatus {
                revision: Some(revision),
                configured: ["refresh_token".into()].into(),
            },
        });
        state.oauth_schedule.insert(installation_id, now);
        state.schedule_twitch_refresh();
        assert!(!state.busy, "disabled installations must not refresh");
        state.packs[0].installed.enabled = true;
        state.oauth_schedule.insert(installation_id, now + 3600);
        state.schedule_twitch_refresh();
        assert!(!state.busy, "fresh tokens must not refresh");
        state.oauth_schedule.insert(installation_id, now + 120);
        state.schedule_twitch_refresh();
        assert!(state.busy);
        assert_eq!(
            state.twitch_auth_status(installation_id).unwrap().status,
            "Preparing authorization"
        );
        let command = commands.try_recv().unwrap();
        match command {
            Command::PrepareRefresh {
                expected,
                credential_revision,
                ..
            } => {
                assert_eq!(expected.installation_id, installation_id);
                assert_eq!(credential_revision, Some(revision));
            }
            _ => panic!("expected private refresh preparation"),
        }
        // A failed/uncertain rotating grant is never retried for the same revision.
        state.busy = false;
        state.refresh_not_before = None;
        let auth = state.twitch_auth.as_mut().unwrap();
        auth.terminal = Some("Refresh failed".into());
        auth.gate.store(4, Ordering::Release);
        state.schedule_twitch_refresh();
        assert!(!state.busy);
        assert!(commands.try_recv().is_err());
        state.packs[0].credentials.revision = Some(Uuid::new_v4());
        state.schedule_twitch_refresh();
        assert!(
            state.busy,
            "a new credential revision permits a new attempt"
        );
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::PrepareRefresh { .. })
        ));
        state.packs[0].installed.enabled = false;
        state.poll_twitch_auth();
        assert_eq!(
            state
                .twitch_auth
                .as_ref()
                .unwrap()
                .gate
                .load(Ordering::Acquire),
            1
        );
        assert_eq!(
            state.twitch_auth_status(installation_id).unwrap().status,
            "Integration changed; connect again"
        );
    }

    #[test]
    fn cancelled_refresh_prepare_receipt_retains_live_worker_admission_until_reaped() {
        let (mut state, commands) = idle_state();
        let installation_id = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        let expected = InstalledPack {
            installation_id,
            revision: Uuid::new_v4(),
            package_id: "studio.integration.twitch".into(),
            name: "Twitch".into(),
            version: "1.0.0".into(),
            enabled: false,
            executable: true,
            grants: vec![],
            settings: Settings::new(),
        };
        state.packs.push(PackView {
            installed: expected.clone(),
            content: None,
            data_directory: None,
            credentials: CredentialStatus::default(),
            error: None,
        });
        state.twitch_auth = Some(AuthAttempt {
            preparing: true,
            id: attempt,
            expected,
            credential_revision: None,
            job: None,
            bundle: None,
            gate: Arc::new(AtomicU8::new(0)),
            terminal: None,
        });
        state.busy = true;
        let (job, release) = crate::twitch_auth::TwitchAuth::held_fixture();
        assert!(
            !job.is_finished(),
            "controlled worker must be live before delivery"
        );
        // Consent is cancelled after worker creation, before its prepare receipt.
        state.cancel_twitch_auth(attempt);
        let (events, receiver) = mpsc::sync_channel(1);
        state.receiver = Mutex::new(receiver);
        assert!(
            events
                .send(Outcome {
                    initial_load: false,
                    auth_attempt: Some(attempt),
                    auth_job: Some(job),
                    oauth_schedule: BTreeMap::new(),
                    packs: state.packs.clone(),
                    result: Ok("Refreshing Twitch authorization".into())
                })
                .is_ok()
        );
        state.poll();
        assert!(!state.busy);
        let view = state.twitch_auth_status(installation_id).unwrap();
        assert_eq!(view.status, "Authorization cancelled");
        assert!(view.active, "cancelled live worker must retain admission");
        assert!(
            state
                .start_twitch_auth(installation_id, "offlineClient".into())
                .unwrap_err()
                .contains("busy")
        );
        assert!(
            commands.try_recv().is_err(),
            "cancelled result must not queue a save"
        );
        release.store(true, Ordering::Release);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while state.twitch_auth.as_ref().unwrap().job.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "released worker did not finish"
            );
            state.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!state.twitch_auth_status(installation_id).unwrap().active);
        assert!(commands.try_recv().is_err());
        assert!(
            !state
                .start_twitch_auth(installation_id, "offlineClient".into())
                .unwrap_err()
                .contains("busy")
        );
    }

    #[test]
    fn busy_submission_is_visible_and_does_not_queue_another_command() {
        let (mut state, commands) = idle_state();
        state.refresh();
        state.pick_install();
        state.pick_install_integration();
        state.install_integration(PathBuf::from("integration"));
        state.set_grants(Uuid::new_v4(), vec![Capability::ReplyRead]);
        state.set_settings(Uuid::new_v4(), Settings::new());
        assert!(state.busy);
        assert!(state.error.as_ref().unwrap().contains("busy"));
        assert!(matches!(commands.try_recv(), Ok(Command::Refresh)));
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn explicit_integration_and_grant_commands_preserve_the_selected_input() {
        let (mut state, commands) = idle_state();
        let folder = PathBuf::from("chosen-integration");
        state.install_integration(folder.clone());
        assert!(
            matches!(commands.try_recv(), Ok(Command::InstallIntegration(path)) if path == folder)
        );
        state.busy = false;
        state.pick_install_integration();
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::PickInstallIntegration)
        ));
        state.busy = false;
        let id = Uuid::new_v4();
        state.set_grants(
            id,
            vec![Capability::LifecycleObserve, Capability::ReplyRead],
        );
        assert!(
            matches!(commands.try_recv(), Ok(Command::Grants(actual, grants))
            if actual == id && grants == vec![Capability::LifecycleObserve, Capability::ReplyRead])
        );
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn settings_command_preserves_typed_draft_and_rejects_unbounded_values_before_queueing() {
        let (mut state, commands) = idle_state();
        let id = Uuid::new_v4();
        let settings = Settings::from([
            ("enabled".into(), SettingValue::Bool(false)),
            ("count".into(), SettingValue::Integer(7)),
            ("label".into(), SettingValue::Text("hello".into())),
        ]);
        state.set_settings(id, settings.clone());
        assert!(state.busy);
        assert!(
            state.packs.is_empty(),
            "drafts must not optimistically alter the catalog"
        );
        assert!(
            matches!(commands.try_recv(), Ok(Command::Settings(actual, values))
            if actual == id && values == settings)
        );
        state.busy = false;
        state.set_settings(
            id,
            Settings::from([(
                "label".into(),
                SettingValue::Text("x".repeat(MAX_STRING_BYTES + 1)),
            )]),
        );
        assert!(state.error.is_some());
        assert!(!state.busy);
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn drop_cancels_queued_commands_and_discards_picker_result() {
        for kind in [InstallKind::Content, InstallKind::Integration] {
            let root = std::env::temp_dir().join(format!("waifu-worker-{}", Uuid::new_v4()));
            let mut store = ExtensionStore::open(root.clone()).unwrap();
            let (mut state, commands) = idle_state();
            let cancelled = state.cancelled.clone();
            state.refresh();
            // Model a picker already open when the owning state is dropped.
            let result = install_picked(&mut store, &cancelled, kind, || {
                drop(state);
                Some(root.join("missing-source-must-not-be-inspected"))
            });
            assert_eq!(result.unwrap_err(), "Extension operation cancelled");
            assert!(cancelled.load(Ordering::Acquire));
            assert!(matches!(commands.try_recv(), Ok(Command::Refresh)));
            assert!(store.packs().is_empty());
            assert!(!root.join("installed.json").exists());
            assert!(ExtensionStore::open(root.clone()).is_err());
            // Already cancelled commands must not even open another picker.
            assert!(
                install_picked(&mut store, &cancelled, kind, || panic!("stale picker")).is_err()
            );
            drop(store);
            drop(ExtensionStore::open(root.clone()).unwrap());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("waifu-install-worker-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn integration(&self, version: &str) -> PathBuf {
            let source = self.0.join("source");
            std::fs::create_dir_all(&source).unwrap();
            let program = source.join("integration");
            std::fs::write(&program, b"#!/bin/sh\nexit 7\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let manifest = serde_json::json!({
                "api_version": 1,
                "id": "org.example.worker",
                "name": "Worker fixture",
                "version": version,
                "description": "Install only; never launch",
                "provenance": {"author": "Fixture", "license": "MIT"},
                "process": {"program": "integration", "args": []},
                "capabilities": ["catalog.get", "reply.read"]
            });
            std::fs::write(
                source.join("extension.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            source
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn settle(state: &mut ExtensionState) {
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_secs(4);
        while state.busy {
            state.poll();
            assert!(
                Instant::now() < deadline,
                "extension worker timed out: {:?}",
                state.error
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_credentials_publish_only_status_and_bound_drafts_before_queueing() {
        let dir = TestDir::new();
        let source = dir.integration("1.0.0");
        let path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["credentials"] = serde_json::json!([{"id":"token", "label":"Access token"}]);
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let root = dir.0.join("store");
        let mut state = ExtensionState::new(root.clone());
        settle(&mut state);
        state.install_integration(source);
        settle(&mut state);
        assert!(state.error.is_none());
        let installed = state.packs[0].installed.clone();
        let id = installed.installation_id;
        assert_eq!(state.packs[0].credentials, CredentialStatus::default());
        let updates = BTreeMap::from([("token".into(), Some("private-worker-fixture".into()))]);
        state.set_credentials(id, updates.clone());
        assert!(state.busy);
        assert_eq!(
            state.packs[0].credentials,
            CredentialStatus::default(),
            "no optimistic status"
        );
        settle(&mut state);
        assert!(state.error.is_none());
        let configured = state.packs[0].credentials.clone();
        assert!(configured.revision.is_some());
        assert!(configured.configured.contains("token"));
        assert_eq!(state.packs[0].installed, installed);
        assert!(!format!("{:?}", state.packs).contains("private-worker-fixture"));
        state.set_credentials(id, updates);
        settle(&mut state);
        assert_eq!(state.packs[0].credentials, configured);
        for updates in [
            BTreeMap::from([("unknown".into(), Some("private-rejected-fixture".into()))]),
            BTreeMap::from([("token".into(), Some("x".repeat(4097)))]),
        ] {
            state.set_credentials(id, updates);
            assert!(!state.busy, "unbounded draft reached the worker queue");
            assert!(state.error.is_some());
            assert!(
                !state
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("private-rejected-fixture")
            );
            assert_eq!(state.packs[0].credentials, configured);
        }
        state.refresh();
        settle(&mut state);
        assert!(state.error.is_none());
        assert_eq!(state.packs[0].credentials, configured);
        state.set_credentials(id, BTreeMap::from([("token".into(), None)]));
        settle(&mut state);
        assert!(state.error.is_none());
        assert!(state.packs[0].credentials.configured.is_empty());
        assert_ne!(state.packs[0].credentials.revision, configured.revision);
        assert_eq!(state.packs[0].installed, installed);
        drop(state);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            if let Ok(store) = ExtensionStore::open(root.clone()) {
                drop(store);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker retained store lock"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_saves_complete_typed_settings_and_invalid_changes_preserve_persisted_values() {
        let dir = TestDir::new();
        let source = dir.integration("1.0.0");
        let definitions = vec![
            SettingDefinition {
                id: "enabled".into(),
                label: "Enabled".into(),
                description: String::new(),
                kind: SettingKind::Toggle { default: true },
            },
            SettingDefinition {
                id: "count".into(),
                label: "Count".into(),
                description: String::new(),
                kind: SettingKind::Integer {
                    default: 2,
                    min: 0,
                    max: 10,
                },
            },
            SettingDefinition {
                id: "label".into(),
                label: "Label".into(),
                description: String::new(),
                kind: SettingKind::Text {
                    default: "default".into(),
                    max_length: 32,
                },
            },
            SettingDefinition {
                id: "style".into(),
                label: "Style".into(),
                description: String::new(),
                kind: SettingKind::Choice {
                    default: "brief".into(),
                    options: vec!["brief".into(), "full".into()],
                },
            },
        ];
        let path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["settings"] = serde_json::to_value(definitions).unwrap();
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let root = dir.0.join("store");
        let mut state = ExtensionState::new(root.clone());
        settle(&mut state);
        state.install_integration(source);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        let id = state.packs[0].installed.installation_id;
        assert_eq!(
            state.packs[0].installed.settings.get("count"),
            Some(&SettingValue::Integer(2))
        );
        state.set_enabled(id, true);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        let settings = Settings::from([
            ("enabled".into(), SettingValue::Bool(false)),
            ("count".into(), SettingValue::Integer(7)),
            ("label".into(), SettingValue::Text("saved".into())),
            ("style".into(), SettingValue::Text("full".into())),
        ]);
        state.set_settings(id, settings.clone());
        assert!(state.busy);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.packs[0].installed.settings, settings);
        assert!(state.packs[0].installed.enabled);

        let mut invalid = settings.clone();
        invalid.insert("count".into(), SettingValue::Integer(11));
        state.set_settings(id, invalid);
        settle(&mut state);
        assert!(state.error.is_some());
        assert_eq!(state.packs[0].installed.settings, settings);
        state.refresh(); // Reopens the store from disk, not the previous catalog.
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.packs[0].installed.settings, settings);
        assert!(state.packs[0].installed.enabled);
        drop(state);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            if let Ok(store) = ExtensionStore::open(root.clone()) {
                drop(store);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker retained store lock"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_installs_executables_only_explicitly_and_caches_stable_data_directory() {
        let dir = TestDir::new();
        let source = dir.integration("1.0.0");
        let root = dir.0.join("store");
        let mut state = ExtensionState::new(root.clone());
        settle(&mut state);
        assert!(state.error.is_none());
        state.install(source.clone());
        settle(&mut state);
        assert!(state.error.is_some());
        assert!(state.packs.is_empty());

        state.install_integration(source.clone());
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        let installed = &state.packs[0];
        assert!(installed.installed.executable);
        assert!(!installed.installed.enabled);
        assert!(installed.installed.grants.is_empty());
        assert!(
            installed
                .content
                .as_ref()
                .unwrap()
                .manifest
                .process
                .is_some()
        );
        let id = installed.installed.installation_id;
        let revision = installed.installed.revision;
        let data = installed.data_directory.clone().unwrap();
        assert!(data.is_absolute() && data.is_dir());
        assert!(state.active().next().is_none());
        std::fs::write(data.join("retained.txt"), b"per-install state").unwrap();

        state.set_grants(id, vec![Capability::ReplyRead]);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.packs[0].installed.grants, vec![Capability::ReplyRead]);
        assert!(!state.packs[0].installed.enabled);
        state.set_grants(id, vec![Capability::MotionPlay]);
        settle(&mut state);
        assert!(state.error.is_some());
        assert_eq!(state.packs[0].installed.grants, vec![Capability::ReplyRead]);
        state.set_enabled(id, true);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.active().count(), 1);
        state.set_grants(id, vec![]);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(state.packs[0].installed.grants.is_empty());

        dir.integration("2.0.0");
        state.install_integration(source);
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.packs[0].installed.installation_id, id);
        assert_ne!(state.packs[0].installed.revision, revision);
        assert_eq!(state.packs[0].data_directory.as_ref(), Some(&data));
        state.refresh();
        settle(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.packs[0].data_directory.as_ref(), Some(&data));
        assert_eq!(
            std::fs::read(data.join("retained.txt")).unwrap(),
            b"per-install state"
        );

        drop(state);
        // The production Drop stays nonblocking. Wait here only so fixture
        // cleanup cannot remove a store still owned by its worker.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            if let Ok(store) = ExtensionStore::open(root.clone()) {
                drop(store);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker retained store lock"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
