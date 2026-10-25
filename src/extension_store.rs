//! Durable content-pack and explicitly opted-in executable installation.
//! Executables are never started here; grants authorize host APIs, not OS access.
//! Revisions are immutable: an index replacement activates an update without
//! replacing files used by an existing character or a running animation.
//! An exclusive OS lock serializes store lifetimes. Filesystem checks are
//! snapshots; this module does not provide an OS sandbox.
use crate::{
    extension_manifest::{Manifest, ResolvedPack, load_integration_pack, load_pack},
    extension_protocol::Capability,
    extension_settings::{
        MAX_SETTINGS_BYTES, Settings, migrate_values, validate_values, validate_wire_values,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_PACKS: usize = 128;
const MAX_INDEX_BYTES: u64 = 1024 * 1024;
const MAX_FILES: usize = 4096;
const MAX_COPY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstalledPack {
    pub installation_id: Uuid,
    pub revision: Uuid,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    #[serde(default)]
    pub executable: bool,
    #[serde(default)]
    pub grants: Vec<Capability>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    packs: Vec<InstalledPack>,
}

pub struct ExtensionStore {
    root: PathBuf,
    index: Index,
    // Never unlink the lock file: replacements must lock the same inode.
    _lock: StoreLock,
}

/// An OS lock belongs to this store lifetime, not to accidental inherited
/// descriptors. Closing alone retains flock through dup/fork until the last
/// shared descriptor closes; CLOEXEC does not cover the pre-exec window.
struct StoreLock(fs::File);
impl Drop for StoreLock {
    fn drop(&mut self) {
        // Unlock before File closes. Different open-file descriptions (the next
        // store owner) are unaffected by later closes of an old inherited copy.
        let _ = self.0.unlock();
    }
}

/// Once rename succeeds, memory and revisions must follow the new index even
/// if syncing its directory fails. Returning an error must not delete its files.
#[derive(Debug)]
struct SaveError {
    message: String,
    committed: bool,
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("Cannot sync directory '{}': {e}", path.display()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn real_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Store directory '{}' must be a real directory, not a symbolic link",
            path.display()
        ));
    }
    Ok(())
}

fn requested_capabilities(manifest: &Manifest) -> Result<Vec<Capability>, String> {
    manifest
        .capabilities
        .iter()
        .map(|name| {
            serde_json::from_value(serde_json::Value::String(name.clone()))
                .map_err(|_| format!("Unknown integration capability '{name}'"))
        })
        .collect()
}

fn load_for_policy(root: &Path, executable: bool) -> Result<ResolvedPack, String> {
    if executable {
        load_integration_pack(root)
    } else {
        load_pack(root)
    }
}

/// Count compact serialized bytes without allocating another copy of values.
/// In particular, reject a large caller-owned text before validation clones it.
fn validate_settings_size(settings: &Settings) -> Result<(), String> {
    struct SizeLimit(usize);
    impl Write for SizeLimit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_SETTINGS_BYTES.saturating_sub(self.0) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Settings exceed the 16 KiB serialized size limit",
                ));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(SizeLimit(0), settings)
        .map_err(|error| format!("Invalid extension settings: {error}"))?;
    validate_wire_values(settings)
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Extension metadata must be a regular file, not a symbolic link".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() {
        return Err("Extension metadata must be a regular file".into());
    }
    if opened.len() > limit {
        return Err("Extension metadata exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Extension metadata exceeds its size limit".into());
    }
    Ok(bytes)
}

