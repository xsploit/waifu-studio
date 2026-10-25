//! Reversible chest-only overrides for actual VRM spring joints.
//!
//! Register `SpringPhysicsPlugin` alongside the vendor's SpringBonePlugin; sync
//! the selected character's SceneSettings config into SpringPhysicsState in
//! Update. Disabling this override never disables the vendor simulation.
use crate::loading::AvatarLoadState;
use bevy::prelude::*;
use bevy_vrm::spring_bones::{SpringBones, SpringSettings};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeSet, HashMap};

const MAX_SELECTION: usize = 256;
const MAX_KEY_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointSelection {
    #[default]
    Detected,
    /// Only the explicitly selected keys, including ambiguously named helpers.
    Manual,
}

/// Saved by SceneSettings, and thus by its existing per-character snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpringPhysicsConfig {
    /// Enables tuning, not the vendor simulation. False restores authored values.
    pub enabled: bool,
    /// 0 = authored, 0.65 = expressive, 1 = maximum supported jiggle tuning.
    #[serde(deserialize_with = "read_strength", serialize_with = "write_strength")]
    pub strength: f32,
    pub selection: JointSelection,
    /// Model-local keys exposed by available_joints, never runtime entity IDs.
    #[serde(deserialize_with = "read_selection")]
    pub manual_joints: BTreeSet<String>,
}

impl Default for SpringPhysicsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 0.0,
            selection: JointSelection::Detected,
            manual_joints: BTreeSet::new(),
        }
    }
}

impl SpringPhysicsConfig {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Bounds direct UI/programmatic edits too; serialized invalid numbers fail.
    pub fn sanitize(&mut self) {
        self.strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.manual_joints
            .retain(|key| !key.is_empty() && key.len() <= MAX_KEY_BYTES);
        while self.manual_joints.len() > MAX_SELECTION {
            self.manual_joints.pop_last();
        }
    }
}

fn read_strength<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = f32::deserialize(deserializer)?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "spring strength must be finite and in 0..=1",
        ))
    }
}

fn write_strength<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_finite() && (0.0..=1.0).contains(value) {
        value.serialize(serializer)
    } else {
        Err(serde::ser::Error::custom(
            "spring strength must be finite and in 0..=1",
        ))
    }
}

fn read_selection<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeSet<String>, D::Error> {
    let keys = BTreeSet::<String>::deserialize(deserializer)?;
    if keys.len() > MAX_SELECTION
        || keys
            .iter()
            .any(|key| key.is_empty() || key.len() > MAX_KEY_BYTES)
    {
        return Err(serde::de::Error::custom(
            "invalid or oversized spring joint selection",
        ));
    }
    Ok(keys)
}

#[derive(Clone, Debug)]
pub struct SpringJointInfo {
    pub key: String,
    pub name: String,
    pub bone: Entity,
    pub chain_index: usize,
    pub joint_index: usize,
    pub detected: bool,
    pub selected: bool,
    pub tuned: bool,
    /// None means the UI may select this joint manually.
    pub blocked_reason: Option<&'static str>,
    pub authored: SpringSettings,
}

#[derive(Resource)]
pub struct SpringPhysicsState {
    pub config: SpringPhysicsConfig,
    pub available_joints: Vec<SpringJointInfo>,
    pub status: String,
}

