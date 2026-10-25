//! Private per-install credentials. These values must never enter plain settings,
//! installation indexes, handshakes, logs, or UI status. `read` is for workers.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use uuid::Uuid;

const FILE_LIMIT: usize = 32 * 1024;
const VALUE_LIMIT: usize = 4 * 1024;
const TOTAL_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialDefinition {
    pub id: String,
    pub label: String,
}

/// Contains identifiers only, never credential values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CredentialStatus {
    pub revision: Option<Uuid>,
    pub configured: BTreeSet<String>,
}

/// Private, validated OAuth identity and expiry; never protocol or UI data.
/// Intentionally no Debug. Expired timestamps remain valid for refresh on restart.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthMetadata {
    pub client_id: String,
    pub user_id: String,
    pub scopes: BTreeSet<String>,
    pub expires_at: u64,
}

// Intentionally no Debug: even internal diagnostics must not format values.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCredentials {
    revision: Option<Uuid>,
    values: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth_metadata: Option<OAuthMetadata>,
}

fn validate_oauth(metadata: &OAuthMetadata) -> Result<(), String> {
    let identity = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && !value
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
    };
    if !identity(&metadata.client_id)
        || !identity(&metadata.user_id)
        || !(1..=253_402_300_799).contains(&metadata.expires_at)
        || metadata.scopes.len() > 32
        || metadata.scopes.iter().any(|scope| {
            scope.is_empty()
                || scope.len() > 128
                // OAuth scope-token grammar: printable ASCII except quote/backslash.
                || !scope.bytes().all(|byte| {
                    matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e)
                })
        })
    {
        return Err("Invalid OAuth metadata".into());
    }
    Ok(())
}