impl ExtensionStore {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let lock_path = root.join(".store.lock");
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                return Err("Extension store lock must be a regular file".into());
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.to_string()),
        }
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("Cannot open extension store lock: {e}"))?;
        lock.try_lock()
            .map_err(|e| format!("Extension store is busy or cannot be locked: {e}"))?;
        // Install the guard immediately so all subsequent open-error paths also
        // release the owner's lock even if a concurrent spawn inherited its fd.
        let lock = StoreLock(lock);
        let packages = root.join("packages");
        match fs::symlink_metadata(&packages) {
            Ok(_) => real_directory(&packages)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&packages).map_err(|e| e.to_string())?;
                sync_directory(&root)?;
            }
            Err(error) => return Err(error.to_string()),
        }
        let path = root.join("installed.json");
        let index: Index = match fs::symlink_metadata(&path) {
            Ok(_) => {
                let bytes = read_limited(&path, MAX_INDEX_BYTES).map_err(|e| {
                    format!("Cannot read extension list; original file retained: {e}")
                })?;
                serde_json::from_slice(&bytes).map_err(|e| {
                    format!("Cannot read extension list; original file retained: {e}")
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Index {
                version: 1,
                packs: vec![],
            },
            Err(error) => {
                return Err(format!(
                    "Cannot inspect extension list; original file retained: {error}"
                ));
            }
        };
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for pack in &index.packs {
            validate_settings_size(&pack.settings).map_err(|error| {
                format!("Cannot read extension list; original file retained: {error}")
            })?;
        }
        if index.version != 1
            || index.packs.len() > MAX_PACKS
            || index.packs.iter().any(|p| {
                p.package_id.trim().is_empty()
                    || p.package_id.len() > 128
                    || p.name.trim().is_empty()
                    || p.name.len() > 256
                    || p.version.trim().is_empty()
                    || p.version.len() > 64
                    || (!p.executable && !p.grants.is_empty())
                    || p.grants
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != p.grants.len()
                    || !ids.insert(p.installation_id)
                    || !names.insert(p.package_id.clone())
            })
        {
            return Err("Invalid or unsupported extension list; original file retained".into());
        }
        reap_debris(&root, ".installed-", ".tmp", false)?;
        reap_debris(&packages, ".stage-", "", true)?;
        Ok(Self {
            root,
            index,
            _lock: lock,
        })
    }

    pub fn packs(&self) -> &[InstalledPack] {
        &self.index.packs
    }

    pub fn directory(&self, pack: &InstalledPack) -> PathBuf {
        self.root
            .join("packages")
            .join(pack.installation_id.to_string())
            .join(pack.revision.to_string())
    }

    /// Revalidate the installed copy, not the directory from which it was added.
    pub fn validate(&self, pack: &InstalledPack) -> Result<(), String> {
        self.resolved(pack).map(|_| ())
    }

    /// Resolve using the durable trust type and reject stale/forged descriptors.
    /// Also validates persisted settings and that grants remain requested.
    pub fn resolved(&self, pack: &InstalledPack) -> Result<ResolvedPack, String> {
        self.require_current(pack)?;
        let directory = self.directory(pack);
        real_directory(&self.root)?;
        real_directory(&self.root.join("packages"))?;
        real_directory(directory.parent().unwrap())?;
        real_directory(&directory)?;
        let resolved = directory.canonicalize().map_err(|e| e.to_string())?;
        if !resolved.starts_with(self.root.join("packages")) {
            return Err("Installed package points outside the extension store".into());
        }
        let validated = load_for_policy(&directory, pack.executable)?;
        if validated.manifest.id != pack.package_id
            || validated.manifest.version != pack.version
            || validated.manifest.name != pack.name
        {
            return Err("Installed package identity differs from the extension list".into());
        }
        let requested = requested_capabilities(&validated.manifest)?;
        if pack.grants.iter().any(|grant| !requested.contains(grant)) {
            return Err(
                "Installed grants include a capability no longer requested by the package".into(),
            );
        }
        validate_settings_size(&pack.settings)?;
        let settings = validate_values(&validated.manifest.settings, &pack.settings)?;
        validate_settings_size(&settings)?;
        Ok(validated)
    }

    fn require_current(&self, pack: &InstalledPack) -> Result<(), String> {
        if !self.index.packs.contains(pack) {
            return Err("Extension descriptor is stale or is no longer installed".into());
        }
        Ok(())
    }

    /// Worker-side filesystem operation. Writable data belongs to the stable
    /// installation, not a revision, and is retained on update and removal.
    /// Directory checks are snapshots, not protection against hostile OS code.
    pub fn data_directory(&self, pack: &InstalledPack) -> Result<PathBuf, String> {
        self.require_current(pack)?;
        if !pack.executable {
            return Err("Only executable integrations have a writable data directory".into());
        }
        real_directory(&self.root)?;
        let data = self.root.join("data");
        let directory = data.join(pack.installation_id.to_string());
        for path in [&data, &directory] {
            match fs::symlink_metadata(path) {
                Ok(_) => real_directory(path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let mut builder = fs::DirBuilder::new();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::DirBuilderExt;
                        builder.mode(0o700);
                    }
                    builder.create(path).map_err(|e| e.to_string())?;
                }
                Err(error) => return Err(error.to_string()),
            }
            real_directory(path)?;
            // Repeat syncs on existing directories too, so a previous sync
            // error can be retried without publishing a nondurable data path.
            sync_directory(path)?;
            sync_directory(path.parent().unwrap())?;
        }
        let canonical = directory.canonicalize().map_err(|e| e.to_string())?;
        if canonical != directory {
            return Err("Integration data directory points outside its installation".into());
        }
        Ok(canonical)
    }

    pub fn install(&mut self, source: &Path) -> Result<Uuid, String> {
        self.install_with_sync(source, sync_directory)
    }

    /// Explicit opt-in to install local code. New installations are disabled
    /// with no grants. This does not start code or provide a process sandbox.
    pub fn install_integration(&mut self, source: &Path) -> Result<Uuid, String> {
        self.install_policy_with_sync(source, true, sync_directory)
    }

    fn install_with_sync(
        &mut self,
        source: &Path,
        sync: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<Uuid, String> {
        self.install_policy_with_sync(source, false, sync)
    }

    fn install_policy_with_sync(
        &mut self,
        source: &Path,
        executable: bool,
        sync: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<Uuid, String> {
        real_directory(&self.root)?;
        real_directory(&self.root.join("packages"))?;
        real_directory(source)?;
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        if source.starts_with(&self.root) || self.root.starts_with(&source) {
            return Err("Choose a package folder outside the extension store".into());
        }
        let metadata = load_for_policy(&source, executable)?.manifest;
        let package_id = metadata.id;
        let existing = self
            .index
            .packs
            .iter()
            .position(|p| p.package_id == package_id);
        if existing.is_some_and(|i| self.index.packs[i].executable != executable) {
            return Err("An update cannot change a package between content-only and executable; remove it before explicitly reinstalling with a different trust type".into());
        }
        if existing.is_none() && self.index.packs.len() >= MAX_PACKS {
            return Err(format!("At most {MAX_PACKS} extensions can be installed"));
        }
        let mut pack = InstalledPack {
            installation_id: existing
                .map(|i| self.index.packs[i].installation_id)
                .unwrap_or_else(Uuid::new_v4),
            revision: Uuid::new_v4(),
            package_id,
            name: metadata.name,
            version: metadata.version,
            enabled: existing.map_or(!executable, |i| self.index.packs[i].enabled),
            executable,
            grants: vec![],
            settings: Settings::new(),
        };
        let target = self.directory(&pack);
        let stage = self
            .root
            .join("packages")
            .join(format!(".stage-{}", Uuid::new_v4()));
        let mut created_parent = false;
        let result = (|| {
            copy_tree(&source, &stage)?;
            let copied = load_for_policy(&stage, executable)?.manifest;
            if copied.id != pack.package_id
                || copied.version != pack.version
                || copied.name != pack.name
            {
                return Err("Package identity changed while it was being copied".into());
            }
            // Use the copied manifest: the source can change during the copy.
            // Newly requested capabilities never become grants automatically.
            let requested = requested_capabilities(&copied)?;
            if let Some(i) = existing {
                pack.grants = self.index.packs[i]
                    .grants
                    .iter()
                    .copied()
                    .filter(|grant| requested.contains(grant))
                    .collect();
            }
            let empty = Settings::new();
            let old_settings = existing.map_or(&empty, |i| &self.index.packs[i].settings);
            pack.settings = migrate_values(&copied.settings, old_settings)?;
            validate_settings_size(&pack.settings)?;
            let parent = target.parent().unwrap();
            match fs::symlink_metadata(parent) {
                Ok(_) => real_directory(parent)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(parent).map_err(|e| e.to_string())?;
                    created_parent = true;
                }
                Err(error) => return Err(error.to_string()),
            }
            fs::rename(&stage, &target).map_err(|e| e.to_string())?;
            // Publish files durably before an index can refer to this revision.
            if let Err(error) =
                sync_directory(parent).and_then(|()| sync_directory(&self.root.join("packages")))
            {
                let _ = fs::remove_dir_all(&target);
                return Err(error);
            }
            let previous = self.index.clone();
            if let Some(i) = existing {
                self.index.packs[i] = pack.clone();
            } else {
                self.index.packs.push(pack.clone());
            }
            if let Err(error) = self.save_with_sync(sync) {
                if !error.committed {
                    self.index = previous;
                    let _ = fs::remove_dir_all(&target);
                }
                return Err(error.message);
            }
            Ok(pack.installation_id)
        })();
        let _ = fs::remove_dir_all(stage);
        if result.is_err() && created_parent {
            // Only removes an empty parent, never a committed/retained revision.
            let _ = fs::remove_dir(target.parent().unwrap());
        }
        result
    }

    pub fn set_enabled(&mut self, id: Uuid, enabled: bool) -> Result<(), String> {
        let index = self
            .index
            .packs
            .iter()
            .position(|p| p.installation_id == id)
            .ok_or("Extension is no longer installed")?;
        if enabled {
            self.validate(&self.index.packs[index])?;
        }
        let previous = self.index.packs[index].enabled;
        self.index.packs[index].enabled = enabled;
        if let Err(error) = self.save() {
            if !error.committed {
                self.index.packs[index].enabled = previous;
            }
            return Err(error.message);
        }
        Ok(())
    }

    /// Replace the complete grant set; every grant must be explicitly requested.
    /// Known capabilities are enforced by the typed enum and strict deserializer.
    pub fn set_grants(&mut self, id: Uuid, grants: Vec<Capability>) -> Result<(), String> {
        self.set_grants_with_sync(id, grants, sync_directory)
    }

    fn set_grants_with_sync(
        &mut self,
        id: Uuid,
        mut grants: Vec<Capability>,
        sync: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        let index = self
            .index
            .packs
            .iter()
            .position(|pack| pack.installation_id == id)
            .ok_or("Extension is no longer installed")?;
        let pack = &self.index.packs[index];
        if !pack.executable {
            return Err("Content-only packs cannot receive executable capability grants".into());
        }
        let resolved = self.resolved(pack)?;
        let requested = requested_capabilities(&resolved.manifest)?;
        grants.sort_unstable();
        if grants.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("Capability grants must be unique".into());
        }
        if let Some(grant) = grants.iter().find(|grant| !requested.contains(grant)) {
            return Err(format!(
                "Capability {grant:?} was not requested by this integration"
            ));
        }
        let previous = std::mem::replace(&mut self.index.packs[index].grants, grants);
        if let Err(error) = self.save_with_sync(sync) {
            if !error.committed {
                self.index.packs[index].grants = previous;
            }
            return Err(error.message);
        }
        Ok(())
    }

    /// Replace typed settings, filling declared defaults without changing grants.
    /// Only executable integrations accept user-configurable process settings.
    pub fn set_settings(&mut self, id: Uuid, settings: Settings) -> Result<(), String> {
        self.set_settings_with_sync(id, settings, sync_directory)
    }

    fn set_settings_with_sync(
        &mut self,
        id: Uuid,
        settings: Settings,
        sync: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        validate_settings_size(&settings)?;
        let index = self
            .index
            .packs
            .iter()
            .position(|pack| pack.installation_id == id)
            .ok_or("Extension is no longer installed")?;
        let pack = &self.index.packs[index];
        if !pack.executable {
            return Err("Content-only packs cannot receive executable settings".into());
        }
        let resolved = self.resolved(pack)?;
        let settings = validate_values(&resolved.manifest.settings, &settings)?;
        validate_settings_size(&settings)?;
        let previous = std::mem::replace(&mut self.index.packs[index].settings, settings);
        if let Err(error) = self.save_with_sync(sync) {
            if !error.committed {
                self.index.packs[index].settings = previous;
            }
            return Err(error.message);
        }
        Ok(())
    }

    /// Worker-only private credential update, separate from the durable package
    /// index/settings. The credential store owns validation and atomic saving;
    /// values never become installed metadata or returned catalog data.
    pub fn set_credentials(
        &mut self,
        id: Uuid,
        updates: BTreeMap<String, Option<String>>,
    ) -> Result<(), String> {
        let pack = self
            .index
            .packs
            .iter()
            .find(|pack| pack.installation_id == id)
            .ok_or("Extension is no longer installed")?;
        if !pack.executable {
            return Err("Content-only packs cannot receive integration credentials".into());
        }
        let resolved = self.resolved(pack)?;
        let data = self.data_directory(pack)?;
        crate::extension_credentials::update(&data, &resolved.manifest.credentials, &updates)
            .map(|_| ())
    }

    /// Commit a background credential result only against its captured install
    /// and credential state. Call on the single store-owner worker; this never
    /// enables an installation or grants a capability.
    pub fn set_credentials_if_current(
        &mut self,
        expected: &InstalledPack,
        credential_revision: Option<Uuid>,
        updates: BTreeMap<String, Option<String>>,
    ) -> Result<(), String> {
        self.credential_target_if_current(expected, credential_revision)?;
        self.set_credentials(expected.installation_id, updates)
    }

    /// Save a validated OAuth rotation and its identity/expiry in one private
    /// file replacement; never separate token writes from their metadata.
    pub fn set_oauth_credentials_if_current(
        &mut self,
        expected: &InstalledPack,
        credential_revision: Option<Uuid>,
        updates: BTreeMap<String, Option<String>>,
        metadata: &crate::extension_credentials::OAuthMetadata,
    ) -> Result<(), String> {
        let (data, definitions) =
            self.credential_target_if_current(expected, credential_revision)?;
        crate::extension_credentials::update_oauth(&data, &definitions, &updates, metadata)
            .map(|_| ())
    }

    /// Read-only snapshot gate shared by preparing and committing authorization.
    pub fn credential_target_if_current(
        &self,
        expected: &InstalledPack,
        credential_revision: Option<Uuid>,
    ) -> Result<
        (
            PathBuf,
            Vec<crate::extension_credentials::CredentialDefinition>,
        ),
        String,
    > {
        let current = self
            .index
            .packs
            .iter()
            .find(|pack| pack.installation_id == expected.installation_id)
            .ok_or("Extension is no longer installed")?;
        if current != expected {
            return Err("Integration changed during authorization; connect again".into());
        }
        let resolved = self.resolved(current)?;
        let data = self.data_directory(current)?;
        let status = crate::extension_credentials::status(&data, &resolved.manifest.credentials)?;
        if status.revision != credential_revision {
            return Err(
                "Integration credentials changed during authorization; connect again".into(),
            );
        }
        Ok((data, resolved.manifest.credentials))
    }

    /// Unregister without deleting files potentially retained by user settings.
    /// Old revisions remain available to imported/customized user assets.
    pub fn remove(&mut self, id: Uuid) -> Result<(), String> {
        let i = self
            .index
            .packs
            .iter()
            .position(|p| p.installation_id == id)
            .ok_or("Extension is no longer installed")?;
        let removed = self.index.packs.remove(i);
        if let Err(error) = self.save() {
            if !error.committed {
                self.index.packs.insert(i, removed);
            }
            return Err(error.message);
        }
        Ok(())
    }

    fn save(&self) -> Result<(), SaveError> {
        self.save_with_sync(sync_directory)
    }

    fn save_with_sync(
        &self,
        sync: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), SaveError> {
        let path = self.root.join("installed.json");
        let temporary = self.root.join(format!(".installed-{}.tmp", Uuid::new_v4()));
        let mut committed = false;
        let result = (|| -> Result<(), String> {
            for pack in &self.index.packs {
                validate_settings_size(&pack.settings)?;
            }
            // Index sizes are bounded by MAX_PACKS and validated metadata lengths.
            let bytes = serde_json::to_vec_pretty(&self.index).map_err(|e| e.to_string())?;
            if bytes.len() as u64 > MAX_INDEX_BYTES {
                return Err("Extension list exceeds its size limit".into());
            }
            // Do not silently replace a symlink or nonregular index after open.
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    return Err("Extension list must be a regular file, not a symbolic link".into());
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.to_string()),
            }
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            drop(file);
            fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
            committed = true;
            sync(&self.root).map_err(|error| format!("Extension list was replaced but its directory could not be synced; new state retained: {error}"))
        })();
        let _ = fs::remove_file(temporary);
        result.map_err(|message| SaveError { message, committed })
    }
}

