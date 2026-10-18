//! Per-character part visibility for the active avatar only. Mesh primitives
//! are discovered once per avatar by walking that avatar's own hierarchy, so
//! unrelated scene meshes are never touched and no frame scans the world.
//! Keys are escaped name paths with an ordinal for duplicate siblings, so a
//! saved choice maps onto the same part when the same model is reloaded.
//!
//! Deliberately excluded: morph channels (expressions.rs and animation clips
//! already own `MorphWeights`; a second writer would corrupt their baselines)
//! and material tint (materials are shared between primitives and the shader
//! preview variants, so tinting one part would recolor others). This hides
//! whole parts the model already has; it is not a clothing or geometry editor.
use crate::loading::AvatarLoadState;
use bevy::{gltf::GltfMaterialName, prelude::*};
use bevy_shader_mtoon::OutlineMaterial;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

pub const MAX_SAVED_PARTS: usize = 4096;
const MAX_KEY_BYTES: usize = 1024;
const MAX_PARTS: usize = 4096;
const MAX_DEPTH: usize = 256;
const MAX_VISITED: usize = 32_768;
const MAX_DISCOVERY_FRAMES: u8 = 120;

/// Saved per character inside `SceneSettings`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Part keys hidden by the user. Keys absent from the current model are
    /// kept, so switching to another avatar and back does not lose choices.
    pub hidden_parts: BTreeSet<String>,
}
impl AppearanceConfig {
    /// Drops empty/oversized keys and bounds the set; use on loaded settings.
    pub fn sanitized(mut self) -> Self {
        self.hidden_parts
            .retain(|key| !key.is_empty() && key.len() <= MAX_KEY_BYTES);
        while self.hidden_parts.len() > MAX_SAVED_PARTS {
            self.hidden_parts.pop_last();
        }
        self
    }
    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden_parts.contains(key)
    }
    pub fn set_hidden(&mut self, key: &str, hidden: bool) {
        if hidden {
            if self.hidden_parts.len() < MAX_SAVED_PARTS && key.len() <= MAX_KEY_BYTES {
                self.hidden_parts.insert(key.to_owned());
            }
        } else {
            self.hidden_parts.remove(key);
        }
    }
    pub fn reset(&mut self) {
        self.hidden_parts.clear();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvatarPart {
    pub key: String,
    /// glTF node name of the part (or its own name when it has no named parent).
    pub label: String,
    pub material: Option<String>,
    /// The model itself ships this part hidden; it stays hidden.
    pub hidden_by_model: bool,
}

#[derive(Resource)]
pub struct AppearanceState {
    pub config: AppearanceConfig,
    pub parts: Vec<AvatarPart>,
    pub status: String,
    /// UI search text; not persisted.
    pub filter: String,
}
impl Default for AppearanceState {
    fn default() -> Self {
        Self {
            config: AppearanceConfig::default(),
            parts: Vec::new(),
            status: "No avatar loaded".into(),
            filter: String::new(),
        }
    }
}
impl AppearanceState {
    /// Saved hidden keys that this model does not have.
    pub fn missing_saved(&self) -> usize {
        self.config
            .hidden_parts
            .iter()
            .filter(|key| !self.parts.iter().any(|p| &p.key == *key))
            .count()
    }
    pub fn forget_missing(&mut self) {
        let parts = &self.parts;
        self.config
            .hidden_parts
            .retain(|key| parts.iter().any(|p| &p.key == key));
    }
    pub fn hidden_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| self.config.is_hidden(&p.key))
            .count()
    }
}

/// Visibility writes precede propagation in PostUpdate.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppearanceSystems;

pub struct AppearancePlugin;
impl Plugin for AppearancePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppearanceState>()
            .init_resource::<AppearanceRuntime>()
            .configure_sets(
                PostUpdate,
                AppearanceSystems
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(PostUpdate, update_appearance.in_set(AppearanceSystems));
    }
}

struct CachedPart {
    entity: Entity,
    key: String,
    original: Visibility,
}
#[derive(Resource, Default)]
struct AppearanceRuntime {
    root: Option<Entity>,
    parts: Vec<CachedPart>,
    applied: BTreeSet<String>,
    discovered: bool,
    wait_frames: u8,
}