fn oauth_values_valid(
    metadata: &OAuthMetadata,
    values: &BTreeMap<String, String>,
) -> Result<(), String> {
    validate_oauth(metadata)?;
    if values.get("client_id") != Some(&metadata.client_id)
        || ["access_token", "refresh_token"]
            .iter()
            .any(|id| values.get(*id).is_none_or(String::is_empty))
    {
        return Err("OAuth credentials do not match metadata".into());
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn definitions(defs: &[CredentialDefinition]) -> Result<BTreeSet<&str>, String> {
    if defs.len() > 16 {
        return Err("Too many credential definitions".into());
    }
    let mut ids = BTreeSet::new();
    for def in defs {
        if !valid_id(&def.id) || !ids.insert(def.id.as_str()) {
            return Err("Invalid or duplicate credential definition ID".into());
        }
        if def.label.trim().is_empty()
            || def.label.len() > 128
            || def.label.chars().any(char::is_control)
        {
            return Err("Invalid credential definition label".into());
        }
    }
    Ok(ids)
}

pub fn validate_definitions(defs: &[CredentialDefinition]) -> Result<(), String> {
    definitions(defs).map(|_| ())
}

fn values_valid(values: &BTreeMap<String, String>) -> Result<(), String> {
    if values.len() > 16 {
        return Err("Too many stored credentials".into());
    }
    let mut total = 0usize;
    for (id, value) in values {
        if !valid_id(id) {
            return Err("Invalid stored credential ID".into());
        }
        if value.len() > VALUE_LIMIT || value.contains('\0') {
            return Err("Invalid credential value or value exceeds size limit".into());
        }
        total = total.saturating_add(value.len());
        if total > TOTAL_LIMIT {
            return Err("Credentials exceed total size limit".into());
        }
    }
    Ok(())
}

fn load(data_dir: &Path) -> Result<SavedCredentials, String> {
    let path = data_dir.join("credentials.json");
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err("Credential file must be a regular file".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SavedCredentials::default());
        }
        Err(_) => return Err("Cannot inspect credential file".into()),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|_| "Cannot open credential file")?;
    let meta = file
        .metadata()
        .map_err(|_| "Cannot inspect opened credential file")?;
    if !meta.is_file() {
        return Err("Credential file must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid takes no pointers and has no preconditions.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o7777 != 0o600 {
            return Err("Credential file must be owned by this user with mode 0600".into());
        }
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read credential file")?;
    if bytes.len() > FILE_LIMIT {
        return Err("Credential file exceeds size limit".into());
    }
    // Never forward serde errors: they can contain the input credential text.
    let saved: SavedCredentials =
        serde_json::from_slice(&bytes).map_err(|_| "Malformed credential file")?;
    if saved.revision.is_none_or(|revision| revision.is_nil()) {
        return Err("Invalid credential revision".into());
    }
    values_valid(&saved.values)?;
    if let Some(metadata) = &saved.oauth_metadata {
        oauth_values_valid(metadata, &saved.values)?;
    }
    Ok(saved)
}

fn summary(saved: &SavedCredentials) -> CredentialStatus {
    CredentialStatus {
        revision: saved.revision,
        configured: saved.values.keys().cloned().collect(),
    }
}

pub fn status(data_dir: &Path, defs: &[CredentialDefinition]) -> Result<CredentialStatus, String> {
    let ids = definitions(defs)?;
    let mut saved = load(data_dir)?;
    saved.values.retain(|id, _| ids.contains(id.as_str()));
    Ok(summary(&saved))
}

/// Worker-only access. Callers must not log or serialize the returned values into
/// settings, indexes, or protocol messages.
pub fn read(
    data_dir: &Path,
    defs: &[CredentialDefinition],
) -> Result<BTreeMap<String, String>, String> {
    let ids = definitions(defs)?;
    let mut saved = load(data_dir)?;
    saved.values.retain(|id, _| ids.contains(id.as_str()));
    Ok(saved.values)
}

/// Worker-only metadata access, filtered by current declarations just like read.
/// Missing OAuth declarations hide metadata without rewriting the private file.
pub fn read_oauth_metadata(
    data_dir: &Path,
    defs: &[CredentialDefinition],
) -> Result<Option<OAuthMetadata>, String> {
    let ids = definitions(defs)?;
    let saved = load(data_dir)?;
    if !["client_id", "access_token", "refresh_token"]
        .iter()
        .all(|id| ids.contains(id))
    {
        return Ok(None);
    }
    Ok(saved.oauth_metadata)
}

/// Apply a partial update; None clears a value. Explicit saves also prune
/// credentials no longer declared by the manifest. Reads/status never migrate
/// the file. Value changes clear OAuth metadata; no-ops retain it and its revision.
/// A failure after rename may still have committed: status remains readable and
/// callers must refresh it before reporting configuration state or retrying.
pub fn update(
    data_dir: &Path,
    defs: &[CredentialDefinition],
    updates: &BTreeMap<String, Option<String>>,
) -> Result<CredentialStatus, String> {
    let (mut saved, values) = prepare_update(data_dir, defs, updates)?;
    if saved.values == values {
        return Ok(summary(&saved));
    }
    saved.values = values;
    saved.oauth_metadata = None;
    saved.revision = Some(Uuid::new_v4());
    persist(data_dir, &saved, sync_parent)?;
    Ok(summary(&saved))
}

/// Atomically save OAuth values and validated metadata under one revision.
/// No-op saves retain the revision; identity, scopes or expiry changes replace it.
/// As with update, an error after rename does not imply that nothing was saved.
pub fn update_oauth(
    data_dir: &Path,
    defs: &[CredentialDefinition],
    updates: &BTreeMap<String, Option<String>>,
    metadata: &OAuthMetadata,
) -> Result<CredentialStatus, String> {
    validate_oauth(metadata)?;
    let (mut saved, values) = prepare_update(data_dir, defs, updates)?;
    oauth_values_valid(metadata, &values)?;
    if saved.values == values && saved.oauth_metadata.as_ref() == Some(metadata) {
        return Ok(summary(&saved));
    }
    saved.values = values;
    saved.oauth_metadata = Some(metadata.clone());
    saved.revision = Some(Uuid::new_v4());
    persist(data_dir, &saved, sync_parent)?;
    Ok(summary(&saved))
}

fn prepare_update(
    data_dir: &Path,
    defs: &[CredentialDefinition],
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(SavedCredentials, BTreeMap<String, String>), String> {
    let ids = definitions(defs)?;
    if updates.len() > 16 {
        return Err("Too many credential updates".into());
    }
    let mut total = 0usize;
    for (id, value) in updates {
        if !ids.contains(id.as_str()) {
            return Err("Unknown credential ID".into());
        }
        if let Some(value) = value {
            if value.len() > VALUE_LIMIT || value.contains('\0') {
                return Err("Invalid credential value or value exceeds size limit".into());
            }
            total = total.saturating_add(value.len());
        }
        if total > TOTAL_LIMIT {
            return Err("Credentials exceed total size limit".into());
        }
    }
    let saved = load(data_dir)?;
    // All incoming strings are bounded before cloning.
    let mut values = saved.values.clone();
    values.retain(|id, _| ids.contains(id.as_str()));
    for (id, value) in updates {
        match value {
            Some(value) => {
                values.insert(id.clone(), value.clone());
            }
            None => {
                values.remove(id);
            }
        }
    }
    values_valid(&values)?;
    Ok((saved, values))
}

fn sync_parent(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Cannot sync credential directory")?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn persist(
    data_dir: &Path,
    saved: &SavedCredentials,
    sync: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    // Values are bounded before serialization (at most 16 KiB plus JSON escaping).
    let bytes = serde_json::to_vec(saved).map_err(|_| "Cannot encode credentials")?;
    if bytes.len() > FILE_LIMIT {
        return Err("Encoded credentials exceed file size limit".into());
    }
    fs::create_dir_all(data_dir).map_err(|_| "Cannot create credential directory")?;
    let temporary = data_dir.join(format!(".credentials-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Do not remove a preexisting path if create_new fails.
    let mut file = options
        .open(&temporary)
        .map_err(|_| "Cannot create private credential temporary file")?;
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "Cannot set private credential permissions")?;
        }
        file.write_all(&bytes)
            .map_err(|_| "Cannot write credential file")?;
        file.sync_all().map_err(|_| "Cannot sync credential file")?;
        fs::rename(&temporary, data_dir.join("credentials.json"))
            .map_err(|_| "Cannot replace credential file")?;
        sync(data_dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("studio-credentials-{}", Uuid::new_v4())))
        }
        fn defs(&self) -> Vec<CredentialDefinition> {
            ["access_token", "refresh_token"]
                .into_iter()
                .map(|id| CredentialDefinition {
                    id: id.into(),
                    label: id.replace('_', " "),
                })
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn changes(id: &str, value: Option<&str>) -> BTreeMap<String, Option<String>> {
        [(id.into(), value.map(str::to_owned))].into()
    }

    fn oauth_fixture() -> (
        Vec<CredentialDefinition>,
        BTreeMap<String, Option<String>>,
        OAuthMetadata,
    ) {
        let defs = ["client_id", "access_token", "refresh_token", "extra_token"]
            .into_iter()
            .map(|id| CredentialDefinition {
                id: id.into(),
                label: "Private credential".into(),
            })
            .collect();
        let updates = BTreeMap::from([
            ("client_id".into(), Some("example-client".into())),
            ("access_token".into(), Some("PRIVATE-ACCESS".into())),
            ("refresh_token".into(), Some("PRIVATE-REFRESH".into())),
        ]);
        let metadata = OAuthMetadata {
            client_id: "example-client".into(),
            user_id: "PRIVATE-USER".into(),
            scopes: BTreeSet::from(["example:read".into()]),
            // Intentionally expired: readers must preserve refresh information.
            expires_at: 1,
        };
        (defs, updates, metadata)
    }

    #[test]
    fn legacy_credentials_have_no_metadata_and_oauth_promotion_preserves_values() {
        let fixture = Fixture::new();
        let (defs, updates, metadata) = oauth_fixture();
        assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().is_none());
        let legacy = update(&fixture.0, &defs, &updates).unwrap();
        let before = fs::read(fixture.0.join("credentials.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert!(json.get("oauth_metadata").is_none());
        assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().is_none());
        let values = read(&fixture.0, &defs).unwrap();
        let promoted = update_oauth(&fixture.0, &defs, &BTreeMap::new(), &metadata).unwrap();
        assert_ne!(promoted.revision, legacy.revision);
        assert_eq!(read(&fixture.0, &defs).unwrap(), values);
        assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().as_ref() == Some(&metadata));
        assert_eq!(status(&fixture.0, &defs).unwrap(), promoted);
        assert!(!format!("{promoted:?}").contains("PRIVATE"));
    }

    #[test]
    fn oauth_noops_and_metadata_only_revisions_survive_fresh_reads() {
        let fixture = Fixture::new();
        let (defs, updates, mut metadata) = oauth_fixture();
        let first = update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap();
        let path = fixture.0.join("credentials.json");
        let before = fs::read(&path).unwrap();
        assert_eq!(update(&fixture.0, &defs, &updates).unwrap(), first);
        assert_eq!(update(&fixture.0, &defs, &BTreeMap::new()).unwrap(), first);
        assert_eq!(
            update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap(),
            first
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        for change in 0..3 {
            match change {
                0 => metadata.expires_at = 2,
                1 => {
                    metadata.scopes.insert("example:write".into());
                }
                _ => metadata.user_id = "PRIVATE-OTHER-USER".into(),
            }
            let previous = status(&fixture.0, &defs).unwrap();
            let next = update_oauth(&fixture.0, &defs, &BTreeMap::new(), &metadata).unwrap();
            assert_ne!(next.revision, previous.revision);
            assert_eq!(next.configured, first.configured);
            assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().as_ref() == Some(&metadata));
            assert_eq!(status(&fixture.0, &defs).unwrap(), next);
        }
        let rotated = changes("refresh_token", Some("PRIVATE-ROTATED"));
        metadata.expires_at = 3;
        let rotated_status = update_oauth(&fixture.0, &defs, &rotated, &metadata).unwrap();
        let saved = load(&fixture.0).unwrap();
        assert_eq!(saved.revision, rotated_status.revision);
        assert_eq!(saved.values["refresh_token"], "PRIVATE-ROTATED");
        assert!(saved.oauth_metadata.as_ref() == Some(&metadata));
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn manual_changes_and_declaration_pruning_invalidate_metadata() {
        for (id, value) in [
            ("client_id", Some("other-client")),
            ("access_token", Some("OTHER-ACCESS")),
            ("refresh_token", None),
            ("extra_token", Some("UNRELATED-SECRET")),
        ] {
            let fixture = Fixture::new();
            let (defs, updates, metadata) = oauth_fixture();
            let first = update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap();
            let changed = update(&fixture.0, &defs, &changes(id, value)).unwrap();
            assert_ne!(changed.revision, first.revision);
            assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().is_none());
            assert_eq!(
                read(&fixture.0, &defs).unwrap().get(id).map(String::as_str),
                value
            );
        }
        let fixture = Fixture::new();
        let (mut defs, updates, metadata) = oauth_fixture();
        let first = update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap();
        let path = fixture.0.join("credentials.json");
        let before = fs::read(&path).unwrap();
        defs.retain(|def| def.id != "refresh_token");
        assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().is_none());
        assert_eq!(fs::read(&path).unwrap(), before);
        let pruned = update(&fixture.0, &defs, &BTreeMap::new()).unwrap();
        assert_ne!(pruned.revision, first.revision);
        assert!(load(&fixture.0).unwrap().oauth_metadata.is_none());
    }

    #[test]
    fn invalid_oauth_metadata_and_pairs_have_fixed_errors_and_preserve_file() {
        let fixture = Fixture::new();
        let (defs, updates, metadata) = oauth_fixture();
        update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap();
        let path = fixture.0.join("credentials.json");
        let before = fs::read(&path).unwrap();
        for (field, value) in [
            ("client_id", serde_json::json!("")),
            ("client_id", serde_json::json!("PRIVATE".repeat(20))),
            ("user_id", serde_json::json!("PRIVATE\nUSER")),
            ("user_id", serde_json::json!("PRIVATE USER")),
            ("expires_at", serde_json::json!(0)),
            ("expires_at", serde_json::json!(253_402_300_800u64)),
            ("scopes", serde_json::json!(["PRIVATE SCOPE"])),
            ("scopes", serde_json::json!(["PRIVATE\\SCOPE"])),
            ("scopes", serde_json::json!(["PRIVATE".repeat(20)])),
            (
                "scopes",
                serde_json::json!((0..33).map(|n| format!("private:{n}")).collect::<Vec<_>>()),
            ),
        ] {
            let mut json = serde_json::to_value(&metadata).unwrap();
            json[field] = value;
            let invalid: OAuthMetadata = serde_json::from_value(json).unwrap();
            assert_eq!(
                update_oauth(&fixture.0, &defs, &BTreeMap::new(), &invalid).unwrap_err(),
                "Invalid OAuth metadata"
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        for updates in [
            changes("client_id", Some("PRIVATE-OTHER-CLIENT")),
            changes("access_token", None),
            changes("refresh_token", Some("")),
        ] {
            assert_eq!(
                update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap_err(),
                "OAuth credentials do not match metadata"
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        // Stored metadata is validated too, through fixed errors on every read.
        let mut json: serde_json::Value = serde_json::from_slice(&before).unwrap();
        json["oauth_metadata"]["client_id"] = serde_json::json!("PRIVATE-OTHER-CLIENT");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(
            read_oauth_metadata(&fixture.0, &defs).err().unwrap(),
            "OAuth credentials do not match metadata"
        );
        assert_eq!(
            status(&fixture.0, &defs).unwrap_err(),
            "OAuth credentials do not match metadata"
        );
        json["oauth_metadata"]["unknown"] = serde_json::json!("PRIVATE");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(
            read(&fixture.0, &defs).unwrap_err(),
            "Malformed credential file"
        );
    }

    #[test]
    fn post_rename_sync_failure_exposes_complete_oauth_pair_and_revision() {
        let fixture = Fixture::new();
        let (defs, updates, mut metadata) = oauth_fixture();
        update_oauth(&fixture.0, &defs, &updates, &metadata).unwrap();
        let mut saved = load(&fixture.0).unwrap();
        metadata.expires_at = 10;
        saved.oauth_metadata = Some(metadata.clone());
        saved
            .values
            .insert("access_token".into(), "PRIVATE-NEW-ACCESS".into());
        saved
            .values
            .insert("refresh_token".into(), "PRIVATE-NEW-REFRESH".into());
        saved.revision = Some(Uuid::new_v4());
        assert_eq!(
            persist(&fixture.0, &saved, |_| Err("Simulated sync failure".into())).unwrap_err(),
            "Simulated sync failure"
        );
        assert_eq!(status(&fixture.0, &defs).unwrap().revision, saved.revision);
        assert_eq!(read(&fixture.0, &defs).unwrap(), saved.values);
        assert!(read_oauth_metadata(&fixture.0, &defs).unwrap().as_ref() == Some(&metadata));
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn missing_partial_noop_clear_and_secret_free_status() {
        let fixture = Fixture::new();
        let defs = fixture.defs();
        assert_eq!(
            status(&fixture.0, &defs).unwrap(),
            CredentialStatus::default()
        );
        assert!(read(&fixture.0, &defs).unwrap().is_empty());
        assert_eq!(
            update(&fixture.0, &defs, &changes("access_token", None)).unwrap(),
            CredentialStatus::default()
        );
        assert!(!fixture.0.exists());
        let first = update(
            &fixture.0,
            &defs,
            &changes("access_token", Some("SENSITIVE-ONE")),
        )
        .unwrap();
        assert!(first.revision.is_some_and(|revision| !revision.is_nil()));
        assert_eq!(first.configured, BTreeSet::from(["access_token".into()]));
        assert!(!format!("{first:?}").contains("SENSITIVE-ONE"));
        let before = fs::read(fixture.0.join("credentials.json")).unwrap();
        assert_eq!(
            update(
                &fixture.0,
                &defs,
                &changes("access_token", Some("SENSITIVE-ONE"))
            )
            .unwrap(),
            first
        );
        assert_eq!(
            fs::read(fixture.0.join("credentials.json")).unwrap(),
            before
        );
        let second = update(
            &fixture.0,
            &defs,
            &changes("refresh_token", Some("SENSITIVE-TWO")),
        )
        .unwrap();
        assert_ne!(first.revision, second.revision);
        assert_eq!(
            read(&fixture.0, &defs).unwrap(),
            BTreeMap::from([
                ("access_token".into(), "SENSITIVE-ONE".into()),
                ("refresh_token".into(), "SENSITIVE-TWO".into()),
            ])
        );
        let cleared = update(&fixture.0, &defs, &changes("access_token", None)).unwrap();
        assert_eq!(cleared.configured, BTreeSet::from(["refresh_token".into()]));
        let empty = update(&fixture.0, &defs, &changes("refresh_token", None)).unwrap();
        assert!(empty.configured.is_empty());
        assert!(empty.revision.is_some());
        assert_eq!(status(&fixture.0, &defs).unwrap(), empty);
    }

    #[test]
    fn definition_and_update_bounds_fail_without_overwriting_or_echoing_inputs() {
        let fixture = Fixture::new();
        let defs = fixture.defs();
        update(
            &fixture.0,
            &defs,
            &changes("access_token", Some("original")),
        )
        .unwrap();
        let original = fs::read(fixture.0.join("credentials.json")).unwrap();
        for updates in [
            changes("SENSITIVE-UNKNOWN", Some("SENSITIVE-VALUE")),
            changes("access_token", Some(&"SENSITIVE".repeat(600))),
            changes("access_token", Some("SENSITIVE\0VALUE")),
        ] {
            let error = update(&fixture.0, &defs, &updates).unwrap_err();
            assert!(!error.contains("SENSITIVE"));
            assert_eq!(
                fs::read(fixture.0.join("credentials.json")).unwrap(),
                original
            );
        }
        for id in [
            "",
            "Upper",
            "under-dash",
            "has space",
            "é",
            "1first",
            &"a".repeat(65),
        ] {
            assert!(
                definitions(&[CredentialDefinition {
                    id: id.into(),
                    label: "label".into()
                }])
                .is_err()
            );
        }
        for label in ["", "   ", "bad\0label", &"a".repeat(129)] {
            assert!(
                definitions(&[CredentialDefinition {
                    id: "valid_id".into(),
                    label: label.into()
                }])
                .is_err()
            );
        }
        assert!(definitions(&[defs[0].clone(), defs[0].clone()]).is_err());
        assert!(
            serde_json::from_str::<CredentialDefinition>(
                r#"{"id":"token","label":"Token","value":"SENSITIVE"}"#
            )
            .is_err()
        );
        assert_eq!(
            fs::read(fixture.0.join("credentials.json")).unwrap(),
            original
        );
    }

    #[test]
    fn removed_or_renamed_declarations_hide_secrets_and_explicit_save_prunes_once() {
        for remove_all in [false, true] {
            let fixture = Fixture::new();
            let original_defs = fixture.defs();
            let original = update(
                &fixture.0,
                &original_defs,
                &BTreeMap::from([
                    ("access_token".into(), Some("OLD-SECRET".into())),
                    ("refresh_token".into(), Some("RETAINED-SECRET".into())),
                ]),
            )
            .unwrap();
            let path = fixture.0.join("credentials.json");
            let before = fs::read(&path).unwrap();
            let current_defs = if remove_all {
                vec![]
            } else {
                vec![
                    CredentialDefinition {
                        id: "renamed_token".into(),
                        label: "Renamed token".into(),
                    },
                    original_defs[1].clone(),
                ]
            };
            let filtered = status(&fixture.0, &current_defs).unwrap();
            assert_eq!(filtered.revision, original.revision);
            let expected: BTreeMap<String, String> = if remove_all {
                BTreeMap::new()
            } else {
                BTreeMap::from([("refresh_token".into(), "RETAINED-SECRET".into())])
            };
            assert_eq!(read(&fixture.0, &current_defs).unwrap(), expected);
            assert_eq!(filtered.configured, expected.keys().cloned().collect());
            assert_eq!(fs::read(&path).unwrap(), before);
            // Stale update keys are still rejected, including explicit clears.
            assert!(update(&fixture.0, &current_defs, &changes("access_token", None)).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
            let pruned = update(&fixture.0, &current_defs, &BTreeMap::new()).unwrap();
            assert_ne!(pruned.revision, original.revision);
            assert_eq!(pruned.configured, filtered.configured);
            let saved = load(&fixture.0).unwrap();
            assert_eq!(saved.values, expected);
            let after = fs::read(&path).unwrap();
            assert!(
                !String::from_utf8(after.clone())
                    .unwrap()
                    .contains("OLD-SECRET")
            );
            assert_eq!(
                update(&fixture.0, &current_defs, &BTreeMap::new()).unwrap(),
                pruned
            );
            assert_eq!(fs::read(&path).unwrap(), after);
            if !remove_all {
                let renamed = update(
                    &fixture.0,
                    &current_defs,
                    &changes("renamed_token", Some("NEW-SECRET")),
                )
                .unwrap();
                assert_ne!(renamed.revision, pruned.revision);
                assert_eq!(
                    read(&fixture.0, &current_defs).unwrap(),
                    BTreeMap::from([
                        ("renamed_token".into(), "NEW-SECRET".into()),
                        ("refresh_token".into(), "RETAINED-SECRET".into()),
                    ])
                );
            }
            // Reintroducing an old declaration cannot resurrect a pruned value.
            assert!(
                !read(&fixture.0, &original_defs)
                    .unwrap()
                    .contains_key("access_token")
            );
        }
    }

    #[test]
    fn explicit_partial_save_prunes_removed_values_and_preserves_unchanged_declared_values() {
        let fixture = Fixture::new();
        let defs = fixture.defs();
        update(
            &fixture.0,
            &defs,
            &BTreeMap::from([
                ("access_token".into(), Some("OLD-SECRET".into())),
                ("refresh_token".into(), Some("RETAINED-SECRET".into())),
            ]),
        )
        .unwrap();
        let renamed = vec![
            defs[1].clone(),
            CredentialDefinition {
                id: "new_token".into(),
                label: "New token".into(),
            },
        ];
        update(
            &fixture.0,
            &renamed,
            &changes("new_token", Some("NEW-SECRET")),
        )
        .unwrap();
        let saved = load(&fixture.0).unwrap();
        assert_eq!(
            saved.values,
            BTreeMap::from([
                ("new_token".into(), "NEW-SECRET".into()),
                ("refresh_token".into(), "RETAINED-SECRET".into()),
            ])
        );
    }

    #[test]
    fn undeclared_stored_values_still_require_valid_ids_and_size_bounds() {
        let fixture = Fixture::new();
        for (id, value) in [
            ("old_token".to_owned(), "x".repeat(VALUE_LIMIT + 1)),
            ("a".repeat(65), "secret".to_owned()),
            ("old_token".to_owned(), "bad\0secret".to_owned()),
        ] {
            let saved = SavedCredentials {
                revision: Some(Uuid::new_v4()),
                values: BTreeMap::from([(id, value)]),
                oauth_metadata: None,
            };
            persist(&fixture.0, &saved, sync_parent).unwrap();
            let before = fs::read(fixture.0.join("credentials.json")).unwrap();
            assert!(status(&fixture.0, &[]).is_err());
            assert!(read(&fixture.0, &[]).is_err());
            assert!(update(&fixture.0, &[], &BTreeMap::new()).is_err());
            assert_eq!(
                fs::read(fixture.0.join("credentials.json")).unwrap(),
                before
            );
        }
    }

    #[test]
    fn total_and_encoded_bounds_are_checked_and_existing_values_count() {
        let fixture = Fixture::new();
        let defs: Vec<_> = (0..16)
            .map(|i| CredentialDefinition {
                id: format!("token_{i}"),
                label: "Token".into(),
            })
            .collect();
        let value = "x".repeat(VALUE_LIMIT);
        let four: BTreeMap<_, _> = (0..4)
            .map(|i| (format!("token_{i}"), Some(value.clone())))
            .collect();
        update(&fixture.0, &defs, &four).unwrap();
        let path = fixture.0.join("credentials.json");
        let before = fs::read(&path).unwrap();
        assert!(update(&fixture.0, &defs, &changes("token_4", Some("x"))).is_err());
        let five: BTreeMap<_, _> = (0..5)
            .map(|i| (format!("token_{i}"), Some(value.clone())))
            .collect();
        assert!(update(&fixture.0, &defs, &five).is_err());
        let escaped: BTreeMap<_, _> = (0..4)
            .map(|i| (format!("token_{i}"), Some("\u{0001}".repeat(VALUE_LIMIT))))
            .collect();
        assert!(update(&fixture.0, &defs, &escaped).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let too_many: Vec<_> = (0..17)
            .map(|i| CredentialDefinition {
                id: format!("token_{i}"),
                label: "Token".into(),
            })
            .collect();
        assert!(status(&fixture.0, &too_many).is_err());
    }

    #[test]
    fn malformed_oversized_and_nonregular_files_stay_intact_with_safe_errors() {
        let fixture = Fixture::new();
        let defs = fixture.defs();
        update(
            &fixture.0,
            &defs,
            &changes("access_token", Some("original")),
        )
        .unwrap();
        let path = fixture.0.join("credentials.json");
        for bytes in [
            b"SENSITIVE-MALFORMED".to_vec(),
            br#"{"revision":null,"values":{"access_token":"SENSITIVE"}}"#.to_vec(),
            format!(
                r#"{{"revision":"{}","values":{{"SENSITIVE-UNKNOWN":"SENSITIVE"}}}}"#,
                Uuid::new_v4()
            )
            .into_bytes(),
            vec![b'x'; FILE_LIMIT + 1],
        ] {
            fs::write(&path, &bytes).unwrap();
            for error in [
                status(&fixture.0, &defs).unwrap_err(),
                read(&fixture.0, &defs).unwrap_err(),
                update(&fixture.0, &defs, &BTreeMap::new()).unwrap_err(),
            ] {
                assert!(!error.contains("SENSITIVE"));
            }
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(status(&fixture.0, &defs).is_err());
        assert!(path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn private_permissions_owner_and_symlinks_are_enforced() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let fixture = Fixture::new();
        let defs = fixture.defs();
        update(
            &fixture.0,
            &defs,
            &changes("access_token", Some("SENSITIVE")),
        )
        .unwrap();
        let path = fixture.0.join("credentials.json");
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o600);
        assert_eq!(meta.uid(), unsafe { libc::geteuid() });
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&fixture.0, &defs).is_err());
        assert!(update(&fixture.0, &defs, &changes("access_token", None)).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let original = fs::read(&path).unwrap();
        let retained = fixture.0.join("retained.json");
        fs::rename(&path, &retained).unwrap();
        symlink(&retained, &path).unwrap();
        assert!(status(&fixture.0, &defs).is_err());
        assert!(update(&fixture.0, &defs, &changes("access_token", None)).is_err());
        assert_eq!(fs::read(&retained).unwrap(), original);
        fs::remove_file(&path).unwrap();
        symlink(fixture.0.join("missing"), &path).unwrap();
        assert!(read(&fixture.0, &defs).is_err());
    }

    #[test]
    fn post_rename_sync_error_keeps_committed_status_readable_and_cleans_temporary() {
        let fixture = Fixture::new();
        let defs = fixture.defs();
        let revision = Uuid::new_v4();
        let saved = SavedCredentials {
            revision: Some(revision),
            values: BTreeMap::from([("access_token".into(), "SENSITIVE".into())]),
            oauth_metadata: None,
        };
        let error = persist(&fixture.0, &saved, |_| {
            Err("Simulated directory sync failure".into())
        })
        .unwrap_err();
        assert!(!error.contains("SENSITIVE"));
        let status = status(&fixture.0, &defs).unwrap();
        assert_eq!(status.revision, Some(revision));
        assert_eq!(status.configured, BTreeSet::from(["access_token".into()]));
        assert!(!format!("{status:?}").contains("SENSITIVE"));
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }
}