// Only exact names emitted by this installer qualify. Do not follow symlinks
// or inspect installation UUID directories: their revisions may be retained.
fn reap_debris(root: &Path, prefix: &str, suffix: &str, directory: bool) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_prefix(prefix))
            .and_then(|n| n.strip_suffix(suffix))
        else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(id) else {
            continue;
        };
        if uuid.to_string() != id {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if directory && kind.is_dir() {
            fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
        } else if !directory && kind.is_file() {
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir(target).map_err(|e| e.to_string())?;
    let mut pending = vec![(source.to_owned(), target.to_owned(), 0)];
    let mut directories = vec![target.to_owned()];
    let (mut count, mut bytes) = (0usize, 0u64);
    while let Some((from, into, depth)) = pending.pop() {
        for entry in fs::read_dir(&from).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            count += 1;
            if count > MAX_FILES {
                return Err("Package contains too many files".into());
            }
            if kind.is_symlink() {
                return Err("Package folders cannot contain symbolic links".into());
            }
            let path = entry.path().canonicalize().map_err(|e| e.to_string())?;
            if !path.starts_with(source) {
                return Err("Package file escapes its folder".into());
            }
            let output = into.join(entry.file_name());
            if kind.is_dir() {
                if depth >= 16 {
                    return Err("Package contains too many nested folders".into());
                }
                fs::create_dir(&output).map_err(|e| e.to_string())?;
                directories.push(output.clone());
                pending.push((path, output, depth + 1));
            } else if kind.is_file() {
                let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
                if size > MAX_COPY_BYTES.saturating_sub(bytes) {
                    return Err("Package exceeds the 2 GiB installation limit".into());
                }
                let input = fs::File::open(&path).map_err(|e| e.to_string())?;
                let input_metadata = input.metadata().map_err(|e| e.to_string())?;
                if !input_metadata.is_file() {
                    return Err("Package contains a nonregular file".into());
                }
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)
                    .map_err(|e| e.to_string())?;
                bytes += std::io::copy(&mut input.take(MAX_COPY_BYTES - bytes + 1), &mut output)
                    .map_err(|e| e.to_string())?;
                if bytes > MAX_COPY_BYTES {
                    return Err("Package grew beyond the installation limit while copying".into());
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    // Retain execute bits for bundled programs/scripts without
                    // copying setuid, setgid or sticky bits from the source.
                    let mode = output
                        .metadata()
                        .map_err(|e| e.to_string())?
                        .permissions()
                        .mode();
                    let mode = (mode & 0o666) | (input_metadata.permissions().mode() & 0o111);
                    output
                        .set_permissions(fs::Permissions::from_mode(mode))
                        .map_err(|e| e.to_string())?;
                }
                output.sync_all().map_err(|e| e.to_string())?;
            } else {
                return Err("Package contains a nonregular file".into());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        sync_directory(&directory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("waifu-extension-store-{}", Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn package(&self, folder: &str, version: &str, greeting: &str) -> PathBuf {
            let root = self.0.join(folder);
            fs::create_dir_all(root.join("assets")).unwrap();
            // Tiny real files copied with the package; content-only room/preset
            // descriptors do not require a decoder or a renderer to validate.
            fs::write(root.join("assets/greeting.txt"), greeting).unwrap();
            fs::write(root.join("extension.json"), serde_json::to_vec(&json!({
                "api_version": 1, "id": "org.example.durable", "version": version,
                "name": "Durable fixture", "description": "Local store fixture",
                "provenance": {"author": "Fixture author", "license": "CC0-1.0"},
                "content": {
                    "backgrounds": [{"id": "room", "name": "Room", "source": {"type": "room", "room": "cozy_room"}}],
                    "presets": [{"id": "portrait", "name": "Portrait", "settings": {"portrait": true}}]
                }
            })).unwrap()).unwrap();
            root
        }

        fn store(&self) -> ExtensionStore {
            ExtensionStore::open(self.0.join("store")).unwrap()
        }

        fn integration(&self, folder: &str, version: &str, capabilities: &[&str]) -> PathBuf {
            let root = self.package(folder, version, "integration asset");
            // A real executable file for path/permission validation only. Tests
            // never run it or require an interpreter/provider/renderer.
            let program = root.join("observer.py");
            fs::write(&program, b"#!/usr/bin/env python3\npass\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let path = root.join("extension.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            manifest["process"] = json!({"program": "observer.py", "args": []});
            manifest["capabilities"] = json!(capabilities);
            fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            root
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn greeting(directory: &Path) -> String {
        fs::read_to_string(directory.join("assets/greeting.txt")).unwrap()
    }

    fn children(directory: &Path) -> Vec<PathBuf> {
        let mut result: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        result.sort();
        result
    }

    fn write_settings_schema(root: &Path, schema: serde_json::Value) {
        let path = root.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["settings"] = schema;
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }

    fn settings_schema() -> serde_json::Value {
        json!([
            {"id":"enabled", "label":"Enabled", "type":"toggle", "default":true},
            {"id":"count", "label":"Count", "type":"integer", "default":2, "min":0, "max":20},
            {"id":"label", "label":"Label", "type":"text", "default":"Hello", "max_length":32},
            {"id":"mode", "label":"Mode", "type":"choice", "default":"quiet", "options":["quiet","lively"]},
            {"id":"removed", "label":"Removed", "type":"toggle", "default":true},
            {"id":"kind", "label":"Kind", "type":"toggle", "default":false}
        ])
    }

    fn settings(value: serde_json::Value) -> Settings {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn background_credentials_reject_stale_install_and_private_revision() {
        let dir = TestDir::new();
        let source = dir.integration("auth-source", "1.0.0", &["catalog.get"]);
        let path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["credentials"] = json!([{"id":"token", "label":"Access token"}]);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        let captured = store.packs()[0].clone();
        let updates = || BTreeMap::from([("token".into(), Some("offline-auth-result".into()))]);
        store.set_enabled(id, true).unwrap();
        assert!(
            store
                .set_credentials_if_current(&captured, None, updates())
                .is_err()
        );
        let current = store.packs()[0].clone();
        let defs = store.resolved(&current).unwrap().manifest.credentials;
        let data = store.data_directory(&current).unwrap();
        assert!(
            crate::extension_credentials::status(&data, &defs)
                .unwrap()
                .configured
                .is_empty()
        );
        store
            .set_credentials_if_current(&current, None, updates())
            .unwrap();
        let saved = crate::extension_credentials::status(&data, &defs).unwrap();
        assert!(saved.revision.is_some());
        let replacement = BTreeMap::from([("token".into(), Some("must-not-overwrite".into()))]);
        assert!(
            store
                .set_credentials_if_current(&current, None, replacement)
                .is_err()
        );
        assert_eq!(
            crate::extension_credentials::read(&data, &defs).unwrap()["token"],
            "offline-auth-result"
        );
        assert_eq!(store.packs()[0], current);
        store.remove(id).unwrap();
        assert!(
            store
                .set_credentials_if_current(&current, saved.revision, updates())
                .is_err()
        );
    }

    #[test]
    fn private_credentials_persist_and_clear_without_changing_index_settings_or_grants() {
        use crate::extension_credentials::status;
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get"]);
        let path = source.join("extension.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["credentials"] = json!([{"id":"token", "label":"Access token"}]);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        let installed = store.packs()[0].clone();
        let defs = store.resolved(&installed).unwrap().manifest.credentials;
        let data = store.data_directory(&installed).unwrap();
        let index_path = dir.0.join("store/installed.json");
        let before = fs::read(&index_path).unwrap();
        assert_eq!(status(&data, &defs).unwrap().revision, None);
        let update = BTreeMap::from([("token".into(), Some("private-fixture-value".into()))]);
        store.set_credentials(id, update.clone()).unwrap();
        let configured = status(&data, &defs).unwrap();
        assert!(configured.revision.is_some());
        assert_eq!(
            configured.configured,
            std::collections::BTreeSet::from(["token".into()])
        );
        assert_eq!(store.packs()[0], installed);
        assert_eq!(fs::read(&index_path).unwrap(), before);
        store.set_credentials(id, update).unwrap();
        assert_eq!(status(&data, &defs).unwrap(), configured);
        for updates in [
            BTreeMap::from([("undeclared".into(), Some("private-invalid-value".into()))]),
            BTreeMap::from([("token".into(), Some("x".repeat(4097)))]),
        ] {
            let error = store.set_credentials(id, updates).unwrap_err();
            assert!(!error.contains("private-invalid-value"));
            assert_eq!(status(&data, &defs).unwrap(), configured);
        }
        drop(store);
        let mut store = dir.store();
        assert_eq!(status(&data, &defs).unwrap(), configured);
        store
            .set_credentials(id, BTreeMap::from([("token".into(), None)]))
            .unwrap();
        let cleared = status(&data, &defs).unwrap();
        assert!(cleared.configured.is_empty());
        assert_ne!(cleared.revision, configured.revision);
        assert_eq!(store.packs()[0], installed);
        assert_eq!(fs::read(&index_path).unwrap(), before);
        drop(store);
        let store = dir.store();
        assert_eq!(status(&data, &defs).unwrap(), cleared);
        let content = dir.package("content", "1.0.0", "content");
        drop(store);
        let mut content_store = ExtensionStore::open(dir.0.join("content-store")).unwrap();
        let content_id = content_store.install(&content).unwrap();
        assert!(
            content_store
                .set_credentials(content_id, BTreeMap::new())
                .is_err()
        );
        assert!(
            content_store
                .set_credentials(Uuid::new_v4(), BTreeMap::new())
                .is_err()
        );
        assert!(!dir.0.join("content-store/data").exists());
    }

    #[test]
    fn typed_settings_persist_restart_and_migrate_with_installed_update_schema() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get"]);
        write_settings_schema(&source, settings_schema());
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        assert_eq!(
            store.packs()[0].settings,
            settings(json!({
                "enabled":true, "count":2, "label":"Hello", "mode":"quiet", "removed":true, "kind":false
            }))
        );
        store.set_grants(id, vec![Capability::CatalogGet]).unwrap();
        store.set_enabled(id, true).unwrap();
        let chosen = settings(json!({
            "enabled":false, "count":12, "label":"Custom", "mode":"lively", "removed":false, "kind":true
        }));
        store.set_settings(id, chosen.clone()).unwrap();
        let first = store.packs()[0].clone();
        let old_directory = store.directory(&first);
        drop(store);
        let mut store = dir.store();
        assert_eq!(store.packs()[0], first);
        assert_eq!(store.packs()[0].settings, chosen);
        store.validate(&store.packs()[0]).unwrap();

        dir.integration("source", "2.0.0", &["catalog.get", "event.emit"]);
        write_settings_schema(
            &source,
            json!([
                {"id":"enabled", "label":"Enabled", "type":"toggle", "default":true},
                {"id":"count", "label":"Count", "type":"integer", "default":3, "min":0, "max":5},
                {"id":"label", "label":"Label", "type":"text", "default":"New default", "max_length":32},
                {"id":"mode", "label":"Mode", "type":"choice", "default":"calm", "options":["quiet","calm"]},
                {"id":"kind", "label":"Kind", "type":"text", "default":"converted", "max_length":16},
                {"id":"added", "label":"Added", "type":"integer", "default":4, "min":0, "max":4}
            ]),
        );
        assert_eq!(store.install_integration(&source).unwrap(), id);
        let updated = store.packs()[0].clone();
        assert_eq!(
            updated.settings,
            settings(json!({
                "enabled":false, "count":3, "label":"Custom", "mode":"calm", "kind":"converted", "added":4
            }))
        );
        assert_eq!(updated.grants, vec![Capability::CatalogGet]);
        assert!(updated.enabled);
        assert_ne!(updated.revision, first.revision);
        assert!(old_directory.join("extension.json").is_file());
        drop(store);
        fs::remove_dir_all(source).unwrap();
        let store = dir.store();
        assert_eq!(store.packs(), std::slice::from_ref(&updated));
        let resolved = store.resolved(&updated).unwrap();
        assert_eq!(
            validate_values(&resolved.manifest.settings, &updated.settings).unwrap(),
            updated.settings
        );
    }

    #[test]
    fn settings_replacement_fills_defaults_and_rejects_invalid_values_atomically() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get"]);
        write_settings_schema(&source, settings_schema());
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        store.set_grants(id, vec![Capability::CatalogGet]).unwrap();
        store
            .set_settings(id, settings(json!({"count":5})))
            .unwrap();
        let previous = store.packs()[0].clone();
        assert_eq!(
            previous.settings,
            settings(json!({
                "enabled":true, "count":5, "label":"Hello", "mode":"quiet", "removed":true, "kind":false
            }))
        );
        assert_eq!(previous.grants, vec![Capability::CatalogGet]);
        assert!(!previous.enabled);
        let index = fs::read(store.root.join("installed.json")).unwrap();
        for invalid in [
            json!({"unknown":true}),
            json!({"enabled":"true"}),
            json!({"count":21}),
            json!({"mode":"missing"}),
            json!({"label":"x".repeat(33)}),
            json!({"label":"x".repeat(MAX_SETTINGS_BYTES)}),
        ] {
            assert!(store.set_settings(id, settings(invalid)).is_err());
            assert_eq!(store.packs(), std::slice::from_ref(&previous));
            assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
        }
        assert!(store.set_settings(Uuid::new_v4(), Settings::new()).is_err());
        store.remove(id).unwrap();
        let content = dir.package("content", "1.0.0", "content");
        let id = store.install(&content).unwrap();
        let index = fs::read(store.root.join("installed.json")).unwrap();
        assert!(
            store
                .set_settings(id, Settings::new())
                .unwrap_err()
                .contains("Content-only")
        );
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
    }

    #[test]
    fn settings_update_and_setter_follow_index_commit_rollback_semantics() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get"]);
        write_settings_schema(&source, settings_schema());
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        store.set_grants(id, vec![Capability::CatalogGet]).unwrap();
        store
            .set_settings(id, settings(json!({"count":12})))
            .unwrap();
        let previous = store.packs()[0].clone();
        let revisions = children(store.directory(&previous).parent().unwrap());
        let index = store.root.join("installed.json");
        let backup = store.root.join("saved-index.json");
        let bytes = fs::read(&index).unwrap();
        fs::rename(&index, &backup).unwrap();
        fs::create_dir(&index).unwrap();
        assert!(
            store
                .set_settings(id, settings(json!({"count":3})))
                .is_err()
        );
        assert_eq!(store.packs(), std::slice::from_ref(&previous));
        dir.integration("source", "2.0.0", &["catalog.get"]);
        write_settings_schema(
            &source,
            json!([
                {"id":"count", "label":"Count", "type":"integer", "default":2, "min":0, "max":4}
            ]),
        );
        assert!(store.install_integration(&source).is_err());
        assert_eq!(store.packs(), std::slice::from_ref(&previous));
        assert_eq!(
            children(store.directory(&previous).parent().unwrap()),
            revisions
        );
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        fs::remove_dir(&index).unwrap();
        fs::rename(&backup, &index).unwrap();

        // Only the source changed: use the installed schema's still-valid range.
        let error = store
            .set_settings_with_sync(id, settings(json!({"count":15})), |_| {
                Err("injected directory sync failure".into())
            })
            .unwrap_err();
        assert!(error.contains("new state retained"));
        let expected = store.packs()[0].clone();
        assert_eq!(
            expected.settings["count"],
            crate::extension_settings::SettingValue::Integer(15)
        );
        assert_eq!(expected.grants, previous.grants);
        drop(store);
        let store = dir.store();
        assert_eq!(store.packs(), std::slice::from_ref(&expected));
        store.validate(&expected).unwrap();
    }

    #[test]
    fn oversized_saved_settings_are_rejected_without_replacing_index() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &[]);
        write_settings_schema(&source, settings_schema());
        let mut store = dir.store();
        store.install_integration(&source).unwrap();
        let index = store.root.join("installed.json");
        let root = store.root.clone();
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        // The complete index remains under 1 MiB. Settings have their own limit,
        // measured as serialized bytes rather than unescaped string lengths.
        saved["packs"][0]["settings"] = json!({"label":"\"".repeat(MAX_SETTINGS_BYTES / 2)});
        let bytes = serde_json::to_vec(&saved).unwrap();
        assert!(bytes.len() < MAX_INDEX_BYTES as usize);
        drop(store);
        fs::write(&index, &bytes).unwrap();
        let error = ExtensionStore::open(root).err().unwrap();
        assert!(error.contains("16 KiB"), "{error}");
        assert_eq!(fs::read(index).unwrap(), bytes);
    }

    #[test]
    fn resolved_rejects_saved_settings_incompatible_with_installed_schema() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &[]);
        write_settings_schema(&source, settings_schema());
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        let index = store.root.join("installed.json");
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        saved["packs"][0]["settings"]["count"] = json!(999);
        let bytes = serde_json::to_vec(&saved).unwrap();
        drop(store);
        fs::write(&index, &bytes).unwrap();
        let mut store = dir.store();
        assert!(store.resolved(&store.packs()[0]).is_err());
        assert!(store.set_enabled(id, true).is_err());
        assert!(!store.packs()[0].enabled);
        assert_eq!(fs::read(index).unwrap(), bytes);
    }

    #[test]
    fn executable_install_is_explicit_and_updates_cannot_change_trust_type() {
        let dir = TestDir::new();
        let executable = dir.integration("integration", "1.0.0", &["catalog.get"]);
        let content = dir.package("content", "2.0.0", "content");
        let mut store = dir.store();
        assert!(store.install(&executable).is_err());
        assert!(store.packs().is_empty());
        assert!(children(&store.root.join("packages")).is_empty());

        let id = store.install_integration(&executable).unwrap();
        let pack = store.packs()[0].clone();
        assert!(pack.executable);
        assert!(!pack.enabled);
        assert!(pack.grants.is_empty());
        assert!(store.resolved(&pack).unwrap().manifest.process.is_some());
        let index = fs::read(store.root.join("installed.json")).unwrap();
        let revisions = children(store.directory(&pack).parent().unwrap());
        assert!(store.install(&content).unwrap_err().contains("trust type"));
        assert_eq!(store.packs(), std::slice::from_ref(&pack));
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
        assert_eq!(
            children(store.directory(&pack).parent().unwrap()),
            revisions
        );

        store.remove(id).unwrap();
        store.install(&content).unwrap();
        let pack = store.packs()[0].clone();
        let index = fs::read(store.root.join("installed.json")).unwrap();
        assert!(
            store
                .install_integration(&executable)
                .unwrap_err()
                .contains("trust type")
        );
        assert_eq!(store.packs(), std::slice::from_ref(&pack));
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
        assert!(store.install_integration(&content).is_err());
        assert!(store.data_directory(&pack).is_err());
    }

    #[test]
    fn executable_updates_intersect_grants_and_restart_restores_explicit_state() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["motion.play", "catalog.get"]);
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        store
            .set_grants(id, vec![Capability::MotionPlay, Capability::CatalogGet])
            .unwrap();
        store.set_enabled(id, true).unwrap();
        let first = store.packs()[0].clone();
        let old_directory = store.directory(&first);

        dir.integration("source", "2.0.0", &["catalog.get", "event.emit"]);
        assert_eq!(store.install_integration(&source).unwrap(), id);
        assert_eq!(store.packs()[0].grants, vec![Capability::CatalogGet]);
        assert!(store.packs()[0].enabled);
        assert_ne!(store.packs()[0].revision, first.revision);
        assert!(old_directory.join("observer.py").is_file());

        // Removing a grant is durable: reintroducing the request cannot restore
        // the user's old permission, and a new request stays ungranted.
        dir.integration(
            "source",
            "3.0.0",
            &["motion.play", "catalog.get", "event.emit"],
        );
        store.install_integration(&source).unwrap();
        assert_eq!(store.packs()[0].grants, vec![Capability::CatalogGet]);
        store.set_enabled(id, false).unwrap();
        store.install_integration(&source).unwrap();
        assert!(!store.packs()[0].enabled);
        let expected = store.packs()[0].clone();
        drop(store);
        fs::remove_dir_all(source).unwrap();
        let store = dir.store();
        assert_eq!(store.packs(), std::slice::from_ref(&expected));
        store.validate(&expected).unwrap();
        assert!(old_directory.join("observer.py").is_file());
    }

    #[test]
    fn unrequested_duplicate_and_content_grants_leave_index_unchanged() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get"]);
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        let pack = store.packs()[0].clone();
        let index = fs::read(store.root.join("installed.json")).unwrap();
        for grants in [
            vec![Capability::ReplyRead],
            vec![Capability::CatalogGet, Capability::CatalogGet],
        ] {
            assert!(store.set_grants(id, grants).is_err());
            assert_eq!(store.packs(), std::slice::from_ref(&pack));
            assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
        }
        store.remove(id).unwrap();
        let content = dir.package("content", "1.0.0", "kept");
        let id = store.install(&content).unwrap();
        let index = fs::read(store.root.join("installed.json")).unwrap();
        assert!(store.set_grants(id, vec![Capability::CatalogGet]).is_err());
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
    }

    #[test]
    fn grants_and_executable_updates_roll_back_before_commit_but_keep_committed_grants() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get", "event.emit"]);
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        store.set_grants(id, vec![Capability::CatalogGet]).unwrap();
        let previous = store.packs()[0].clone();
        let revisions = children(store.directory(&previous).parent().unwrap());
        let index = store.root.join("installed.json");
        let backup = store.root.join("saved-index.json");
        let bytes = fs::read(&index).unwrap();
        fs::rename(&index, &backup).unwrap();
        fs::create_dir(&index).unwrap();
        assert!(store.set_grants(id, vec![Capability::EventEmit]).is_err());
        assert_eq!(store.packs(), std::slice::from_ref(&previous));
        dir.integration("source", "2.0.0", &["event.emit"]);
        assert!(store.install_integration(&source).is_err());
        assert_eq!(store.packs(), std::slice::from_ref(&previous));
        assert_eq!(
            children(store.directory(&previous).parent().unwrap()),
            revisions
        );
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        fs::remove_dir(&index).unwrap();
        fs::rename(&backup, &index).unwrap();

        let error = store
            .set_grants_with_sync(id, vec![Capability::EventEmit], |_| {
                Err("injected directory sync failure".into())
            })
            .unwrap_err();
        assert!(error.contains("new state retained"));
        let expected = store.packs()[0].clone();
        assert_eq!(expected.grants, vec![Capability::EventEmit]);
        drop(store);
        let store = dir.store();
        assert_eq!(store.packs(), std::slice::from_ref(&expected));
        store.validate(&expected).unwrap();
    }

    #[test]
    fn legacy_indexes_default_to_content_and_invalid_grants_are_preserved_on_error() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "legacy");
        let mut store = dir.store();
        store.install(&source).unwrap();
        let path = store.root.join("installed.json");
        let root = store.root.clone();
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let pack = legacy["packs"][0].as_object_mut().unwrap();
        pack.remove("executable");
        pack.remove("grants");
        pack.remove("settings");
        drop(store);
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let store = dir.store();
        assert!(!store.packs()[0].executable);
        assert!(store.packs()[0].grants.is_empty());
        assert!(store.packs()[0].settings.is_empty());
        store.validate(&store.packs()[0]).unwrap();
        drop(store);
        for grants in [json!(["catalog.get"]), json!(["unknown.capability"])] {
            let mut invalid = legacy.clone();
            invalid["packs"][0]["grants"] = grants;
            let bytes = serde_json::to_vec(&invalid).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert!(ExtensionStore::open(root.clone()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        let mut invalid = legacy;
        invalid["packs"][0]["executable"] = json!(true);
        invalid["packs"][0]["grants"] = json!(["catalog.get", "catalog.get"]);
        let bytes = serde_json::to_vec(&invalid).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(ExtensionStore::open(root).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn resolved_enforces_persisted_policy_identity_and_requested_grants() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &["catalog.get"]);
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        store.set_grants(id, vec![Capability::CatalogGet]).unwrap();
        let pack = store.packs()[0].clone();
        let mut forged = pack.clone();
        forged.executable = false;
        assert!(store.resolved(&forged).is_err());
        assert!(store.data_directory(&forged).is_err());
        let path = store.directory(&pack).join("extension.json");
        let original = fs::read(&path).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
        manifest["capabilities"] = json!([]);
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            store
                .resolved(&pack)
                .unwrap_err()
                .contains("no longer requested")
        );
        assert!(store.set_enabled(id, true).is_err());
        assert!(!store.packs()[0].enabled);
        manifest.as_object_mut().unwrap().remove("process");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            store
                .resolved(&pack)
                .unwrap_err()
                .contains("requires a persistent process")
        );
        fs::write(&path, original).unwrap();
        store.resolved(&pack).unwrap();
    }

    #[test]
    fn writable_data_survives_updates_removal_and_reopen_without_accepting_stale_descriptors() {
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &[]);
        let mut store = dir.store();
        let id = store.install_integration(&source).unwrap();
        let first = store.packs()[0].clone();
        let data = store.data_directory(&first).unwrap();
        fs::write(data.join("state.txt"), b"retained config").unwrap();
        dir.integration("source", "2.0.0", &[]);
        store.install_integration(&source).unwrap();
        assert!(store.data_directory(&first).is_err());
        assert!(store.resolved(&first).is_err());
        let current = store.packs()[0].clone();
        assert_eq!(store.data_directory(&current).unwrap(), data);
        assert!(!data.starts_with(store.directory(&current)));
        drop(store);
        let mut store = dir.store();
        assert_eq!(store.data_directory(&current).unwrap(), data);
        store.remove(id).unwrap();
        assert!(store.data_directory(&current).is_err());
        assert_eq!(
            fs::read(data.join("state.txt")).unwrap(),
            b"retained config"
        );
        drop(store);
        assert!(dir.store().packs().is_empty());
        assert_eq!(
            fs::read(data.join("state.txt")).unwrap(),
            b"retained config"
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_copy_retains_execute_permissions_without_special_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &[]);
        fs::set_permissions(
            source.join("observer.py"),
            fs::Permissions::from_mode(0o6755),
        )
        .unwrap();
        let mut store = dir.store();
        store.install_integration(&source).unwrap();
        let pack = &store.packs()[0];
        let copied = store.directory(pack).join("observer.py");
        let mode = fs::metadata(&copied).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        assert_eq!(mode & 0o7000, 0);
        assert_eq!(
            fs::read(copied).unwrap(),
            fs::read(source.join("observer.py")).unwrap()
        );
        store.resolved(pack).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writable_data_rejects_symlink_parents_leaves_and_regular_files() {
        use std::os::unix::fs::symlink;
        let dir = TestDir::new();
        let source = dir.integration("source", "1.0.0", &[]);
        let mut store = dir.store();
        store.install_integration(&source).unwrap();
        let pack = store.packs()[0].clone();
        let data = store.root.join("data");
        let outside = dir.0.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &data).unwrap();
        assert!(
            store
                .data_directory(&pack)
                .unwrap_err()
                .contains("real directory")
        );
        assert!(children(&outside).is_empty());
        fs::remove_file(&data).unwrap();
        fs::create_dir(&data).unwrap();
        let leaf = data.join(pack.installation_id.to_string());
        symlink(&outside, &leaf).unwrap();
        assert!(store.data_directory(&pack).is_err());
        fs::remove_file(&leaf).unwrap();
        fs::write(&leaf, b"not a directory").unwrap();
        assert!(store.data_directory(&pack).is_err());
        assert_eq!(fs::read(&leaf).unwrap(), b"not a directory");
        assert!(children(&outside).is_empty());
    }

    #[test]
    fn exclusive_lock_rejects_second_store_and_releases_on_drop() {
        let dir = TestDir::new();
        let store = dir.store();
        let root = store.root.clone();
        assert!(
            ExtensionStore::open(root.clone())
                .err()
                .unwrap()
                .contains("busy")
        );
        drop(store);
        assert!(ExtensionStore::open(root).is_ok());
    }

    #[test]
    fn store_drop_releases_lock_even_when_open_description_has_another_descriptor() {
        let dir = TestDir::new();
        let store = dir.store();
        let root = store.root.clone();
        // dup and fork share the same OS open-file description. CLOEXEC only
        // closes the inherited copy after exec, leaving a real fork/exec window.
        let duplicate = store._lock.0.try_clone().unwrap();
        assert!(ExtensionStore::open(root.clone()).is_err());
        drop(store);
        let reopened = ExtensionStore::open(root.clone())
            .expect("owner drop left a duplicate holding the lock");
        drop(duplicate);
        assert!(
            ExtensionStore::open(root.clone()).is_err(),
            "closing an old descriptor released the new owner's lock"
        );
        drop(reopened);
        assert!(ExtensionStore::open(root).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn store_drop_releases_lock_while_forked_child_has_not_execed() {
        struct Child(libc::pid_t);
        impl Drop for Child {
            fn drop(&mut self) {
                // SAFETY: this test owns this unreaped child. Kill before wait;
                // the child never executes Rust destructors or allocator code.
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                    while libc::waitpid(self.0, std::ptr::null_mut(), 0) < 0 {
                        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                            break;
                        }
                    }
                }
            }
        }
        let dir = TestDir::new();
        let store = dir.store();
        let root = store.root.clone();
        // SAFETY: child performs only the async-signal-safe pause syscall until
        // the parent kills/reaps it. It never returns to the multithreaded runtime.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "cannot create controlled fork fixture");
        if pid == 0 {
            loop {
                unsafe {
                    libc::pause();
                }
            }
        }
        let child = Child(pid);
        drop(store);
        let reopened = ExtensionStore::open(root);
        drop(child);
        assert!(
            reopened.is_ok(),
            "pre-exec child retained the dropped owner's lock"
        );
    }

    #[test]
    fn startup_reaps_only_recognized_debris_and_preserves_revisions() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "retained");
        let mut store = dir.store();
        store.install(&source).unwrap();
        let revision = store.directory(&store.packs()[0]);
        store.remove(store.packs()[0].installation_id).unwrap();
        let root = store.root.clone();
        let stage = root
            .join("packages")
            .join(format!(".stage-{}", Uuid::new_v4()));
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("partial"), b"partial").unwrap();
        let temp = root.join(format!(".installed-{}.tmp", Uuid::new_v4()));
        fs::write(&temp, b"partial").unwrap();
        let unknown = root.join("packages/.stage-user-files");
        fs::create_dir(&unknown).unwrap();
        let unknown_temp = root.join(".installed-user.tmp");
        fs::write(&unknown_temp, b"keep").unwrap();
        let wrong_kind = root
            .join("packages")
            .join(format!(".stage-{}", Uuid::new_v4()));
        fs::write(&wrong_kind, b"keep").unwrap();
        #[cfg(unix)]
        let link = {
            let link = root
                .join("packages")
                .join(format!(".stage-{}", Uuid::new_v4()));
            std::os::unix::fs::symlink(&revision, &link).unwrap();
            // Even links inside recognized debris must not affect their targets.
            std::os::unix::fs::symlink(&revision, stage.join("link")).unwrap();
            let temp_link = root.join(format!(".installed-{}.tmp", Uuid::new_v4()));
            std::os::unix::fs::symlink(&unknown_temp, &temp_link).unwrap();
            temp_link
        };
        drop(store);
        let _store = dir.store();
        assert!(!stage.exists());
        assert!(!temp.exists());
        assert!(unknown.is_dir());
        assert!(unknown_temp.is_file());
        assert!(wrong_kind.is_file());
        assert_eq!(greeting(&revision), "retained");
        #[cfg(unix)]
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    }

    #[test]
    fn failed_new_install_removes_empty_installation_parent() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "rollback");
        let mut store = dir.store();
        fs::create_dir(store.root.join("installed.json")).unwrap();
        assert!(store.install(&source).is_err());
        assert!(store.packs().is_empty());
        assert!(children(&store.root.join("packages")).is_empty());
    }

    #[test]
    fn install_update_restart_keep_identity_disabled_state_and_old_files() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "original");
        let mut store = dir.store();
        let id = store.install(&source).unwrap();
        assert_eq!(store.packs().len(), 1);
        assert!(store.packs()[0].enabled);
        let first = store.packs()[0].clone();
        let first_directory = store.directory(&first);
        assert_eq!(greeting(&first_directory), "original");
        store.set_enabled(id, false).unwrap();
        // An update is copied from the same external folder, without editing
        // the immutable revision still referenced by a user setting.
        dir.package("source", "2.0.0", "updated");
        assert_eq!(store.install(&source).unwrap(), id);
        assert_eq!(store.packs().len(), 1);
        let second = store.packs()[0].clone();
        assert_eq!(second.installation_id, first.installation_id);
        assert_ne!(second.revision, first.revision);
        assert_eq!(second.version, "2.0.0");
        assert!(!second.enabled);
        let second_directory = store.directory(&second);
        assert_eq!(greeting(&first_directory), "original");
        assert_eq!(greeting(&second_directory), "updated");
        drop(store);
        // Installed files must survive losing the original source directory.
        fs::remove_dir_all(source).unwrap();
        let restarted = dir.store();
        assert_eq!(restarted.packs(), std::slice::from_ref(&second));
        restarted.validate(&second).unwrap();
        assert_eq!(greeting(&first_directory), "original");
        assert_eq!(greeting(&second_directory), "updated");
    }

    #[test]
    fn removal_persists_without_deleting_current_or_old_revisions() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "first");
        let mut store = dir.store();
        let id = store.install(&source).unwrap();
        let first = store.directory(&store.packs()[0]);
        dir.package("source", "1.1.0", "second");
        store.install(&source).unwrap();
        let second = store.directory(&store.packs()[0]);
        store.remove(id).unwrap();
        assert!(store.packs().is_empty());
        assert_eq!(greeting(&first), "first");
        assert_eq!(greeting(&second), "second");
        assert_eq!(greeting(&source), "second");
        drop(store);
        assert!(dir.store().packs().is_empty());
        assert_eq!(greeting(&second), "second");
    }

    #[test]
    fn malformed_indexes_are_reported_and_preserved_byte_for_byte() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "untouched");
        let mut store = dir.store();
        store.install(&source).unwrap();
        let root = store.root.clone();
        let revision = store.directory(&store.packs()[0]);
        let original = fs::read(root.join("installed.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let mut wrong_version = parsed.clone();
        wrong_version["version"] = json!(2);
        let mut duplicate = parsed.clone();
        duplicate["packs"]
            .as_array_mut()
            .unwrap()
            .push(parsed["packs"][0].clone());
        let mut unknown = parsed.clone();
        unknown["packs"][0]["ignored"] = json!(true);
        let mut empty_version = parsed;
        empty_version["packs"][0]["version"] = json!(" ");
        drop(store);
        for bytes in [
            b"{broken index".to_vec(),
            serde_json::to_vec(&wrong_version).unwrap(),
            serde_json::to_vec(&duplicate).unwrap(),
            serde_json::to_vec(&unknown).unwrap(),
            serde_json::to_vec(&empty_version).unwrap(),
        ] {
            fs::write(root.join("installed.json"), &bytes).unwrap();
            let error = ExtensionStore::open(root.clone()).err().unwrap();
            assert!(error.contains("original file retained"), "{error}");
            assert_eq!(fs::read(root.join("installed.json")).unwrap(), bytes);
            assert_eq!(greeting(&revision), "untouched");
        }
        fs::write(root.join("installed.json"), original).unwrap();
        assert_eq!(dir.store().packs().len(), 1);
    }

    #[test]
    fn failed_index_write_rolls_back_update_enable_and_removal() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "kept");
        let mut store = dir.store();
        let id = store.install(&source).unwrap();
        store.set_enabled(id, false).unwrap();
        let previous = store.packs().to_vec();
        let revision = store.directory(&previous[0]);
        let revision_parent = revision.parent().unwrap();
        let files_before = children(revision_parent);
        let index = store.root.join("installed.json");
        let saved_index = store.root.join("saved-index.json");
        let bytes = fs::read(&index).unwrap();
        fs::rename(&index, &saved_index).unwrap();
        // A directory at the index destination forces a real persistence error
        // on every OS, without unreliable permission tricks under root.
        fs::create_dir(&index).unwrap();
        dir.package("source", "2.0.0", "discarded");
        assert!(store.install(&source).is_err());
        assert_eq!(store.packs(), previous);
        assert_eq!(children(revision_parent), files_before);
        assert_eq!(greeting(&revision), "kept");
        assert!(store.set_enabled(id, true).is_err());
        assert_eq!(store.packs(), previous);
        assert!(store.remove(id).is_err());
        assert_eq!(store.packs(), previous);
        assert_eq!(fs::read(&saved_index).unwrap(), bytes);
        assert!(children(&store.root.join("packages")).iter().all(|path| {
            !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
        assert!(
            children(&store.root)
                .iter()
                .all(|path| !path.extension().is_some_and(|extension| extension == "tmp"))
        );
        fs::remove_dir(&index).unwrap();
        fs::rename(&saved_index, &index).unwrap();
        drop(store);
        assert_eq!(dir.store().packs(), previous);
    }

    #[test]
    fn post_rename_sync_error_keeps_installed_revision_and_committed_index() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "kept after commit");
        let mut store = dir.store();
        store.install(&source).unwrap();
        let previous = store.packs()[0].clone();
        let old_revision = store.directory(&previous);
        dir.package("source", "2.0.0", "new committed revision");
        // Exercise the real temp write, fsync and rename; fail only the final
        // directory sync, the boundary where rolling back used to be unsafe.
        let error = store
            .install_with_sync(&source, |_| Err("injected directory sync failure".into()))
            .unwrap_err();
        assert!(error.contains("new state retained"));
        assert_eq!(store.packs()[0].installation_id, previous.installation_id);
        assert_ne!(store.packs()[0].revision, previous.revision);
        assert_eq!(store.packs()[0].version, "2.0.0");
        let new_revision = store.directory(&store.packs()[0]);
        let expected = store.packs().to_vec();
        drop(store);
        let reopened = dir.store();
        assert_eq!(reopened.packs(), expected);
        reopened.validate(&reopened.packs()[0]).unwrap();
        assert_eq!(greeting(&old_revision), "kept after commit");
        assert_eq!(greeting(&new_revision), "new committed revision");
    }

    #[test]
    fn invalid_update_and_modified_installed_manifest_leave_state_unchanged() {
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "kept");
        let mut store = dir.store();
        let id = store.install(&source).unwrap();
        store.set_enabled(id, false).unwrap();
        let previous = store.packs().to_vec();
        let index = fs::read(store.root.join("installed.json")).unwrap();
        fs::write(source.join("extension.json"), b"{broken").unwrap();
        assert!(store.install(&source).is_err());
        assert_eq!(store.packs(), previous);
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
        let installed = store.directory(&previous[0]);
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(installed.join("extension.json")).unwrap()).unwrap();
        metadata["version"] = json!("9.0.0");
        fs::write(
            installed.join("extension.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        assert!(
            store
                .set_enabled(id, true)
                .unwrap_err()
                .contains("identity differs")
        );
        assert_eq!(store.packs(), previous);
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), index);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_source_symlinks_and_cleans_partial_copies_without_changing_index() {
        use std::os::unix::fs::symlink;
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "kept");
        let mut store = dir.store();
        store.install(&source).unwrap();
        let previous = store.packs().to_vec();
        let packages = children(&store.root.join("packages"));
        let bytes = fs::read(store.root.join("installed.json")).unwrap();
        // Even an unreferenced, contained symlink must not survive copying.
        symlink(
            source.join("assets/greeting.txt"),
            source.join("extra-link.txt"),
        )
        .unwrap();
        assert!(
            store
                .install(&source)
                .unwrap_err()
                .contains("symbolic links")
        );
        assert_eq!(store.packs(), previous);
        assert_eq!(children(&store.root.join("packages")), packages);
        assert_eq!(fs::read(store.root.join("installed.json")).unwrap(), bytes);
        let mut empty_store = ExtensionStore::open(dir.0.join("empty-store")).unwrap();
        assert!(empty_store.install(&source).is_err());
        assert!(empty_store.packs().is_empty());
        assert!(children(&empty_store.root.join("packages")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_dangling_index_and_redirected_store_directories() {
        use std::os::unix::fs::symlink;
        let dir = TestDir::new();
        let source = dir.package("source", "1.0.0", "outside");
        let mut store = dir.store();
        let index = store.root.join("installed.json");
        symlink(dir.0.join("does-not-exist"), &index).unwrap();
        assert!(ExtensionStore::open(store.root.clone()).is_err());
        assert!(
            fs::symlink_metadata(&index)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(store.install(&source).is_err());
        assert!(store.packs().is_empty());
        fs::remove_file(index).unwrap();
        let packages = store.root.join("packages");
        fs::remove_dir_all(&packages).unwrap();
        let outside = dir.0.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &packages).unwrap();
        assert!(ExtensionStore::open(store.root.clone()).is_err());
        assert!(
            store
                .install(&source)
                .unwrap_err()
                .contains("real directory")
        );
        assert!(children(&outside).is_empty());
    }
}