fn escape(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for c in segment.chars() {
        if matches!(c, '/' | '#' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

struct Found {
    entity: Entity,
    key: String,
    label: String,
    material: Option<String>,
}

/// Depth-first in `Children` order, which glTF spawning keeps deterministic,
/// so duplicate-name ordinals are stable across reloads of the same file.
fn discover(
    root: Entity,
    children: &Query<&Children>,
    names: &Query<&Name>,
    meshes: &Query<(), (With<Mesh3d>, Without<MeshMaterial3d<OutlineMaterial>>)>,
    outlines: &Query<(), With<MeshMaterial3d<OutlineMaterial>>>,
    materials: &Query<&GltfMaterialName>,
) -> Vec<Found> {
    let mut found = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    // (entity, parent path, parent display name, depth)
    let mut stack: Vec<(Entity, String, Option<String>, usize)> = Vec::new();
    let push_children = |stack: &mut Vec<(Entity, String, Option<String>, usize)>,
                         entity: Entity,
                         path: &str,
                         name: Option<String>,
                         depth: usize| {
        if let Ok(list) = children.get(entity) {
            for child in list
                .iter()
                .rev()
                .take(MAX_VISITED.saturating_sub(stack.len()))
            {
                if !outlines.contains(child) {
                    stack.push((child, path.to_owned(), name.clone(), depth));
                }
            }
        }
    };
    push_children(&mut stack, root, "", None, 1);
    let mut visited = 0;
    while let Some((entity, parent_path, parent_name, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_VISITED {
            break;
        }
        if depth > MAX_DEPTH || found.len() >= MAX_PARTS {
            continue;
        }
        let name = names.get(entity).ok().map(|n| n.as_str().to_owned());
        let base = format!(
            "{parent_path}/{}",
            escape(name.as_deref().filter(|n| !n.is_empty()).unwrap_or("?"))
        );
        let ordinal = seen.entry(base.clone()).or_default();
        let path = if *ordinal == 0 {
            base.clone()
        } else {
            format!("{base}#{ordinal}")
        };
        *ordinal += 1;
        if meshes.contains(entity) && path.len() <= MAX_KEY_BYTES {
            found.push(Found {
                entity,
                key: path.clone(),
                label: parent_name
                    .clone()
                    .or_else(|| name.clone())
                    .unwrap_or_else(|| "Unnamed part".into()),
                material: materials.get(entity).ok().map(|m| m.0.clone()),
            });
        }
        push_children(&mut stack, entity, &path, name, depth + 1);
    }
    found
}

fn desired(part: &CachedPart, hidden: &BTreeSet<String>) -> Visibility {
    if hidden.contains(&part.key) {
        Visibility::Hidden
    } else {
        part.original
    }
}

// Bevy injects these independent resources/queries as system parameters.
#[allow(clippy::too_many_arguments)]
fn update_appearance(
    avatar: Res<AvatarLoadState>,
    mut state: ResMut<AppearanceState>,
    mut runtime: ResMut<AppearanceRuntime>,
    children: Query<&Children>,
    names: Query<&Name>,
    meshes: Query<(), (With<Mesh3d>, Without<MeshMaterial3d<OutlineMaterial>>)>,
    outlines: Query<(), With<MeshMaterial3d<OutlineMaterial>>>,
    materials: Query<&GltfMaterialName>,
    mut visibility: Query<&mut Visibility>,
) {
    if runtime.root != avatar.active {
        // A retained old root gets its original visibility back; despawned
        // entities simply fail the lookup and are forgotten.
        for part in &runtime.parts {
            if runtime.applied.contains(&part.key)
                && let Ok(mut v) = visibility.get_mut(part.entity)
            {
                *v = part.original;
            }
        }
        *runtime = AppearanceRuntime {
            root: avatar.active,
            ..default()
        };
        state.parts.clear();
        state.status = if avatar.active.is_some() {
            "Discovering avatar parts…".into()
        } else {
            "No avatar loaded".into()
        };
    }
    let Some(root) = runtime.root else {
        return;
    };
    let mut force = false;
    if !runtime.discovered && runtime.wait_frames < MAX_DISCOVERY_FRAMES {
        runtime.wait_frames += 1;
        let found = discover(root, &children, &names, &meshes, &outlines, &materials);
        if !found.is_empty() {
            runtime.discovered = true;
            force = true;
            for f in found {
                let Ok(original) = visibility.get(f.entity).copied() else {
                    continue;
                };
                state.parts.push(AvatarPart {
                    key: f.key.clone(),
                    label: f.label,
                    material: f.material,
                    hidden_by_model: original == Visibility::Hidden,
                });
                runtime.parts.push(CachedPart {
                    entity: f.entity,
                    key: f.key,
                    original,
                });
            }
        } else if runtime.wait_frames >= MAX_DISCOVERY_FRAMES {
            state.status = "This model has no separate mesh parts".into();
        }
    }
    if !runtime.discovered || (!force && runtime.applied == state.config.hidden_parts) {
        return;
    }
    for part in &runtime.parts {
        let want = desired(part, &state.config.hidden_parts);
        if let Ok(mut v) = visibility.get_mut(part.entity)
            && *v != want
        {
            *v = want;
        }
    }
    runtime.applied = state.config.hidden_parts.clone();
    let missing = state.missing_saved();
    state.status = format!(
        "{} parts · {} hidden{}",
        state.parts.len(),
        state.hidden_count(),
        if missing > 0 {
            format!(" · {missing} saved choices not in this model")
        } else {
            String::new()
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(AppearancePlugin);
        app
    }
    fn spawn_part(app: &mut App, parent: Entity, node: &str, vis: Visibility) -> Entity {
        let node_entity = app
            .world_mut()
            .spawn((ChildOf(parent), Name::new(node.to_owned())))
            .id();
        app.world_mut()
            .spawn((
                ChildOf(node_entity),
                Name::new(format!("{node}.0")),
                Mesh3d(Handle::default()),
                vis,
            ))
            .id()
    }
    fn avatar(app: &mut App) -> (Entity, Entity, Entity) {
        let (root, _, body, hat) = avatar_with_scene(app);
        (root, body, hat)
    }
    fn avatar_with_scene(app: &mut App) -> (Entity, Entity, Entity, Entity) {
        let root = app.world_mut().spawn(Name::new("VRM avatar")).id();
        let scene = app
            .world_mut()
            .spawn((ChildOf(root), Name::new("Scene")))
            .id();
        let body = spawn_part(app, scene, "Body", Visibility::Inherited);
        let hat = spawn_part(app, scene, "Hat", Visibility::Visible);
        (root, scene, body, hat)
    }
    fn vis(app: &App, e: Entity) -> Visibility {
        *app.world().get::<Visibility>(e).unwrap()
    }
    fn key_of(app: &App, label: &str) -> String {
        let state = app.world().resource::<AppearanceState>();
        state
            .parts
            .iter()
            .find(|p| p.label == label)
            .unwrap()
            .key
            .clone()
    }
    fn hide(app: &mut App, key: &str, hidden: bool) {
        app.world_mut()
            .resource_mut::<AppearanceState>()
            .config
            .set_hidden(key, hidden);
    }

    #[test]
    fn only_active_avatar_parts_are_hidden_and_outlines_are_not_parts() {
        let mut app = app();
        let (root, _, hat) = avatar(&mut app);
        let (_, _, other_hat) = avatar(&mut app);
        let stage = app.world_mut().spawn(Name::new("Scene")).id();
        let stage_hat = spawn_part(&mut app, stage, "Hat", Visibility::Inherited);
        let outline = app
            .world_mut()
            .spawn((
                ChildOf(hat),
                Mesh3d(Handle::default()),
                MeshMaterial3d::<OutlineMaterial>(Handle::default()),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        assert_eq!(app.world().resource::<AppearanceState>().parts.len(), 2);
        let key = key_of(&app, "Hat");
        assert_eq!(key, "/Scene/Hat/Hat.0");
        hide(&mut app, &key, true);
        app.update();
        assert_eq!(vis(&app, hat), Visibility::Hidden);
        assert_eq!(vis(&app, other_hat), Visibility::Visible);
        assert_eq!(vis(&app, stage_hat), Visibility::Inherited);
        assert_eq!(vis(&app, outline), Visibility::Inherited);
    }

    #[test]
    fn showing_and_reset_restore_original_visibility_and_model_hidden_stays() {
        let mut app = app();
        let (root, scene, body, hat) = avatar_with_scene(&mut app);
        let secret = spawn_part(&mut app, scene, "Secret", Visibility::Hidden);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        assert!(
            app.world()
                .resource::<AppearanceState>()
                .parts
                .iter()
                .find(|p| p.label == "Secret")
                .unwrap()
                .hidden_by_model
        );
        let (b, h) = (key_of(&app, "Body"), key_of(&app, "Hat"));
        hide(&mut app, &b, true);
        hide(&mut app, &h, true);
        app.update();
        assert_eq!(
            (vis(&app, body), vis(&app, hat)),
            (Visibility::Hidden, Visibility::Hidden)
        );
        hide(&mut app, &h, false);
        app.update();
        assert_eq!(vis(&app, hat), Visibility::Visible);
        app.world_mut()
            .resource_mut::<AppearanceState>()
            .config
            .reset();
        app.update();
        assert_eq!(vis(&app, body), Visibility::Inherited);
        assert_eq!(vis(&app, secret), Visibility::Hidden);
        // Unloading restores a retained root's parts and clears the list.
        hide(&mut app, &h, true);
        app.update();
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(vis(&app, hat), Visibility::Visible);
        assert!(app.world().resource::<AppearanceState>().parts.is_empty());
    }

    #[test]
    fn saved_keys_remap_onto_reloaded_model_and_unknown_keys_are_kept() {
        let mut app = app();
        {
            let mut state = app.world_mut().resource_mut::<AppearanceState>();
            state.config.set_hidden("/Scene/Hat/Hat.0", true);
            state.config.set_hidden("/Other/Model.0", true);
        }
        let (first, _, first_hat) = avatar(&mut app);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(first);
        app.update();
        assert_eq!(vis(&app, first_hat), Visibility::Hidden);
        assert_eq!(app.world().resource::<AppearanceState>().missing_saved(), 1);
        let (second, _, second_hat) = avatar(&mut app);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(second);
        app.world_mut().despawn(first);
        app.update();
        assert_eq!(vis(&app, second_hat), Visibility::Hidden);
        let mut state = app.world_mut().resource_mut::<AppearanceState>();
        assert_eq!(state.config.hidden_parts.len(), 2);
        state.forget_missing();
        assert_eq!(
            state.config.hidden_parts.iter().collect::<Vec<_>>(),
            vec!["/Scene/Hat/Hat.0"]
        );
    }

    #[test]
    fn same_named_siblings_get_distinct_stable_keys() {
        let mut app = app();
        let root = app.world_mut().spawn_empty().id();
        let a = spawn_part(&mut app, root, "Hair/Back", Visibility::Inherited);
        let b = spawn_part(&mut app, root, "Hair/Back", Visibility::Inherited);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        let keys: Vec<_> = app
            .world()
            .resource::<AppearanceState>()
            .parts
            .iter()
            .map(|p| p.key.clone())
            .collect();
        assert_eq!(
            keys,
            vec!["/Hair\\/Back/Hair\\/Back.0", "/Hair\\/Back#1/Hair\\/Back.0"]
        );
        hide(&mut app, &keys[1], true);
        app.update();
        assert_eq!(
            (vis(&app, a), vis(&app, b)),
            (Visibility::Inherited, Visibility::Hidden)
        );
    }

    #[test]
    fn stale_root_is_dropped_and_discovery_is_bounded_and_one_shot() {
        let mut app = app();
        let (root, _, _) = avatar(&mut app);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        let key = key_of(&app, "Hat");
        hide(&mut app, &key, true);
        app.update();
        // Parts added after discovery are not picked up: no per-frame scan.
        let scene = app.world().get::<Children>(root).unwrap()[0];
        spawn_part(&mut app, scene, "Late", Visibility::Inherited);
        app.update();
        assert_eq!(app.world().resource::<AppearanceState>().parts.len(), 2);
        app.world_mut().despawn(root);
        let empty = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(empty);
        for _ in 0..MAX_DISCOVERY_FRAMES as usize + 5 {
            app.update();
        }
        let runtime = app.world().resource::<AppearanceRuntime>();
        assert!(runtime.parts.is_empty());
        assert_eq!(runtime.wait_frames, MAX_DISCOVERY_FRAMES);
        let state = app.world().resource::<AppearanceState>();
        assert!(state.parts.is_empty());
        assert_eq!(state.status, "This model has no separate mesh parts");
        assert!(state.config.is_hidden(&key));
    }

    #[test]
    fn config_sanitizes_and_round_trips() {
        let mut config = AppearanceConfig::default();
        config.hidden_parts.insert(String::new());
        config.hidden_parts.insert("x".repeat(MAX_KEY_BYTES + 1));
        config.hidden_parts.insert("/Scene/Hat/Hat.0".into());
        let config = config.sanitized();
        assert_eq!(config.hidden_parts.len(), 1);
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<AppearanceConfig>(&json).unwrap(),
            config
        );
        assert_eq!(
            serde_json::from_str::<AppearanceConfig>("{}").unwrap(),
            AppearanceConfig::default()
        );
        assert_eq!(escape("a/b#c\\"), "a\\/b\\#c\\\\");
    }
}