impl Default for SpringPhysicsState {
    fn default() -> Self {
        Self {
            config: SpringPhysicsConfig::default(),
            available_joints: Vec::new(),
            status: "No avatar loaded".into(),
        }
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpringPhysicsSystems;

pub struct SpringPhysicsPlugin;
impl Plugin for SpringPhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpringPhysicsState>()
            .init_resource::<SpringRuntime>()
            .add_systems(
                PostUpdate,
                update_springs
                    .in_set(SpringPhysicsSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct JointId {
    bone: Entity,
    tail: Entity,
    // A bone may be referenced by several vendor chains with different settings.
    occurrence: usize,
}

struct Baseline {
    authored: SpringSettings,
    tuned: bool,
    last_applied: SpringSettings,
}

struct TrackedJoint {
    id: JointId,
    present: bool,
}

#[derive(Resource, Default)]
struct SpringRuntime {
    // The selected loader wrapper and the vendor's nested scene root differ.
    root: Option<Entity>,
    spring_root: Option<Entity>,
    ambiguous_roots: bool,
    baselines: HashMap<JointId, Baseline>,
    tracked: Vec<TrackedJoint>,
    occurrences: HashMap<(Entity, Entity), usize>,
    config: Option<SpringPhysicsConfig>,
    had_springs: bool,
    #[cfg(test)]
    refreshes: usize,
    #[cfg(test)]
    applications: usize,
}

fn joint_id(
    bone: Entity,
    tail: Entity,
    occurrences: &mut HashMap<(Entity, Entity), usize>,
) -> JointId {
    let occurrence = occurrences.entry((bone, tail)).or_default();
    let id = JointId {
        bone,
        tail,
        occurrence: *occurrence,
    };
    *occurrence += 1;
    id
}

impl SpringRuntime {
    fn restore(&mut self, springs: &mut SpringBones) -> bool {
        self.occurrences.clear();
        let mut changed = false;
        for joint in springs.0.iter_mut().flat_map(|chain| &mut chain.joints) {
            let id = joint_id(joint.bone, joint.tail, &mut self.occurrences);
            if let Some(baseline) = self.baselines.get_mut(&id)
                && baseline.tuned
            {
                if joint.settings != baseline.authored {
                    joint.settings = baseline.authored;
                    changed = true;
                }
                baseline.tuned = false;
            }
        }
        changed
    }
}

/// Token boundaries avoid matching "robust" or "bustle" as chest helpers.
fn classify(name: &str) -> (bool, bool) {
    let mut detected = name.contains('胸');
    let mut excluded = name.contains(['髪', '裙', '衣']) || name.contains("スカート");
    let mut check = |token: &str| {
        detected |= ["bust", "breast", "mune", "boob"]
            .iter()
            .any(|word| token.eq_ignore_ascii_case(word));
        excluded |= [
            "hair",
            "bangs",
            "skirt",
            "dress",
            "cloth",
            "clothes",
            "ribbon",
            "cape",
            "coat",
            "sleeve",
            "accessory",
        ]
        .iter()
        .any(|word| token.eq_ignore_ascii_case(word));
    };
    let mut chars = name.char_indices().peekable();
    let mut previous: Option<char> = None;
    let mut start = 0;
    while let Some((index, c)) = chars.next() {
        let boundary = !c.is_alphabetic()
            || (c.is_uppercase()
                && previous.is_some_and(|prev| {
                    prev.is_lowercase()
                        || (prev.is_uppercase()
                            && chars.peek().is_some_and(|(_, next)| next.is_lowercase()))
                }));
        if boundary {
            if start < index {
                check(&name[start..index]);
            }
            start = if c.is_alphabetic() {
                index
            } else {
                index + c.len_utf8()
            };
        }
        previous = Some(c);
    }
    if start < name.len() {
        check(&name[start..]);
    }
    (detected && !excluded, excluded)
}

fn usable(settings: SpringSettings) -> bool {
    settings.stiffness.is_finite()
        && settings.stiffness >= 0.0
        && settings.drag_force.is_finite()
        && (0.0..=1.0).contains(&settings.drag_force)
        && settings.gravity_power.is_finite()
        && settings.gravity_power >= 0.0
        && settings.gravity_dir.is_finite()
        && settings.hit_radius.is_finite()
        && settings.hit_radius >= 0.0
}

fn tune(authored: SpringSettings, strength: f32) -> SpringSettings {
    SpringSettings {
        stiffness: (authored.stiffness * (1.0 - 0.80 * strength)).max(0.02 * strength),
        drag_force: (authored.drag_force * (1.0 - 0.85 * strength)).max(0.02 * strength),
        ..authored
    }
}

fn update_springs(
    avatar: Res<AvatarLoadState>,
    mut state: ResMut<SpringPhysicsState>,
    mut runtime: ResMut<SpringRuntime>,
    mut springs: Query<&mut SpringBones>,
    spring_roots: Query<Entity, With<SpringBones>>,
    parents: Query<&ChildOf>,
    names: Query<Option<&Name>>,
) {
    let root_changed = runtime.root != avatar.active;
    // Inspect only SpringBones owners, not every node/joint in the scene. This
    // allocation-free lookup also notices late scene insertion/reparenting.
    let (spring_root, ambiguous_roots) = if let Some(root) = avatar.active {
        let mut candidates = spring_roots.iter().filter(|&entity| {
            entity == root || parents.iter_ancestors(entity).any(|parent| parent == root)
        });
        let first = candidates.next();
        let ambiguous = candidates.next().is_some();
        (if ambiguous { None } else { first }, ambiguous)
    } else {
        (None, false)
    };
    let source_changed =
        runtime.spring_root != spring_root || runtime.ambiguous_roots != ambiguous_roots;
    let config_changed = runtime.config.as_ref() != Some(&state.config);
    if config_changed {
        state.config.sanitize();
        runtime.config = Some(state.config.clone());
    }
    if root_changed || source_changed {
        if let Some(root) = runtime.spring_root
            && let Ok(mut old) = springs.get_mut(root)
            && runtime.restore(old.bypass_change_detection())
        {
            old.set_changed();
        }
        runtime.baselines.clear();
        runtime.tracked.clear();
        runtime.had_springs = false;
        runtime.root = avatar.active;
        runtime.spring_root = spring_root;
        runtime.ambiguous_roots = ambiguous_roots;
    }
    if avatar.active.is_none() {
        if root_changed {
            state.available_joints.clear();
            state.status = "No avatar loaded".into();
        }
        return;
    }
    let Some(spring_root) = spring_root else {
        if root_changed || source_changed || config_changed {
            state.available_joints.clear();
            runtime.tracked.clear();
            runtime.had_springs = false;
            state.status = if ambiguous_roots {
                "Multiple VRM spring roots on the active avatar (tuning unsupported; authored physics retained)".into()
            } else {
                "No VRM spring joints on the active avatar (unsupported or still loading)".into()
            };
        }
        return;
    };
    let Ok(mut springs) = springs.get_mut(spring_root) else {
        return;
    };
    let metadata_changed = root_changed
        || source_changed
        || !runtime.had_springs
        || springs.is_changed()
        || runtime.tracked.len() != state.available_joints.len()
        || runtime
            .tracked
            .iter()
            .zip(&state.available_joints)
            .any(|(tracked, info)| {
                let name = names.get(tracked.id.bone);
                let present = name.is_ok() && names.get(tracked.id.tail).is_ok();
                present != tracked.present
                    || name.ok().flatten().map_or("(unnamed)", Name::as_str) != info.name
            });
    if !config_changed && !metadata_changed {
        return;
    }
    if metadata_changed {
        refresh_metadata(&springs, &mut state, &mut runtime, &names);
    }
    runtime.had_springs = true;
    // Merely taking Mut<T> does not mark a component changed. Write through the
    // bypass and emit change detection only when a parameter actually changes.
    if apply_tuning(springs.bypass_change_detection(), &mut state, &mut runtime) {
        springs.set_changed();
    }
    update_status(&mut state);
}

fn refresh_metadata(
    springs: &SpringBones,
    state: &mut SpringPhysicsState,
    runtime: &mut SpringRuntime,
    names: &Query<Option<&Name>>,
) {
    #[cfg(test)]
    {
        runtime.refreshes += 1;
    }
    state.available_joints.clear();
    runtime.tracked.clear();
    runtime.occurrences.clear();
    for (chain_index, chain) in springs.0.iter().enumerate() {
        for (joint_index, joint) in chain.joints.iter().enumerate() {
            let id = joint_id(joint.bone, joint.tail, &mut runtime.occurrences);
            let baseline = runtime.baselines.entry(id).or_insert(Baseline {
                authored: joint.settings,
                tuned: false,
                last_applied: joint.settings,
            });
            // A source replacement/external edit is authoritative when it
            // differs from our last output. Collider-only edits keep baselines.
            if !baseline.tuned || joint.settings != baseline.last_applied {
                baseline.authored = joint.settings;
                baseline.tuned = false;
            }
            let name = names
                .get(joint.bone)
                .ok()
                .flatten()
                .map_or("(unnamed)", Name::as_str);
            // Stable for reloads of the same model's chain/joint layout; names
            // prevent a stale key silently selecting a differently named joint.
            let key = format!("{chain_index}/{joint_index}/{name}");
            let (detected, excluded) = classify(name);
            let present = names.get(joint.bone).is_ok() && names.get(joint.tail).is_ok();
            let blocked_reason = if excluded {
                Some("Hair/clothing spring; excluded from chest tuning")
            } else if !present {
                Some("Spring references a missing bone or tail")
            } else if !usable(baseline.authored) {
                Some("Invalid authored spring settings; left unchanged")
            } else if key.len() > MAX_KEY_BYTES {
                Some("Joint key exceeds the saved selection limit")
            } else {
                None
            };
            runtime.tracked.push(TrackedJoint { id, present });
            state.available_joints.push(SpringJointInfo {
                key,
                name: name.to_owned(),
                bone: joint.bone,
                chain_index,
                joint_index,
                detected,
                selected: false,
                tuned: false,
                blocked_reason,
                authored: baseline.authored,
            });
        }
    }
    runtime.baselines.retain(|id, _| {
        runtime
            .occurrences
            .get(&(id.bone, id.tail))
            .is_some_and(|count| id.occurrence < *count)
    });
}

fn apply_tuning(
    springs: &mut SpringBones,
    state: &mut SpringPhysicsState,
    runtime: &mut SpringRuntime,
) -> bool {
    #[cfg(test)]
    {
        runtime.applications += 1;
    }
    let mut changed = false;
    for (tracked, info) in runtime.tracked.iter().zip(&mut state.available_joints) {
        let baseline = runtime
            .baselines
            .get_mut(&tracked.id)
            .expect("tracked spring baseline");
        let joint = &mut springs.0[info.chain_index].joints[info.joint_index];
        info.selected = info.blocked_reason.is_none()
            && match state.config.selection {
                JointSelection::Detected => info.detected,
                JointSelection::Manual => state.config.manual_joints.contains(&info.key),
            };
        info.tuned = info.selected && state.config.enabled && state.config.strength > 0.0;
        let desired = if info.tuned {
            Some(tune(baseline.authored, state.config.strength))
        } else if baseline.tuned {
            Some(baseline.authored)
        } else {
            None
        };
        if let Some(desired) = desired {
            if joint.settings != desired {
                joint.settings = desired;
                changed = true;
            }
            baseline.last_applied = desired;
        }
        baseline.tuned = info.tuned;
    }
    changed
}

fn update_status(state: &mut SpringPhysicsState) {
    let selected = state
        .available_joints
        .iter()
        .filter(|joint| joint.selected)
        .count();
    state.status = if state.available_joints.is_empty() {
        "No VRM spring joints on the active avatar (unsupported)".into()
    } else if selected == 0 {
        "No eligible chest springs selected; choose ambiguous joints manually (authored physics retained)".into()
    } else if !state.config.enabled || state.config.strength == 0.0 {
        format!("Authored spring physics retained on {selected} selected chest joint(s)")
    } else {
        format!(
            "Chest tuning active on {selected} joint(s); authored gravity and collisions retained"
        )
    };
    if state.config.selection == JointSelection::Manual {
        let missing = state
            .config
            .manual_joints
            .iter()
            .filter(|key| {
                !state
                    .available_joints
                    .iter()
                    .any(|joint| &joint.key == *key)
            })
            .count();
        if missing > 0 {
            state
                .status
                .push_str(&format!("; {missing} saved joint selection(s) unavailable"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use bevy_vrm::spring_bones::{
        SpringBoneLogicState, SpringBonePlugin, SpringChain, SpringCollider, SpringColliderShape,
        SpringJoint,
    };
    use std::time::Duration;

    fn authored(index: usize) -> SpringSettings {
        SpringSettings {
            stiffness: 0.8 + index as f32 * 0.2,
            drag_force: 0.4,
            gravity_power: 0.08,
            gravity_dir: Vec3::X,
            hit_radius: 0.01,
        }
    }

    fn add_rig(app: &mut App, names: &[&str]) -> (Entity, Vec<Entity>) {
        let root = app.world_mut().spawn(Transform::default()).id();
        let chest = app
            .world_mut()
            .spawn((
                Name::new("Chest"),
                serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName::Chest,
                Transform::from_xyz(0.0, 1.2, 0.0),
                ChildOf(root),
            ))
            .id();
        let mut joints = Vec::new();
        let mut bones = Vec::new();
        for (index, &name) in names.iter().enumerate() {
            let bone = app
                .world_mut()
                .spawn((
                    Name::new(name.to_owned()),
                    Transform::from_xyz(index as f32 * 0.3, 0.0, 0.0),
                    ChildOf(chest),
                ))
                .id();
            let tail = app
                .world_mut()
                .spawn((Transform::from_xyz(0.0, -0.2, 0.0), ChildOf(bone)))
                .id();
            joints.push(SpringJoint {
                bone,
                tail,
                settings: authored(index),
            });
            bones.push(bone);
        }
        app.world_mut()
            .entity_mut(root)
            .insert(SpringBones(vec![SpringChain {
                joints,
                colliders: vec![
                    SpringCollider {
                        entity: root,
                        shape: SpringColliderShape::Sphere {
                            offset: Vec3::new(0.0, 0.98, 0.0),
                            radius: 0.05,
                        },
                    },
                    SpringCollider {
                        entity: root,
                        shape: SpringColliderShape::Capsule {
                            offset: Vec3::new(-0.1, 0.9, 0.0),
                            tail: Vec3::new(0.1, 0.9, 0.0),
                            radius: 0.03,
                        },
                    },
                ],
            }]));
        (root, bones)
    }

    fn fixture() -> (App, Entity, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            SpringBonePlugin,
            SpringPhysicsPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .init_resource::<AvatarLoadState>();
        let (root, bones) = add_rig(
            &mut app,
            &["J_Sec_L_Bust1", "Hair_01", "Helper_02", "Skirt_01"],
        );
        // A named mesh/ordinary bone is not enough: actual spring entries only.
        app.world_mut()
            .spawn((Name::new("Breast_R"), Transform::default(), ChildOf(root)));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        (app, root, bones)
    }

    fn settings(app: &App, root: Entity) -> Vec<SpringSettings> {
        app.world()
            .get::<SpringBones>(root)
            .unwrap()
            .0
            .iter()
            .flat_map(|chain| chain.joints.iter().map(|joint| joint.settings))
            .collect()
    }

    #[derive(Resource, Default)]
    struct SpringChanges(usize);

    fn observe_spring_changes(
        springs: Query<Entity, Changed<SpringBones>>,
        mut changes: ResMut<SpringChanges>,
    ) {
        changes.0 += springs.iter().count();
    }

    #[test]
    fn unchanged_frames_and_identical_config_sync_skip_discovery_tuning_and_change_ticks() {
        let (mut app, root, _) = fixture();
        app.init_resource::<SpringChanges>()
            .add_systems(Last, observe_spring_changes);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert!(app.world().resource::<SpringChanges>().0 > 0);
        app.world_mut().resource_mut::<SpringChanges>().0 = 0;
        let runtime = app.world().resource::<SpringRuntime>();
        let counts = (runtime.refreshes, runtime.applications);
        let state = app.world().resource::<SpringPhysicsState>();
        let config = state.config.clone();
        let storage = (
            state.available_joints.as_ptr(),
            state.available_joints[0].name.as_ptr(),
            state.available_joints[0].key.as_ptr(),
            state.status.as_ptr(),
        );
        let tuned = settings(&app, root);
        for _ in 0..20 {
            // Integration may assign the same scene config every Update.
            app.world_mut().resource_mut::<SpringPhysicsState>().config = config.clone();
            app.update();
        }
        assert_eq!(app.world().resource::<SpringChanges>().0, 0);
        let runtime = app.world().resource::<SpringRuntime>();
        assert_eq!((runtime.refreshes, runtime.applications), counts);
        let state = app.world().resource::<SpringPhysicsState>();
        assert_eq!(
            storage,
            (
                state.available_joints.as_ptr(),
                state.available_joints[0].name.as_ptr(),
                state.available_joints[0].key.as_ptr(),
                state.status.as_ptr(),
            )
        );
        assert_eq!(settings(&app, root), tuned);

        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 0.65;
        app.update();
        let runtime = app.world().resource::<SpringRuntime>();
        assert_eq!(runtime.refreshes, counts.0);
        assert_eq!(runtime.applications, counts.1 + 1);
        assert_eq!(app.world().resource::<SpringChanges>().0, 1);
        assert_eq!(settings(&app, root)[0], tune(authored(0), 0.65));
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .reset();
        app.update();
        assert_eq!(app.world().resource::<SpringRuntime>().refreshes, counts.0);
        assert_eq!(settings(&app, root)[0], authored(0));
    }

    #[test]
    fn late_and_replaced_joint_names_invalidate_cached_selection_and_restore_authored_values() {
        let (mut app, root, bones) = fixture();
        app.world_mut().entity_mut(bones[0]).remove::<Name>();
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert_eq!(settings(&app, root)[0], authored(0));
        assert_eq!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints[0]
                .name,
            "(unnamed)"
        );
        let refreshes = app.world().resource::<SpringRuntime>().refreshes;
        app.world_mut()
            .entity_mut(bones[0])
            .insert(Name::new("Breast_L"));
        app.update();
        assert_eq!(
            app.world().resource::<SpringRuntime>().refreshes,
            refreshes + 1
        );
        assert_eq!(settings(&app, root)[0], tune(authored(0), 1.0));
        app.world_mut()
            .entity_mut(bones[0])
            .insert(Name::new("Hair_Bust1"));
        app.update();
        assert_eq!(
            app.world().resource::<SpringRuntime>().refreshes,
            refreshes + 2
        );
        assert_eq!(settings(&app, root)[0], authored(0));
        let info = &app
            .world()
            .resource::<SpringPhysicsState>()
            .available_joints[0];
        assert!(info.blocked_reason.is_some() && !info.selected && !info.tuned);
    }

    #[test]
    fn reordered_and_replaced_chain_layouts_refresh_metadata_without_compounding() {
        let (mut app, root, bones) = fixture();
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        let refreshes = app.world().resource::<SpringRuntime>().refreshes;
        app.world_mut().get_mut::<SpringBones>(root).unwrap().0[0]
            .joints
            .reverse();
        app.update();
        assert_eq!(
            app.world().resource::<SpringRuntime>().refreshes,
            refreshes + 1
        );
        let info = &app
            .world()
            .resource::<SpringPhysicsState>()
            .available_joints[3];
        assert_eq!(info.bone, bones[0]);
        assert_eq!(info.authored, authored(0));
        assert_eq!(settings(&app, root)[3], tune(authored(0), 1.0));

        // Replace the component and split the chain while retaining joint IDs.
        // The edited chest settings are a new source baseline, not our output.
        let mut replacement = app
            .world_mut()
            .entity_mut(root)
            .take::<SpringBones>()
            .unwrap();
        let chest = replacement.0[0].joints.pop().unwrap();
        let new_authored = SpringSettings {
            stiffness: 3.0,
            ..authored(0)
        };
        replacement.0.push(SpringChain {
            joints: vec![SpringJoint {
                settings: new_authored,
                ..chest
            }],
            colliders: vec![],
        });
        app.world_mut().entity_mut(root).insert(replacement);
        app.update();
        assert_eq!(
            app.world().resource::<SpringRuntime>().refreshes,
            refreshes + 2
        );
        let info = &app
            .world()
            .resource::<SpringPhysicsState>()
            .available_joints[3];
        assert_eq!((info.chain_index, info.joint_index), (1, 0));
        assert_eq!(info.authored, new_authored);
        assert_eq!(settings(&app, root)[3], tune(new_authored, 1.0));
        assert_eq!(
            &settings(&app, root)[..3],
            &[authored(3), authored(2), authored(1)]
        );
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .enabled = false;
        app.update();
        assert_eq!(settings(&app, root)[3], new_authored);
    }

    #[test]
    fn nested_scene_roots_scope_tuning_to_selected_avatar_and_restore_on_switch_and_unload() {
        let (mut app, scene, _) = fixture();
        // Match loader wrapper -> spawned scene -> vendor SpringBones owner.
        let wrapper = app.world_mut().spawn(Transform::default()).id();
        let intermediate = app
            .world_mut()
            .spawn((Transform::default(), ChildOf(wrapper)))
            .id();
        app.world_mut()
            .entity_mut(scene)
            .insert(ChildOf(intermediate));
        let original = settings(&app, scene);
        let (other_scene, other_bones) = add_rig(&mut app, &["Mune_R", "Hair_02"]);
        let other_wrapper = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .entity_mut(other_scene)
            .insert(ChildOf(other_wrapper));
        let other_authored = settings(&app, other_scene);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(wrapper);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert!(app.world().get::<SpringBones>(wrapper).is_none());
        assert_eq!(settings(&app, scene)[0], tune(original[0], 1.0));
        assert_eq!(&settings(&app, scene)[1..], &original[1..]);
        assert_eq!(settings(&app, other_scene), other_authored);
        assert_eq!(
            app.world().resource::<SpringRuntime>().spring_root,
            Some(scene)
        );
        let runtime = app.world().resource::<SpringRuntime>();
        let counts = (runtime.refreshes, runtime.applications);
        for _ in 0..3 {
            app.update();
        }
        let runtime = app.world().resource::<SpringRuntime>();
        assert_eq!((runtime.refreshes, runtime.applications), counts);

        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(other_wrapper);
        app.update();
        assert_eq!(settings(&app, scene), original);
        assert_eq!(settings(&app, other_scene)[0], tune(other_authored[0], 1.0));
        assert_eq!(settings(&app, other_scene)[1], other_authored[1]);
        let state = app.world().resource::<SpringPhysicsState>();
        assert_eq!(state.available_joints.len(), other_bones.len());
        assert!(
            state
                .available_joints
                .iter()
                .all(|joint| other_bones.contains(&joint.bone))
        );
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(settings(&app, other_scene), other_authored);
        let runtime = app.world().resource::<SpringRuntime>();
        assert!(runtime.spring_root.is_none() && runtime.baselines.is_empty());
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints
                .is_empty()
        );
    }

    #[test]
    fn late_replaced_and_ambiguous_nested_spring_roots_refresh_without_tuning_other_avatars() {
        let (mut app, unrelated, _) = fixture();
        let unrelated_authored = settings(&app, unrelated);
        let wrapper = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(wrapper);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints
                .is_empty()
        );
        assert_eq!(settings(&app, unrelated), unrelated_authored);

        let (scene, _) = add_rig(&mut app, &["Breast_L"]);
        app.world_mut().entity_mut(scene).insert(ChildOf(wrapper));
        let original = settings(&app, scene);
        app.update();
        assert_eq!(settings(&app, scene)[0], tune(original[0], 1.0));
        // The wrapper survives while its spawned scene is replaced.
        app.world_mut().entity_mut(scene).remove::<ChildOf>();
        app.update();
        assert_eq!(settings(&app, scene), original);
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints
                .is_empty()
        );
        let (replacement, replacement_bones) = add_rig(&mut app, &["Bust_R", "Hair_01"]);
        app.world_mut()
            .entity_mut(replacement)
            .insert(ChildOf(wrapper));
        app.world_mut()
            .get_mut::<SpringBones>(replacement)
            .unwrap()
            .0[0]
            .joints[0]
            .settings
            .stiffness = 3.0;
        let replacement_authored = settings(&app, replacement);
        app.update();
        assert_eq!(
            settings(&app, replacement)[0],
            tune(replacement_authored[0], 1.0)
        );
        let state = app.world().resource::<SpringPhysicsState>();
        assert_eq!(state.available_joints[0].bone, replacement_bones[0]);
        assert_eq!(state.available_joints[0].authored, replacement_authored[0]);

        // Do not pick an arbitrary rig when two scene roots overlap during load.
        app.world_mut().entity_mut(scene).insert(ChildOf(wrapper));
        app.update();
        assert_eq!(settings(&app, replacement), replacement_authored);
        assert_eq!(settings(&app, scene), original);
        let state = app.world().resource::<SpringPhysicsState>();
        assert!(state.available_joints.is_empty() && state.status.contains("Multiple"));
        app.world_mut().entity_mut(scene).remove::<ChildOf>();
        app.update();
        assert_eq!(
            settings(&app, replacement)[0],
            tune(replacement_authored[0], 1.0)
        );
        assert_eq!(settings(&app, unrelated), unrelated_authored);
    }

    #[test]
    fn real_chains_use_authored_baselines_without_accumulation_or_collider_changes() {
        let (mut app, root, bones) = fixture();
        let original = settings(&app, root);
        let colliders = app.world().get::<SpringBones>(root).unwrap().0[0]
            .colliders
            .clone();
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        let maximum = settings(&app, root);
        assert!((maximum[0].stiffness / original[0].stiffness - 0.20).abs() < 1e-6);
        assert!((maximum[0].drag_force / original[0].drag_force - 0.15).abs() < 1e-6);
        assert_eq!(&maximum[1..], &original[1..]);
        assert_eq!(maximum[0].gravity_power, original[0].gravity_power);
        assert_eq!(maximum[0].gravity_dir, original[0].gravity_dir);
        assert_eq!(maximum[0].hit_radius, original[0].hit_radius);
        for index in 0..60 {
            app.world_mut()
                .get_mut::<Transform>(root)
                .unwrap()
                .translation
                .x = index as f32 * 0.002;
            app.update();
            assert_eq!(settings(&app, root), maximum);
            assert_eq!(
                app.world().get::<SpringBones>(root).unwrap().0[0].colliders,
                colliders
            );
            for &bone in &bones {
                let logic = app.world().get::<SpringBoneLogicState>(bone).unwrap();
                assert!(logic.current_tail.is_finite() && logic.bone_length > 0.0);
                assert!(
                    app.world()
                        .get::<Transform>(bone)
                        .unwrap()
                        .rotation
                        .is_finite()
                );
            }
        }
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 0.65;
        app.update();
        assert_ne!(settings(&app, root)[0], maximum[0]);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert_eq!(settings(&app, root), maximum);
        let state = app.world().resource::<SpringPhysicsState>();
        assert_eq!(state.available_joints.len(), 4);
        assert_eq!(
            state
                .available_joints
                .iter()
                .filter(|joint| joint.detected)
                .count(),
            1
        );
        assert_eq!(state.available_joints[0].authored, original[0]);
    }

    #[test]
    fn manual_selection_restores_prior_chest_joint_and_keeps_hair_and_skirt_authored() {
        let (mut app, root, _) = fixture();
        let original = settings(&app, root);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        let keys: Vec<_> = app
            .world()
            .resource::<SpringPhysicsState>()
            .available_joints
            .iter()
            .skip(1)
            .map(|joint| joint.key.clone())
            .collect();
        {
            let mut state = app.world_mut().resource_mut::<SpringPhysicsState>();
            state.config.selection = JointSelection::Manual;
            state.config.manual_joints = keys.into_iter().collect();
        }
        app.update();
        let actual = settings(&app, root);
        assert_eq!(actual[0], original[0]);
        assert_eq!(actual[1], original[1]);
        assert_ne!(actual[2], original[2]);
        assert_eq!(actual[3], original[3]);
        let metadata = &app
            .world()
            .resource::<SpringPhysicsState>()
            .available_joints;
        assert_eq!(metadata[2].name, "Helper_02");
        assert!(metadata[2].selected && metadata[2].tuned && !metadata[2].detected);
        assert!(metadata[1].blocked_reason.is_some() && metadata[3].blocked_reason.is_some());
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .manual_joints
            .clear();
        app.update();
        assert_eq!(settings(&app, root), original);
    }

    #[test]
    fn disable_and_reset_restore_authored_settings_while_vendor_springs_keep_running() {
        let (mut app, root, bones) = fixture();
        let original = settings(&app, root);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert_ne!(settings(&app, root)[0], original[0]);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .enabled = false;
        app.update();
        assert_eq!(settings(&app, root), original);
        let before = app
            .world()
            .get::<SpringBoneLogicState>(bones[0])
            .unwrap()
            .current_tail;
        app.world_mut()
            .get_mut::<Transform>(root)
            .unwrap()
            .translation
            .x += 0.1;
        app.update();
        let after = app
            .world()
            .get::<SpringBoneLogicState>(bones[0])
            .unwrap()
            .current_tail;
        assert!(after.is_finite() && after.distance(before) > 1e-5);
        {
            let mut state = app.world_mut().resource_mut::<SpringPhysicsState>();
            state.config.enabled = true;
            state.config.strength = 0.65;
        }
        app.update();
        assert_ne!(settings(&app, root)[0], original[0]);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .reset();
        app.update();
        assert_eq!(settings(&app, root), original);
        assert!(app.world().resource::<SpringPhysicsState>().config.enabled);
    }

    #[test]
    fn avatar_switch_and_unload_restore_surviving_roots_and_clear_old_metadata() {
        let (mut app, old, _) = fixture();
        let old_authored = settings(&app, old);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        let (new, bones) = add_rig(&mut app, &["Mune_L"]);
        app.world_mut().get_mut::<SpringBones>(new).unwrap().0[0].joints[0]
            .settings
            .stiffness = 3.0;
        let new_authored = settings(&app, new);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(new);
        app.update();
        assert_eq!(settings(&app, old), old_authored);
        assert!((settings(&app, new)[0].stiffness - 0.6).abs() < 1e-6);
        assert_eq!(app.world().resource::<SpringRuntime>().baselines.len(), 1);
        let state = app.world().resource::<SpringPhysicsState>();
        assert_eq!(state.available_joints.len(), 1);
        assert_eq!(state.available_joints[0].bone, bones[0]);
        assert_eq!(state.available_joints[0].authored, new_authored[0]);
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(settings(&app, new), new_authored);
        assert!(app.world().resource::<SpringRuntime>().baselines.is_empty());
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints
                .is_empty()
        );
    }

    #[test]
    fn unsupported_avatar_does_not_invent_chest_springs_and_late_springs_are_discovered() {
        let (mut app, _, _) = fixture();
        let root = app
            .world_mut()
            .spawn((Name::new("Breast_L"), Transform::default()))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        assert!(app.world().get::<SpringBones>(root).is_none());
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .status
                .contains("unsupported")
        );
        let bone = app
            .world_mut()
            .spawn((Name::new("Bust_L"), Transform::default(), ChildOf(root)))
            .id();
        let tail = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, -0.2, 0.0), ChildOf(bone)))
            .id();
        app.world_mut()
            .entity_mut(root)
            .insert(SpringBones(vec![SpringChain {
                joints: vec![SpringJoint {
                    bone,
                    tail,
                    settings: authored(0),
                }],
                colliders: vec![],
            }]));
        app.update();
        assert!(
            app.world()
                .resource::<SpringPhysicsState>()
                .available_joints[0]
                .detected
        );
    }

    #[test]
    fn helper_name_detection_is_conservative_and_excludes_named_hair_or_clothes() {
        for name in [
            "J_Sec_L_Bust1",
            "Breast_L",
            "RightBreast",
            "mune.R",
            "MUNE_L",
            "左胸",
        ] {
            assert_eq!(classify(name), (true, false), "{name}");
        }
        for name in [
            "Chest",
            "UpperChest",
            "Helper_02",
            "Robust",
            "Bustle",
            "Breastplate",
        ] {
            assert_eq!(classify(name), (false, false), "{name}");
        }
        for name in [
            "Hair_Bust1",
            "BustRibbon",
            "Skirt_01",
            "Breast_Cloth",
            "胸_衣",
        ] {
            assert_eq!(classify(name), (false, true), "{name}");
        }
    }

    #[test]
    fn invalid_authored_settings_are_reported_and_unrelated_live_edits_are_preserved() {
        let (mut app, old, _) = fixture();
        // Another owner changing hair must not be overwritten from our snapshot.
        app.world_mut().get_mut::<SpringBones>(old).unwrap().0[0].joints[1]
            .settings
            .stiffness = 7.0;
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        assert_eq!(settings(&app, old)[1].stiffness, 7.0);
        let (root, _) = add_rig(&mut app, &["Bust_L"]);
        // Out-of-domain authored damping is unsupported, not silently repaired.
        app.world_mut().get_mut::<SpringBones>(root).unwrap().0[0].joints[0]
            .settings
            .drag_force = 1.2;
        let original = settings(&app, root);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        assert_eq!(settings(&app, root), original);
        let info = &app
            .world()
            .resource::<SpringPhysicsState>()
            .available_joints[0];
        assert!(info.detected && !info.selected && !info.tuned);
        assert!(info.blocked_reason.unwrap().contains("Invalid authored"));
    }

    #[test]
    fn invalid_serialized_strength_is_rejected_and_live_edits_are_bounded() {
        for number in [
            "-0.1",
            "1.1",
            "1e100",
            "1e400",
            "null",
            "\"NaN\"",
            "\"Infinity\"",
        ] {
            assert!(
                serde_json::from_str::<SpringPhysicsConfig>(&format!("{{\"strength\":{number}}}"))
                    .is_err()
            );
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0] {
            let mut config = SpringPhysicsConfig {
                strength: value,
                ..default()
            };
            assert!(serde_json::to_string(&config).is_err());
            config.sanitize();
            assert!(config.strength.is_finite() && (0.0..=1.0).contains(&config.strength));
        }
        let (mut app, root, _) = fixture();
        let original = settings(&app, root);
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = 1.0;
        app.update();
        app.world_mut()
            .resource_mut::<SpringPhysicsState>()
            .config
            .strength = f32::NAN;
        app.update();
        assert_eq!(settings(&app, root), original);
        assert_eq!(
            app.world().resource::<SpringPhysicsState>().config.strength,
            0.0
        );
    }

    #[test]
    fn scene_defaults_and_saved_character_switches_preserve_independent_spring_configs() {
        use crate::data::{Character, SceneSettings, Store};
        let legacy: SceneSettings = serde_json::from_str("{}").unwrap();
        assert!(legacy.spring_physics.enabled);
        assert_eq!(legacy.spring_physics.strength, 0.0);
        let path = std::env::temp_dir().join(format!("spring-config-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(path.clone()).unwrap();
        let mut ids = Vec::new();
        for name in ["First", "Second"] {
            let id = uuid::Uuid::new_v4();
            ids.push(id);
            library.characters.push(Character {
                delegation: default(),
                id,
                name: name.into(),
                model: "avatar.vrm".into(),
                persona: String::new(),
                format: "VRM 1".into(),
                greeting: String::new(),
                scene: None,
                provider: None,
                voice: None,
            });
        }
        assert!(library.select_character(ids[0]));
        library.scene.spring_physics.strength = 1.0;
        library.scene.spring_physics.selection = JointSelection::Manual;
        library
            .scene
            .spring_physics
            .manual_joints
            .insert("0/2/Helper_02".into());
        let first = library.scene.spring_physics.clone();
        assert!(library.select_character(ids[1]));
        library.scene.spring_physics = SpringPhysicsConfig {
            strength: 0.25,
            ..default()
        };
        let second = library.scene.spring_physics.clone();
        store.save(&library).unwrap();
        let (_, mut loaded) = Store::open(path.clone()).unwrap();
        assert!(loaded.select_character(ids[0]));
        assert_eq!(loaded.scene.spring_physics, first);
        assert!(loaded.select_character(ids[1]));
        assert_eq!(loaded.scene.spring_physics, second);
        std::fs::remove_dir_all(path).unwrap();
    }
}
