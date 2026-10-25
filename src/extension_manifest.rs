//! Bounded API-v1 content-pack descriptions. No installation, execution or runtime state.
//!
//! `extension.json` uses dotted package IDs (`org.example.starter`) and local
//! slug IDs (`greeting`). Resolution produces `org.example.starter/greeting`.
//! Asset checks cover paths and supported extensions, not media decoding; use
//! the host's existing model/motion/image importers before activating assets.
//! Validation is a filesystem snapshot, not a sandbox or protection from later
//! file replacement. A copying registry must validate its destination again.

use crate::extension_protocol::{Capability, MAX_CAPABILITIES};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MANIFEST_FILE: &str = "extension.json";
pub const API_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_CONTENT_ENTRIES: usize = 512;
pub const MAX_PROCESS_ARGS: usize = 64;
pub const MAX_PROCESS_ARG_BYTES: usize = 4096;
pub const MAX_PROCESS_ARGS_BYTES: usize = 16 * 1024;
pub const MAX_PROCESS_PATH_BYTES: usize = 1024;
pub const MAX_HOOKS: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub api_version: u32,
    pub id: String,
    pub version: String,
    pub name: String,
    pub description: String,
    pub provenance: Provenance,
    #[serde(default)]
    pub content: Content,
    #[serde(default)]
    pub process: Option<ProcessMetadata>,
    /// Explicit local test handler; does not replace the normal process/hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_process: Option<ProcessMetadata>,
    #[serde(default)]
    pub hooks: Vec<HookMetadata>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub settings: Vec<crate::extension_settings::SettingDefinition>,
    #[serde(default)]
    pub credentials: Vec<crate::extension_credentials::CredentialDefinition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub author: String,
    pub license: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Content {
    pub characters: Vec<CharacterContent>,
    pub motions: Vec<MotionContent>,
    pub expressions: Vec<ExpressionContent>,
    pub backgrounds: Vec<BackgroundContent>,
    pub presets: Vec<PresetContent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterContent {
    pub id: String,
    pub name: String,
    pub asset: PathBuf,
    pub persona: String,
    pub greeting: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionContent {
    pub id: String,
    pub name: String,
    pub asset: PathBuf,
    pub role: MotionRole,
    #[serde(default)]
    pub emotion: Option<Emotion>,
    #[serde(default)]
    pub loopable: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionRole {
    Idle,
    Talking,
    Gesture,
}

/// Names supported by the host's affect system, kept independent of Bevy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emotion {
    Neutral,
    Happy,
    Sad,
    Angry,
    Relaxed,
    Surprised,
    Amused,
    Excited,
    Curious,
    Confused,
    Thinking,
    Annoyed,
    Embarrassed,
    Grateful,
    Optimistic,
    Proud,
    Nervous,
    Caring,
    Fear,
    Disgust,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionContent {
    pub id: String,
    pub name: String,
    pub emotion: Emotion,
    /// Authored avatar expression name; availability is checked on avatar import.
    pub expression: String,
    pub strength: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundContent {
    pub id: String,
    pub name: String,
    pub source: BackgroundSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackgroundSource {
    Image { asset: PathBuf },
    Room { room: RoomId },
}

/// Only existing room implementations; packages cannot define renderer features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomId {
    CozyRoom,
    MoonlitGarden,
    NeonRooftop,
    RoseBedroom,
    VioletStudio,
    CyanLounge,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetContent {
    pub id: String,
    pub name: String,
    pub settings: PresetSettings,
}

/// Explicit SceneSettings fields only. None means leave the user's value alone.
/// No provider, credentials, arbitrary JSON merge or new setting definitions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PresetSettings {
    pub background: Option<[f32; 3]>,
    pub portrait: Option<bool>,
    pub idle_motion: Option<bool>,
    pub outlines: Option<bool>,
    pub room_view: Option<bool>,
}

/// Descriptive executable metadata only. Never launched by this module.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessMetadata {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookMetadata {
    pub event: String,
    pub handler: ProcessMetadata,
}

/// Owned validation result. Deserializing this type does not validate a package;
/// trust only a fresh loader/resolver result at an installation boundary.
/// Executable metadata requires the explicit integration loader/resolver.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedPack {
    pub manifest: Manifest,
    pub root: PathBuf,
    pub content: Vec<ResolvedContent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedContent {
    pub id: String,
    pub local_id: String,
    pub kind: ContentKind,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Character,
    Motion,
    Expression,
    Background,
    Preset,
}

/// Read at most 256 KiB + one sentinel byte, then validate the entire package.
/// Executable metadata/capabilities are explicitly unsupported for installation.
pub fn load_pack(root: &Path) -> Result<ResolvedPack, String> {
    resolve_pack(root, read_manifest(root)?)
}

/// Explicit opt-in validation for a bundled persistent process or bounded hooks. This does not
/// grant capabilities, approve installation, or execute code. Like content-only
/// loading, the installed destination must be revalidated after copying.
/// Manifest process paths remain relative; canonical containment is checked here.
pub fn load_integration_pack(root: &Path) -> Result<ResolvedPack, String> {
    resolve_integration_pack(root, read_manifest(root)?)
}

/// Read-only author check. Selects the existing content/integration validator;
/// this never installs a pack, grants capabilities, or launches a process.
pub fn check_pack(root: &Path) -> Result<ResolvedPack, String> {
    let manifest = read_manifest(root)?;
    if manifest.process.is_some() || manifest.test_process.is_some() || !manifest.hooks.is_empty() {
        resolve_integration_pack(root, manifest)
    } else {
        resolve_pack(root, manifest)
    }
}

fn read_manifest(root: &Path) -> Result<Manifest, String> {
    let root = canonical_root(root)?;
    let path = contained_file(&root, Path::new(MANIFEST_FILE))?;
    let file = File::open(&path).map_err(|e| format!("Cannot open {MANIFEST_FILE}: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(manifest_size_error());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read {MANIFEST_FILE}: {e}"))?;
    parse_manifest(&bytes)
}

/// Parse bounded, strict JSON and validate metadata/content descriptors.
/// This inspection API preserves process/test_process/hooks/capabilities; it grants no right
/// to install them. load_pack/resolve_pack remain content-only; the explicit
/// integration APIs additionally validate executable metadata.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(manifest_size_error());
    }
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid {MANIFEST_FILE}: {e}"))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// Resolve all content or return an error; never returns a partially valid pack.
/// Safe for callers that constructed or edited a Manifest directly as well.
pub fn resolve_pack(root: &Path, manifest: Manifest) -> Result<ResolvedPack, String> {
    validate_manifest(&manifest)?;
    if manifest.process.is_some()
        || manifest.test_process.is_some()
        || !manifest.hooks.is_empty()
        || !manifest.capabilities.is_empty()
        || !manifest.settings.is_empty()
        || !manifest.credentials.is_empty()
    {
        return Err("Executable process/test_process/hooks, capabilities, settings or credentials are unsupported for content-only installation; use explicit local integration installation; external processes execute local code and are not sandboxed".into());
    }
    resolve_content(root, manifest)
}

/// Explicit opt-in equivalent of resolve_pack for directly constructed manifests.
/// Choose one persistent process or 1..=4 unique supported event hooks.
/// Program/working-directory metadata
/// remains pack-relative for rebinding at the installed root. Arguments are
/// literal argv entries, never shell text; no command/PATH lookup is performed.
/// Bundled code, including shebang scripts, is local code and is not sandboxed.
pub fn resolve_integration_pack(root: &Path, manifest: Manifest) -> Result<ResolvedPack, String> {
    // Shared validation includes every content descriptor and the total size cap.
    let pack = resolve_content(root, manifest)?;
    if pack.manifest.process.is_some() && !pack.manifest.hooks.is_empty() {
        return Err("Integration pack must choose a persistent process or hooks, not both".into());
    }
    if pack.manifest.process.is_none() && pack.manifest.hooks.is_empty() {
        return Err(format!(
            "Integration pack requires a persistent process or 1..={MAX_HOOKS} supported hooks"
        ));
    }
    if pack.manifest.hooks.len() > MAX_HOOKS {
        return Err(format!("At most {MAX_HOOKS} unique hooks may be declared"));
    }
    if pack.manifest.capabilities.len() > MAX_CAPABILITIES {
        return Err(format!(
            "At most {MAX_CAPABILITIES} known capabilities may be requested"
        ));
    }
    let mut capabilities = BTreeSet::new();
    for name in &pack.manifest.capabilities {
        let capability: Capability =
            serde_json::from_value(serde_json::Value::String(name.clone()))
                .map_err(|_| format!("Unknown integration capability '{name}'"))?;
        if !capabilities.insert(capability) {
            return Err(format!("Duplicate integration capability '{name}'"));
        }
    }
    if let Some(process) = &pack.manifest.process {
        validate_process(&pack.root, process)?;
    }
    if let Some(process) = &pack.manifest.test_process {
        validate_process(&pack.root, process).map_err(|error| format!("Test process: {error}"))?;
    }
    let mut triggers = BTreeSet::new();
    for hook in &pack.manifest.hooks {
        let _: crate::extension_hooks::HookTrigger =
            serde_json::from_value(serde_json::Value::String(hook.event.clone())).map_err(|_| {
                format!("Unsupported hook event '{}'; expected motion.playback.started, motion.playback.ended, extension.started, extension.stopping, extension.stopped, app.ready, character.selected, reply.finished, speech.started, speech.stopped or emotion.changed", hook.event)
            })?;
        if !triggers.insert(&hook.event) {
            return Err(format!("Duplicate hook event '{}'", hook.event));
        }
        validate_process(&pack.root, &hook.handler)
            .map_err(|error| format!("Hook '{}': {error}", hook.event))?;
    }
    Ok(pack)
}

/// Shared containment, executable-bit and argv bounds for every executable entry.
fn validate_process(root: &Path, process: &ProcessMetadata) -> Result<(), String> {
    if process.program.len() > MAX_PROCESS_PATH_BYTES {
        return Err(format!(
            "Process program path exceeds {MAX_PROCESS_PATH_BYTES} bytes"
        ));
    }
    let program = contained_file(root, Path::new(&process.program))
        .map_err(|e| format!("Process program: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&program)
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err("Bundled process program must have an executable permission bit".into());
        }
    }
    #[cfg(not(unix))]
    let _ = program;
    if process.args.len() > MAX_PROCESS_ARGS {
        return Err(format!("Process exceeds {MAX_PROCESS_ARGS} arguments"));
    }
    let mut bytes = 0usize;
    for arg in &process.args {
        if arg.len() > MAX_PROCESS_ARG_BYTES || arg.chars().any(char::is_control) {
            return Err(format!(
                "Process arguments must contain no control characters and be at most {MAX_PROCESS_ARG_BYTES} bytes each"
            ));
        }
        bytes = bytes.saturating_add(arg.len().saturating_add(1));
        if bytes > MAX_PROCESS_ARGS_BYTES {
            return Err(format!(
                "Process arguments exceed {MAX_PROCESS_ARGS_BYTES} total bytes including terminators"
            ));
        }
    }
    if let Some(relative) = &process.working_directory {
        let raw = relative
            .to_str()
            .ok_or("Process working directory must be UTF-8")?;
        if raw.len() > MAX_PROCESS_PATH_BYTES {
            return Err(format!(
                "Process working directory exceeds {MAX_PROCESS_PATH_BYTES} bytes"
            ));
        }
        // An empty path or '.' explicitly denotes the installed pack root.
        let directory = if raw.is_empty() || raw == "." {
            root.to_owned()
        } else {
            contained_path(root, relative).map_err(|e| format!("Process working directory: {e}"))?
        };
        if !fs::metadata(&directory)
            .map_err(|e| e.to_string())?
            .is_dir()
        {
            return Err("Process working directory must be a directory".into());
        }
    }
    Ok(())
}

fn resolve_content(root: &Path, manifest: Manifest) -> Result<ResolvedPack, String> {
    validate_manifest(&manifest)?;
    // Also bound programmatically constructed manifests, rather than just JSON input.
    if serde_json::to_vec(&manifest)
        .map_err(|e| e.to_string())?
        .len()
        > MAX_MANIFEST_BYTES
    {
        return Err(manifest_size_error());
    }
    let root = canonical_root(root)?;
    let mut content = Vec::new();
    let mut add = |id: &str, kind: ContentKind, asset: Option<&Path>| -> Result<(), String> {
        let path = asset
            .map(|path| resolve_asset(&root, path, kind))
            .transpose()
            .map_err(|e| format!("Content '{id}': {e}"))?;
        content.push(ResolvedContent {
            id: format!("{}/{id}", manifest.id),
            local_id: id.to_owned(),
            kind,
            path,
        });
        Ok(())
    };
    for entry in &manifest.content.characters {
        add(&entry.id, ContentKind::Character, Some(&entry.asset))?;
    }
    for entry in &manifest.content.motions {
        add(&entry.id, ContentKind::Motion, Some(&entry.asset))?;
    }
    for entry in &manifest.content.expressions {
        add(&entry.id, ContentKind::Expression, None)?;
    }
    for entry in &manifest.content.backgrounds {
        let asset = match &entry.source {
            BackgroundSource::Image { asset } => Some(asset.as_path()),
            BackgroundSource::Room { .. } => None,
        };
        add(&entry.id, ContentKind::Background, asset)?;
    }
    for entry in &manifest.content.presets {
        add(&entry.id, ContentKind::Preset, None)?;
    }
    Ok(ResolvedPack {
        manifest,
        root,
        content,
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<(), String> {
    crate::extension_settings::validate_schema(&manifest.settings)?;
    crate::extension_credentials::validate_definitions(&manifest.credentials)?;
    if manifest.api_version != API_VERSION {
        return Err(format!(
            "Unsupported api_version {}; supported version is {API_VERSION}",
            manifest.api_version
        ));
    }
    let segments: Vec<_> = manifest.id.split('.').collect();
    if manifest.id.len() > 128 || segments.len() < 2 || !segments.iter().all(|s| slug(s)) {
        return Err("Invalid package id: use a dotted lowercase namespace such as org.example.starter (max 128 bytes)".into());
    }
    if !valid_version(&manifest.version) {
        return Err("Invalid package version: expected semantic version MAJOR.MINOR.PATCH with optional -prerelease and +build (max 64 bytes), without leading zeroes in numeric version/prerelease identifiers".into());
    }
    name(&manifest.name, "Package name")?;
    text(&manifest.description, "Package description")?;
    text(&manifest.provenance.author, "Provenance author")?;
    text(&manifest.provenance.license, "Provenance license")?;
    if let Some(source) = &manifest.provenance.source {
        text(source, "Provenance source")?;
    }
    let c = &manifest.content;
    let count = [
        c.characters.len(),
        c.motions.len(),
        c.expressions.len(),
        c.backgrounds.len(),
        c.presets.len(),
    ]
    .into_iter()
    .try_fold(0usize, |n, len| n.checked_add(len));
    if count.is_none_or(|n| n > MAX_CONTENT_ENTRIES) {
        return Err(format!(
            "Content exceeds {MAX_CONTENT_ENTRIES} entries across all categories"
        ));
    }
    let mut ids = BTreeSet::new();
    let mut identity = |id: &str, display_name: &str| -> Result<(), String> {
        if !slug(id) {
            return Err(format!(
                "Invalid local content id '{id}': use 1-64 lowercase letters/digits, hyphens or underscores, beginning with a letter/digit"
            ));
        }
        if !ids.insert(id.to_owned()) {
            return Err(format!(
                "Duplicate local content id '{id}' across content categories"
            ));
        }
        name(display_name, &format!("Content '{id}' name"))
    };
    for entry in &c.characters {
        identity(&entry.id, &entry.name)?;
        text(&entry.persona, &format!("Character '{}' persona", entry.id))?;
        text(
            &entry.greeting,
            &format!("Character '{}' greeting", entry.id),
        )?;
    }
    for entry in &c.motions {
        identity(&entry.id, &entry.name)?;
    }
    for entry in &c.expressions {
        identity(&entry.id, &entry.name)?;
        text(
            &entry.expression,
            &format!("Expression '{}' mapping", entry.id),
        )?;
        unit(
            entry.strength,
            &format!("Expression '{}' strength", entry.id),
        )?;
    }
    for entry in &c.backgrounds {
        identity(&entry.id, &entry.name)?;
    }
    for entry in &c.presets {
        identity(&entry.id, &entry.name)?;
        let s = &entry.settings;
        if s.background.is_none()
            && s.portrait.is_none()
            && s.idle_motion.is_none()
            && s.outlines.is_none()
            && s.room_view.is_none()
        {
            return Err(format!(
                "Preset '{}' must describe at least one supported setting",
                entry.id
            ));
        }
        if let Some(color) = s.background {
            for component in color {
                unit(
                    component,
                    &format!("Preset '{}' background component", entry.id),
                )?;
            }
        }
    }
    if let Some(process) = &manifest.process {
        text(&process.program, "Executable process program")?;
    }
    if let Some(process) = &manifest.test_process {
        text(&process.program, "Executable test process program")?;
    }
    for hook in &manifest.hooks {
        text(&hook.event, "Executable hook event")?;
        text(&hook.handler.program, "Executable hook program")?;
    }
    for capability in &manifest.capabilities {
        text(capability, "Requested capability")?;
    }
    Ok(())
}

fn text(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.contains('\0') {
        Err(format!("{label} must be nonempty and contain no NUL"))
    } else {
        Ok(())
    }
}

fn name(value: &str, label: &str) -> Result<(), String> {
    text(value, label)?;
    if value.len() > 256 {
        return Err(format!("{label} exceeds 256 bytes"));
    }
    Ok(())
}

fn unit(value: f32, label: &str) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        Err(format!("{label} must be finite and within [0, 1]"))
    } else {
        Ok(())
    }
}

fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn valid_version(value: &str) -> bool {
    fn numeric(s: &str) -> bool {
        !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s.len() == 1 || !s.starts_with('0'))
    }
    fn identifiers(s: &str, prerelease: bool) -> bool {
        s.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && (!prerelease || !part.bytes().all(|b| b.is_ascii_digit()) || numeric(part))
        })
    }
    if value.len() > 64 {
        return false;
    }
    let (release, build) = value
        .split_once('+')
        .map_or((value, None), |(v, b)| (v, Some(b)));
    if build.is_some_and(|b| !identifiers(b, false)) {
        return false;
    }
    let (core, pre) = release
        .split_once('-')
        .map_or((release, None), |(v, p)| (v, Some(p)));
    if pre.is_some_and(|p| !identifiers(p, true)) {
        return false;
    }
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3 && parts.into_iter().all(numeric)
}

fn manifest_size_error() -> String {
    format!("{MANIFEST_FILE} exceeds {MAX_MANIFEST_BYTES} bytes (256 KiB)")
}

fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve package root '{}': {e}", root.display()))?;
    if !root.is_dir() {
        return Err("Package root must be a directory".into());
    }
    Ok(root)
}

fn contained_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let raw = relative.to_str().ok_or("Asset path must be UTF-8")?;
    // Use portable manifest paths: reject Windows drives/UNC paths on Unix too.
    if raw.is_empty()
        || raw.contains(['\\', ':', '\0'])
        || raw
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "Invalid package-relative file path '{raw}': absolute paths, traversal and nonportable separators are forbidden"
        ));
    }
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve package file '{raw}': {e}"))?;
    if !path.starts_with(root) {
        return Err(format!(
            "Package file '{raw}' escapes package root through a symlink"
        ));
    }
    Ok(path)
}

fn contained_file(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let path = contained_path(root, relative)?;
    let raw = relative.to_string_lossy();
    if !fs::metadata(&path)
        .map_err(|e| format!("Cannot inspect '{raw}': {e}"))?
        .is_file()
    {
        return Err(format!("Package file '{raw}' must be a regular file"));
    }
    Ok(path)
}

fn resolve_asset(root: &Path, relative: &Path, kind: ContentKind) -> Result<PathBuf, String> {
    let supported: &[&str] = match kind {
        // Text glTF requires external dependency resolution, outside this foundation.
        ContentKind::Character => &["vrm", "glb"],
        ContentKind::Motion => &["bvh", "vrma", "fbx"],
        ContentKind::Background => &["png", "jpg", "jpeg", "webp"],
        _ => return Err("This content category cannot reference an asset".into()),
    };
    let path = contained_file(root, relative)?;
    for candidate in [relative, path.as_path()] {
        let extension = candidate
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !supported.contains(&extension.as_str()) {
            return Err(format!(
                "Unsupported {kind:?} format for '{}'; supported extensions: {}",
                candidate.display(),
                supported.join(", ")
            ));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "waifu-extension-manifest-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn write_manifest(&self, value: &Value) {
            fs::write(
                self.0.join(MANIFEST_FILE),
                serde_json::to_vec(value).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn minimal() -> Value {
        json!({
            "api_version": 1,
            "id": "org.example.starter",
            "version": "1.2.3-beta.1+local.01",
            "name": "Starter",
            "description": "Local content fixture",
            "provenance": {"author": "Fixture author", "license": "CC0-1.0"},
            "content": {}
        })
    }

    fn parse(value: &Value) -> Result<Manifest, String> {
        parse_manifest(&serde_json::to_vec(value).unwrap())
    }

    fn preset(id: &str) -> Value {
        json!({"id": id, "name": "Portrait", "settings": {"portrait": true}})
    }

    fn motion(asset: &str) -> Value {
        json!({"id": "idle", "name": "Idle", "asset": asset, "role": "idle", "loopable": true})
    }

    fn fixture() -> (TestDir, Value) {
        let dir = TestDir::new();
        fs::create_dir(dir.0.join("assets")).unwrap();
        // A tiny glTF binary container with a single empty scene, not a mock path.
        let mut scene = br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{}]}"#.to_vec();
        while !scene.len().is_multiple_of(4) {
            scene.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(20u32 + scene.len() as u32).to_le_bytes());
        glb.extend_from_slice(&(scene.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&scene);
        fs::write(dir.0.join("assets/model.glb"), glb).unwrap();
        fs::write(dir.0.join("assets/idle.bvh"), b"HIERARCHY\nROOT Hips\n{\n OFFSET 0 0 0\n CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation\n End Site\n {\n OFFSET 0 1 0\n }\n}\nMOTION\nFrames: 2\nFrame Time: 0.033333\n0 0 0 0 0 0\n0 0 0 0 0 0\n").unwrap();
        // A one-pixel PNG; this module checks containment/format, not decoding.
        fs::write(
            dir.0.join("assets/background.png"),
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100,
                248, 15, 0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
                130,
            ],
        )
        .unwrap();
        let mut value = minimal();
        value["content"] = json!({
            "characters": [{"id": "friend", "name": "Friend", "asset": "assets/model.glb", "persona": "A friendly companion", "greeting": "Hello"}],
            "motions": [motion("assets/idle.bvh")],
            "expressions": [{"id": "smile", "name": "Smile", "emotion": "happy", "expression": "joy", "strength": 0.75}],
            "backgrounds": [
                {"id": "wallpaper", "name": "Wallpaper", "source": {"type": "image", "asset": "assets/background.png"}},
                {"id": "room", "name": "Room", "source": {"type": "room", "room": "cozy_room"}}
            ],
            "presets": [preset("portrait")]
        });
        (dir, value)
    }

    fn integration_fixture() -> (TestDir, Value) {
        let (dir, mut value) = fixture();
        fs::create_dir(dir.0.join("bin")).unwrap();
        fs::create_dir(dir.0.join("work")).unwrap();
        fs::write(
            dir.0.join("bin/worker"),
            b"#!/usr/bin/env python3\nprint('bundled')\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.0.join("bin/worker"), fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        value["process"] = json!({"program": "bin/worker", "args": ["--literal", "hello world", "$(literal); not shell", ""], "working_directory": "work"});
        value["capabilities"] = json!(["lifecycle.observe", "reply.read", "motion.play"]);
        (dir, value)
    }

    #[test]
    fn credentials_are_strict_declarations_requiring_explicit_integration() {
        assert!(parse(&minimal()).unwrap().credentials.is_empty());
        let (dir, mut value) = integration_fixture();
        value["credentials"] = json!([{"id":"access_token", "label":"Access token"}]);
        dir.write_manifest(&value);
        let pack = load_integration_pack(&dir.0).unwrap();
        assert_eq!(pack.manifest.credentials[0].id, "access_token");
        assert!(load_pack(&dir.0).unwrap_err().contains("content-only"));
        let mut content_only = minimal();
        content_only["credentials"] = value["credentials"].clone();
        assert!(resolve_pack(&dir.0, parse(&content_only).unwrap()).is_err());
        for declarations in [
            json!([{"id":"token", "label":"Token", "value":"not permitted"}]),
            json!([{"id":"token", "label":"Token"}, {"id":"token", "label":"Duplicate"}]),
            json!([{"id":"../escape", "label":"Token"}]),
            json!([{"id":"token", "label":" "}]),
            json!(
                (0..17)
                    .map(|index| json!({"id":format!("token_{index}"), "label":"Token"}))
                    .collect::<Vec<_>>()
            ),
        ] {
            value["credentials"] = declarations;
            assert!(parse(&value).is_err());
        }
    }

    #[test]
    fn settings_require_explicit_integration_and_validate_schema_before_resolution() {
        let dir = TestDir::new();
        let mut value = minimal();
        value["settings"] =
            json!([{"id":"active","label":"Active","type":"toggle","default":true}]);
        let manifest = parse_manifest(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            resolve_pack(&dir.0, manifest)
                .unwrap_err()
                .contains("explicit local integration")
        );
        value["settings"][0]["type"] = json!("password");
        assert!(parse_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
        value["settings"] =
            json!([{"id":"count","label":"Count","type":"integer","min":2,"max":1,"default":1}]);
        assert!(parse_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn integration_opt_in_preserves_relative_metadata_and_shares_content_validation() {
        let (dir, mut value) = integration_fixture();
        dir.write_manifest(&value);
        assert!(load_pack(&dir.0).unwrap_err().contains("content-only"));
        assert!(resolve_pack(&dir.0, parse(&value).unwrap()).is_err());
        let pack = load_integration_pack(&dir.0).unwrap();
        assert_eq!(pack.content.len(), 6);
        assert_eq!(pack.root, dir.0.canonicalize().unwrap());
        let process = pack.manifest.process.unwrap();
        assert_eq!(process.program, "bin/worker");
        assert_eq!(process.working_directory, Some(PathBuf::from("work")));
        assert_eq!(
            process.args,
            ["--literal", "hello world", "$(literal); not shell", ""]
        );
        value["content"]["backgrounds"][0]["source"]["asset"] = json!("assets/missing.png");
        assert!(
            resolve_integration_pack(&dir.0, parse(&value).unwrap())
                .unwrap_err()
                .contains("Content 'wallpaper'")
        );
    }

    #[test]
    fn test_process_is_optional_and_requires_a_normal_executable_integration() {
        let absent = parse(&minimal()).unwrap();
        assert!(absent.test_process.is_none());
        assert!(
            serde_json::to_value(absent)
                .unwrap()
                .get("test_process")
                .is_none()
        );
        let (dir, mut value) = integration_fixture();
        value["test_process"] = json!({
            "program": "bin/worker", "args": ["--test", "literal $(data)"], "working_directory": "work",
        });
        for hooks in [false, true] {
            if hooks {
                let handler = value.as_object_mut().unwrap().remove("process").unwrap();
                value["hooks"] = json!([{"event": "reply.finished", "handler": handler}]);
            }
            dir.write_manifest(&value);
            let pack = load_integration_pack(&dir.0).unwrap();
            let test = pack.manifest.test_process.unwrap();
            assert_eq!(test.program, "bin/worker");
            assert_eq!(test.args, ["--test", "literal $(data)"]);
            assert_eq!(test.working_directory, Some(PathBuf::from("work")));
            assert!(load_pack(&dir.0).unwrap_err().contains("content-only"));
            assert!(
                resolve_pack(&dir.0, parse(&value).unwrap())
                    .unwrap_err()
                    .contains("content-only")
            );
        }
        let mut test_only = minimal();
        test_only["test_process"] = value["test_process"].clone();
        dir.write_manifest(&test_only);
        assert!(load_pack(&dir.0).unwrap_err().contains("content-only"));
        assert!(
            resolve_pack(&dir.0, parse(&test_only).unwrap())
                .unwrap_err()
                .contains("content-only")
        );
        assert!(
            load_integration_pack(&dir.0)
                .unwrap_err()
                .contains("requires a persistent process")
        );
    }

    #[test]
    fn test_process_validates_program_args_and_working_directory() {
        let (dir, mut value) = integration_fixture();
        value["test_process"] = value["process"].clone();
        for (field, bad) in [
            ("program", json!("")),
            ("program", json!("/usr/bin/python3")),
            ("program", json!("../worker")),
            ("program", json!("bin/../bin/worker")),
            ("program", json!("bin/missing")),
            ("program", json!("bin")),
            ("program", json!("x".repeat(MAX_PROCESS_PATH_BYTES + 1))),
            ("working_directory", json!("missing")),
            ("working_directory", json!("../outside")),
            ("working_directory", json!("/tmp")),
            ("working_directory", json!("bin/worker")),
            ("args", json!(vec!["x"; MAX_PROCESS_ARGS + 1])),
            ("args", json!(["x".repeat(MAX_PROCESS_ARG_BYTES + 1)])),
            ("args", json!(vec!["x".repeat(MAX_PROCESS_ARG_BYTES); 4])),
            ("args", json!(["bad\narg"])),
        ] {
            let mut invalid = value.clone();
            invalid["test_process"][field] = bad;
            let error = parse(&invalid)
                .and_then(|manifest| resolve_integration_pack(&dir.0, manifest))
                .unwrap_err();
            assert!(
                error.to_ascii_lowercase().contains("test process"),
                "{error}"
            );
        }
        let mut invalid = value;
        invalid["test_process"] = json!({"program": "bin/worker", "shell": "ignored"});
        assert!(parse(&invalid).unwrap_err().contains("unknown field"));
    }

    #[cfg(unix)]
    #[test]
    fn test_process_rejects_symlink_escapes_and_nonexecutable_programs() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (dir, mut value) = integration_fixture();
        let outside = TestDir::new();
        fs::write(outside.0.join("worker"), b"#!/bin/sh\nexit 0\n").unwrap();
        symlink(outside.0.join("worker"), dir.0.join("escaped-worker")).unwrap();
        symlink(&outside.0, dir.0.join("escaped-work")).unwrap();
        for handler in [
            json!({"program": "escaped-worker"}),
            json!({"program": "bin/worker", "working_directory": "escaped-work"}),
        ] {
            value["test_process"] = handler;
            let error = resolve_integration_pack(&dir.0, parse(&value).unwrap()).unwrap_err();
            assert!(
                error.contains("Test process") && error.contains("escapes"),
                "{error}"
            );
        }
        fs::copy(dir.0.join("bin/worker"), dir.0.join("bin/test-worker")).unwrap();
        fs::set_permissions(
            dir.0.join("bin/test-worker"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        value["test_process"] = json!({"program": "bin/test-worker"});
        let error = resolve_integration_pack(&dir.0, parse(&value).unwrap()).unwrap_err();
        assert!(error.contains("Test process") && error.contains("executable permission"));
    }

    #[test]
    fn integration_requires_executable_rejects_mixed_modes_unknown_and_duplicate_capabilities() {
        let (dir, value) = integration_fixture();
        assert!(
            resolve_integration_pack(&dir.0, parse(&minimal()).unwrap())
                .unwrap_err()
                .contains("requires a persistent process")
        );
        let mut hooks = value.clone();
        hooks["hooks"] = json!([{"event": "app.ready", "handler": {"program": "bin/worker"}}]);
        assert!(
            resolve_integration_pack(&dir.0, parse(&hooks).unwrap())
                .unwrap_err()
                .contains("not both")
        );
        for (caps, error) in [
            (json!(["shell.run"]), "Unknown integration capability"),
            (
                json!(["reply.read", "reply.read"]),
                "Duplicate integration capability",
            ),
            (json!(vec!["reply.read"; MAX_CAPABILITIES + 1]), "At most"),
        ] {
            let mut invalid = value.clone();
            invalid["capabilities"] = caps;
            assert!(
                resolve_integration_pack(&dir.0, parse(&invalid).unwrap())
                    .unwrap_err()
                    .contains(error)
            );
        }
        let mut all = value;
        all["capabilities"] = json!([
            "motion.play",
            "expression.apply",
            "message.deliver",
            "event.emit",
            "catalog.get",
            "lifecycle.observe",
            "reply.read"
        ]);
        assert!(resolve_integration_pack(&dir.0, parse(&all).unwrap()).is_ok());
    }

    #[test]
    fn supported_hooks_load_whole_package_and_remain_rejected_as_content() {
        let (dir, mut value) = integration_fixture();
        let handler = value.as_object_mut().unwrap().remove("process").unwrap();
        for events in [
            vec!["motion.playback.ended"],
            vec!["extension.started"],
            vec!["extension.stopping"],
            vec!["extension.stopped"],
            vec![
                "extension.started",
                "app.ready",
                "character.selected",
                "reply.finished",
            ],
            vec!["app.ready"],
            vec![
                "app.ready",
                "character.selected",
                "reply.finished",
                "emotion.changed",
            ],
            vec!["character.selected"],
            vec!["reply.finished"],
            vec!["emotion.changed"],
            vec!["reply.finished", "speech.started", "speech.stopped"],
            vec![
                "reply.finished",
                "speech.started",
                "speech.stopped",
                "emotion.changed",
            ],
        ] {
            value["hooks"] = json!(
                events
                    .iter()
                    .map(|event| { json!({"event": event, "handler": handler}) })
                    .collect::<Vec<_>>()
            );
            dir.write_manifest(&value);
            let pack = load_integration_pack(&dir.0).unwrap();
            assert!(pack.manifest.process.is_none());
            assert_eq!(pack.manifest.hooks.len(), events.len());
            for (hook, event) in pack.manifest.hooks.iter().zip(events) {
                assert_eq!(hook.event, event);
                assert_eq!(hook.handler.program, "bin/worker");
                assert_eq!(hook.handler.working_directory, Some(PathBuf::from("work")));
            }
            assert_eq!(pack.content.len(), 6);
            assert!(
                load_pack(&dir.0)
                    .unwrap_err()
                    .contains("explicit local integration")
            );
        }
        fs::remove_file(dir.0.join("assets/background.png")).unwrap();
        assert!(
            load_integration_pack(&dir.0)
                .unwrap_err()
                .contains("Content 'wallpaper'")
        );
    }

    #[test]
    fn hooks_reject_duplicate_unknown_and_excess_triggers() {
        let (dir, mut value) = integration_fixture();
        let handler = value.as_object_mut().unwrap().remove("process").unwrap();
        for (events, expected) in [
            (
                vec!["reply.finished", "reply.finished"],
                "Duplicate hook event",
            ),
            (
                vec!["emotion.changed", "emotion.changed"],
                "Duplicate hook event 'emotion.changed'",
            ),
            (
                vec![
                    "reply.finished",
                    "speech.started",
                    "emotion.changed",
                    "emotion.changed",
                ],
                "Duplicate hook event 'emotion.changed'",
            ),
            (vec!["app.stopping"], "Unsupported hook event"),
            (vec!["emotion.change"], "Unsupported hook event"),
            (
                vec![
                    "reply.finished",
                    "speech.started",
                    "speech.stopped",
                    "emotion.changed",
                    "reply.finished",
                ],
                "At most 4",
            ),
        ] {
            value["hooks"] = json!(
                events
                    .into_iter()
                    .map(|event| { json!({"event": event, "handler": handler}) })
                    .collect::<Vec<_>>()
            );
            let error = resolve_integration_pack(&dir.0, parse(&value).unwrap()).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn every_hook_handler_uses_contained_bounded_process_validation() {
        let (dir, mut value) = integration_fixture();
        let handler = value.as_object_mut().unwrap().remove("process").unwrap();
        value["hooks"] = json!([
            {"event":"reply.finished", "handler":handler},
            {"event":"speech.stopped", "handler":handler}
        ]);
        // Put each error in the second handler so validation must cover all hooks.
        for (field, bad) in [
            ("program", json!("/usr/bin/python3")),
            ("program", json!("../worker")),
            ("program", json!("missing")),
            ("program", json!("work")),
            ("working_directory", json!("../outside")),
            ("working_directory", json!("bin/worker")),
            ("args", json!(vec!["x"; MAX_PROCESS_ARGS + 1])),
            ("args", json!(["x".repeat(MAX_PROCESS_ARG_BYTES + 1)])),
            ("args", json!(["line\nbreak"])),
        ] {
            let mut invalid = value.clone();
            invalid["hooks"][1]["handler"][field] = bad;
            let error = resolve_integration_pack(&dir.0, parse(&invalid).unwrap()).unwrap_err();
            assert!(error.contains("Hook 'speech.stopped'"), "{error}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let outside = TestDir::new();
            fs::write(outside.0.join("worker"), b"#!/bin/sh\nexit 0\n").unwrap();
            symlink(outside.0.join("worker"), dir.0.join("escaped-worker")).unwrap();
            let mut invalid = value.clone();
            invalid["hooks"][1]["handler"]["program"] = json!("escaped-worker");
            assert!(resolve_integration_pack(&dir.0, parse(&invalid).unwrap()).is_err());
            symlink(&outside.0, dir.0.join("escaped-work")).unwrap();
            invalid = value.clone();
            invalid["hooks"][1]["handler"]["working_directory"] = json!("escaped-work");
            assert!(resolve_integration_pack(&dir.0, parse(&invalid).unwrap()).is_err());
            fs::set_permissions(dir.0.join("bin/worker"), fs::Permissions::from_mode(0o644))
                .unwrap();
            assert!(
                resolve_integration_pack(&dir.0, parse(&value).unwrap())
                    .unwrap_err()
                    .contains("executable permission")
            );
        }
    }

    #[test]
    fn integration_rejects_program_lookup_traversal_and_nonregular_files() {
        let (dir, value) = integration_fixture();
        for program in [
            "python3",
            "/usr/bin/python3",
            "../worker",
            "bin/../bin/worker",
            "C:/worker",
            "bin\\worker",
            "bin",
            "bin/missing",
        ] {
            let mut invalid = value.clone();
            invalid["process"]["program"] = json!(program);
            assert!(
                resolve_integration_pack(&dir.0, parse(&invalid).unwrap()).is_err(),
                "{program}"
            );
        }
        for working in ["../", "/tmp", "bin/worker", "C:/temp", "missing"] {
            let mut invalid = value.clone();
            invalid["process"]["working_directory"] = json!(working);
            assert!(
                resolve_integration_pack(&dir.0, parse(&invalid).unwrap()).is_err(),
                "{working}"
            );
        }
        for working in [json!(null), json!(""), json!("."), json!("work")] {
            let mut valid = value.clone();
            valid["process"]["working_directory"] = working.clone();
            let pack = resolve_integration_pack(&dir.0, parse(&valid).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(pack.manifest.process.unwrap()).unwrap()["working_directory"],
                working
            );
        }
    }

    #[test]
    fn integration_bounds_direct_manifest_args_and_paths() {
        let (dir, value) = integration_fixture();
        let original = parse(&value).unwrap();
        for args in [
            vec!["x".into(); MAX_PROCESS_ARGS + 1],
            vec!["x".repeat(MAX_PROCESS_ARG_BYTES + 1)],
            vec!["x".repeat(MAX_PROCESS_ARG_BYTES); 4],
            vec!["contains\0nul".into()],
            vec!["contains\nnewline".into()],
        ] {
            let mut invalid = original.clone();
            invalid.process.as_mut().unwrap().args = args;
            assert!(
                resolve_integration_pack(&dir.0, invalid)
                    .unwrap_err()
                    .contains("arguments")
            );
        }
        let mut valid = original.clone();
        valid.process.as_mut().unwrap().args = vec!["x".repeat(MAX_PROCESS_ARG_BYTES - 1); 4];
        assert!(resolve_integration_pack(&dir.0, valid).is_ok());
        let mut invalid = original.clone();
        invalid.process.as_mut().unwrap().program = "a".repeat(MAX_PROCESS_PATH_BYTES + 1);
        assert!(
            resolve_integration_pack(&dir.0, invalid)
                .unwrap_err()
                .contains("path exceeds")
        );
        let mut invalid = original;
        invalid.process.as_mut().unwrap().working_directory =
            Some(PathBuf::from("a".repeat(MAX_PROCESS_PATH_BYTES + 1)));
        assert!(
            resolve_integration_pack(&dir.0, invalid)
                .unwrap_err()
                .contains("exceeds")
        );
    }

    #[cfg(unix)]
    #[test]
    fn integration_rejects_symlink_escapes_and_nonexecutable_programs() {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        let (dir, value) = integration_fixture();
        let outside = TestDir::new();
        fs::write(outside.0.join("worker"), b"outside").unwrap();
        fs::set_permissions(outside.0.join("worker"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink(outside.0.join("worker"), dir.0.join("escape")).unwrap();
        symlink(&outside.0, dir.0.join("escape-dir")).unwrap();
        let mut invalid = value.clone();
        invalid["process"]["program"] = json!("escape");
        assert!(
            resolve_integration_pack(&dir.0, parse(&invalid).unwrap())
                .unwrap_err()
                .contains("escapes package root")
        );
        invalid = value.clone();
        invalid["process"]["working_directory"] = json!("escape-dir");
        assert!(
            resolve_integration_pack(&dir.0, parse(&invalid).unwrap())
                .unwrap_err()
                .contains("escapes package root")
        );
        fs::set_permissions(dir.0.join("bin/worker"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            resolve_integration_pack(&dir.0, parse(&value).unwrap())
                .unwrap_err()
                .contains("executable permission")
        );
    }

    #[test]
    fn resolves_whole_package_with_owned_canonical_paths_and_namespaced_ids() {
        let (dir, value) = fixture();
        dir.write_manifest(&value);
        let pack = load_pack(&dir.0).unwrap();
        assert_eq!(pack.root, dir.0.canonicalize().unwrap());
        assert_eq!(pack.manifest.id, "org.example.starter");
        assert_eq!(pack.content.len(), 6);
        let ids: Vec<_> = pack.content.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "org.example.starter/friend",
                "org.example.starter/idle",
                "org.example.starter/smile",
                "org.example.starter/wallpaper",
                "org.example.starter/room",
                "org.example.starter/portrait"
            ]
        );
        assert_eq!(pack.content[0].kind, ContentKind::Character);
        assert_eq!(
            pack.content[0].path,
            Some(dir.0.join("assets/model.glb").canonicalize().unwrap())
        );
        assert_eq!(
            pack.content[1].path,
            Some(dir.0.join("assets/idle.bvh").canonicalize().unwrap())
        );
        assert_eq!(
            pack.content[3].path,
            Some(dir.0.join("assets/background.png").canonicalize().unwrap())
        );
        assert!(pack.content[2].path.is_none());
        assert!(pack.content[4].path.is_none());
        assert!(pack.content[5].path.is_none());
        assert_eq!(pack.manifest.content.characters[0].greeting, "Hello");
        assert_eq!(pack.manifest.content.motions[0].loopable, Some(true));
    }

    #[test]
    fn rejects_versions_and_identity_before_resolution() {
        for bad in [
            "1",
            "1.2",
            "v1.2.3",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-01",
            "1.2.3-",
            "1.2.3+",
            "1.2.3+a+b",
            "1.2.3-a..b",
            "1.2.3 ",
        ] {
            let mut value = minimal();
            value["version"] = json!(bad);
            assert!(parse(&value).unwrap_err().contains("version"), "{bad}");
        }
        for bad in [
            "starter",
            "Org.example",
            "org..starter",
            "org/example",
            "org.é",
            "org.-starter",
        ] {
            let mut value = minimal();
            value["id"] = json!(bad);
            assert!(parse(&value).unwrap_err().contains("package id"), "{bad}");
        }
        for bad in [0, 2, u32::MAX] {
            let mut value = minimal();
            value["api_version"] = json!(bad);
            assert!(
                parse(&value)
                    .unwrap_err()
                    .contains("Unsupported api_version")
            );
        }
        for bad in [json!("1"), json!(-1), json!(1.5)] {
            let mut value = minimal();
            value["api_version"] = bad;
            assert!(parse(&value).is_err());
        }
        for field in ["name", "description"] {
            let mut value = minimal();
            value[field] = json!(" \n ");
            assert!(parse(&value).unwrap_err().contains("nonempty"));
        }
        for field in ["author", "license"] {
            let mut value = minimal();
            value["provenance"][field] = json!("");
            assert!(parse(&value).unwrap_err().contains("Provenance"));
        }
        let mut value = minimal();
        value["name"] = json!("n".repeat(257));
        assert!(parse(&value).unwrap_err().contains("exceeds 256 bytes"));
        value["name"] = json!("Starter");
        value["version"] = json!(format!("1.2.3+{}", "b".repeat(59)));
        assert!(parse(&value).unwrap_err().contains("version"));
    }

    #[test]
    fn rejects_duplicate_ids_across_categories_and_invalid_local_ids() {
        let (_, mut value) = fixture();
        value["content"]["presets"][0]["id"] = json!("friend");
        assert!(
            parse(&value)
                .unwrap_err()
                .contains("Duplicate local content id 'friend'")
        );
        for bad in [
            "",
            "../friend",
            "other/entry",
            "UPPER",
            "white space",
            "a.b",
        ] {
            value["content"]["presets"][0]["id"] = json!(bad);
            assert!(
                parse(&value)
                    .unwrap_err()
                    .contains("Invalid local content id")
            );
        }
        value["content"]["presets"][0]["id"] = json!("portrait");
        value["content"]["presets"][0]["name"] = json!(" ");
        assert!(parse(&value).unwrap_err().contains("name must be nonempty"));
    }

    #[test]
    fn strict_enums_and_fields_do_not_silently_drop_unsupported_content() {
        let (_, original) = fixture();
        for (pointer, bad) in [
            ("/content/motions/0/role", "dance"),
            ("/content/motions/0/emotion", "joy"),
            ("/content/expressions/0/emotion", "invalid"),
            ("/content/backgrounds/0/source/type", "shader"),
            ("/content/backgrounds/1/source/room", "new_procedural_room"),
        ] {
            let mut value = original.clone();
            // emotion is optional in the fixture; insert it explicitly.
            if pointer.ends_with("motions/0/emotion") {
                value["content"]["motions"][0]["emotion"] = json!(bad);
            } else {
                *value.pointer_mut(pointer).unwrap() = json!(bad);
            }
            assert!(
                parse(&value).unwrap_err().contains("unknown variant"),
                "{pointer}"
            );
        }
        for pointer in [
            "/content",
            "/content/presets/0/settings",
            "/content/backgrounds/0/source",
        ] {
            let mut value = original.clone();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("arbitrary_merge".into(), json!({"provider": "secret"}));
            assert!(parse(&value).unwrap_err().contains("unknown field"));
        }
        let mut value = original;
        value["content"]["motions"][0]["loopable"] = json!("yes");
        assert!(parse(&value).unwrap_err().contains("boolean"));
    }

    #[test]
    fn executable_metadata_is_inspectable_but_never_installable() {
        let dir = TestDir::new();
        for (key, metadata) in [
            (
                "process",
                json!({"program": "python", "args": ["handler.py"]}),
            ),
            (
                "test_process",
                json!({"program": "python", "args": ["test.py"]}),
            ),
            (
                "hooks",
                json!([{"event": "app.ready", "handler": {"program": "node", "args": ["hook.js"]}}]),
            ),
            ("capabilities", json!(["motion.play"])),
        ] {
            let mut value = minimal();
            value[key] = metadata;
            let manifest = parse(&value).unwrap();
            assert!(
                manifest.process.is_some()
                    || manifest.test_process.is_some()
                    || !manifest.hooks.is_empty()
                    || !manifest.capabilities.is_empty()
            );
            dir.write_manifest(&value);
            let error = load_pack(&dir.0).unwrap_err();
            assert!(
                error.contains("unsupported for content-only installation"),
                "{error}"
            );
            assert!(error.contains("explicit local integration"));
        }
        let mut value = minimal();
        value["process"] = json!({"shell": "echo silently ignored"});
        assert!(parse(&value).is_err());
        value.as_object_mut().unwrap().remove("process");
        value["entrypoint"] = json!("run.py");
        assert!(parse(&value).unwrap_err().contains("unknown field"));
    }

    #[test]
    fn validates_finite_bounded_values_even_for_directly_constructed_manifests() {
        let (dir, value) = fixture();
        let original = parse(&value).unwrap();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 1.01] {
            let mut manifest = original.clone();
            manifest.content.expressions[0].strength = bad;
            assert!(
                resolve_pack(&dir.0, manifest)
                    .unwrap_err()
                    .contains("strength must be finite")
            );
            let mut manifest = original.clone();
            manifest.content.presets[0].settings.background = Some([0.0, bad, 1.0]);
            assert!(
                resolve_pack(&dir.0, manifest)
                    .unwrap_err()
                    .contains("background component must be finite")
            );
        }
        let mut value = minimal();
        value["content"]["presets"] = json!([{"id": "empty", "name": "Empty", "settings": {}}]);
        assert!(
            parse(&value)
                .unwrap_err()
                .contains("at least one supported setting")
        );
    }

    #[test]
    fn enforces_manifest_and_total_content_limits_including_read_boundary() {
        let dir = TestDir::new();
        let mut bytes = serde_json::to_vec(&minimal()).unwrap();
        bytes.resize(MAX_MANIFEST_BYTES, b' ');
        assert!(parse_manifest(&bytes).is_ok());
        fs::write(dir.0.join(MANIFEST_FILE), &bytes).unwrap();
        assert!(load_pack(&dir.0).is_ok());
        bytes.push(b' ');
        assert!(parse_manifest(&bytes).unwrap_err().contains("256 KiB"));
        fs::write(dir.0.join(MANIFEST_FILE), bytes).unwrap();
        assert!(load_pack(&dir.0).unwrap_err().contains("256 KiB"));
        let mut value = minimal();
        value["content"]["presets"] = Value::Array(
            (0..MAX_CONTENT_ENTRIES)
                .map(|i| preset(&format!("p{i}")))
                .collect(),
        );
        assert!(parse(&value).is_ok());
        value["content"]["motions"] = json!([motion("missing.bvh")]);
        assert!(
            parse(&value)
                .unwrap_err()
                .contains("512 entries across all categories")
        );
        assert!(
            parse_manifest(b"{not json}")
                .unwrap_err()
                .contains("Invalid extension.json")
        );
    }

    #[test]
    fn rejects_portable_path_escapes_missing_files_directories_and_formats() {
        let (dir, original) = fixture();
        let absolute = dir.0.join("assets/idle.bvh").to_str().unwrap().to_owned();
        for bad in [
            absolute.as_str(),
            "../idle.bvh",
            "assets/../assets/idle.bvh",
            "./assets/idle.bvh",
            "C:/idle.bvh",
            "C:\\idle.bvh",
            "\\\\host\\idle.bvh",
            "assets//idle.bvh",
        ] {
            let mut value = original.clone();
            value["content"]["motions"][0]["asset"] = json!(bad);
            assert!(
                resolve_pack(&dir.0, parse(&value).unwrap())
                    .unwrap_err()
                    .contains("package-relative file path"),
                "{bad}"
            );
        }
        for (bad, message) in [
            ("missing.bvh", "Cannot resolve"),
            ("assets", "regular file"),
            ("assets/script.py", "Unsupported Motion format"),
        ] {
            fs::write(dir.0.join("assets/script.py"), b"print('never run')").unwrap();
            let mut value = original.clone();
            value["content"]["motions"][0]["asset"] = json!(bad);
            assert!(
                resolve_pack(&dir.0, parse(&value).unwrap())
                    .unwrap_err()
                    .contains(message)
            );
        }
        fs::write(dir.0.join("assets/model.gltf"), b"{}").unwrap();
        let mut value = original;
        value["content"]["characters"][0]["asset"] = json!("assets/model.gltf");
        assert!(
            resolve_pack(&dir.0, parse(&value).unwrap())
                .unwrap_err()
                .contains("supported extensions: vrm, glb")
        );
    }

    #[test]
    fn failure_in_last_asset_rejects_the_whole_package() {
        let (dir, mut value) = fixture();
        value["content"]["backgrounds"][0]["source"]["asset"] = json!("assets/missing.png");
        dir.write_manifest(&value);
        let error = load_pack(&dir.0).unwrap_err();
        assert!(error.contains("Content 'wallpaper'"));
        assert!(error.contains("missing.png"));
        // Repairing the last asset makes the same entire package resolvable.
        fs::copy(
            dir.0.join("assets/background.png"),
            dir.0.join("assets/missing.png"),
        )
        .unwrap();
        assert_eq!(load_pack(&dir.0).unwrap().content.len(), 6);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file_directory_and_manifest_escapes() {
        use std::os::unix::fs::symlink;
        let (dir, original) = fixture();
        let outside = TestDir::new();
        fs::write(
            outside.0.join("outside.bvh"),
            b"not read by manifest validator",
        )
        .unwrap();
        symlink(outside.0.join("outside.bvh"), dir.0.join("file.bvh")).unwrap();
        symlink(&outside.0, dir.0.join("external")).unwrap();
        for asset in ["file.bvh", "external/outside.bvh"] {
            let mut value = original.clone();
            value["content"]["motions"][0]["asset"] = json!(asset);
            assert!(
                resolve_pack(&dir.0, parse(&value).unwrap())
                    .unwrap_err()
                    .contains("escapes package root")
            );
        }
        outside.write_manifest(&original);
        symlink(outside.0.join(MANIFEST_FILE), dir.0.join(MANIFEST_FILE)).unwrap();
        assert!(
            load_pack(&dir.0)
                .unwrap_err()
                .contains("escapes package root")
        );
        fs::remove_file(dir.0.join(MANIFEST_FILE)).unwrap();
        symlink(dir.0.join("assets/idle.bvh"), dir.0.join("alias.bvh")).unwrap();
        let mut value = original;
        value["content"]["motions"][0]["asset"] = json!("alias.bvh");
        dir.write_manifest(&value);
        assert_eq!(
            load_pack(&dir.0).unwrap().content[1].path,
            Some(dir.0.join("assets/idle.bvh").canonicalize().unwrap())
        );
    }
}
