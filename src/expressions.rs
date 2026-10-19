//! Morph and material expressions are decoded by a glTF loader hook, never on the frame thread.
//! VRM 0 binds mesh indices; VRM 1 binds node indices. The hook resolves this
//! distinction before the scene is instantiated and caches each visible avatar.
use crate::{
    loading::AvatarLoadState,
    material_expressions::{
        MaterialExpressionBinds, MaterialExpressionFrame, ParsedMaterials, ResolvedMaterialWeights,
    },
};
use bevy::{
    asset::LoadContext,
    gltf::{
        GltfLoaderSettings,
        extensions::{ErasedGltfExtensionHandler, GltfExtensionHandler, GltfExtensionHandlers},
    },
    mesh::morph::{MeshMorphWeights, MorphWeights},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Instant,
};

use uuid::Uuid;

const MAX_EXPRESSIONS: usize = 256;
const MAX_BINDS: usize = 16_384;
const MAX_BINDING_WAIT_FRAMES: u8 = 120;
const BLINK_CLOSE_SECONDS: f32 = 0.045;
const BLINK_HOLD_SECONDS: f32 = 0.028;
const BLINK_OPEN_SECONDS: f32 = 0.105;
const BLINK_TOTAL_SECONDS: f32 = BLINK_CLOSE_SECONDS + BLINK_HOLD_SECONDS + BLINK_OPEN_SECONDS;
const DEFAULT_BLINK_INTERVAL: f32 = 4.2;
fn unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FacialConfig {
    pub selected_expression: String,
    pub expression_weight: f32,
    pub weights: BTreeMap<String, f32>,
    pub auto_blink: bool,
    pub blink_strength: f32,
    pub blink_interval_secs: f32,
    pub lip_sync: bool,
    pub mouth_expression: String,
    pub mouth_gain: f32,
}
impl Default for FacialConfig {
    fn default() -> Self {
        Self {
            selected_expression: String::new(),
            expression_weight: 1.0,
            weights: BTreeMap::new(),
            auto_blink: true,
            blink_strength: 1.0,
            blink_interval_secs: DEFAULT_BLINK_INTERVAL,
            lip_sync: true,
            mouth_expression: "aa".into(),
            mouth_gain: 2.0,
        }
    }
}
#[derive(Resource, Default)]
pub struct FacialState {
    pub config: FacialConfig,
    pub available: Vec<String>,
    pub status: String,
}
/// Insert on each scene occupant root. Focused config still comes from FacialState.
#[derive(Component, Clone)]
pub struct SceneMemberFacial {
    pub character: Uuid,
    pub config: FacialConfig,
}
/// None preserves focused-character playback; Some routes audio to that character only.
#[derive(Resource, Default)]
pub struct SpeechTarget(pub Option<Uuid>);

struct EnvelopeInput {
    level: AtomicU32,
    updated: AtomicU64,
    epoch: Instant,
}
/// Clone this resource into a playback worker and call `set` with the RMS of
/// actually played samples. Values expire after 250ms; stopped audio cannot
/// leave a character's mouth open. Calling `silence` stops it immediately.
#[derive(Resource, Clone)]
pub struct AudioEnvelope(Arc<EnvelopeInput>);
impl Default for AudioEnvelope {
    fn default() -> Self {
        Self(Arc::new(EnvelopeInput {
            level: AtomicU32::new(0),
            updated: AtomicU64::new(0),
            epoch: Instant::now(),
        }))
    }
}
impl AudioEnvelope {
    pub fn set(&self, value: f32) {
        self.0.level.store(unit(value).to_bits(), Ordering::Relaxed);
        self.0.updated.store(
            self.0.epoch.elapsed().as_micros().min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
    }
    pub fn silence(&self) {
        self.set(0.0);
    }
    pub fn level(&self) -> f32 {
        let now = self.0.epoch.elapsed().as_micros().min(u64::MAX as u128) as u64;
        if now.saturating_sub(self.0.updated.load(Ordering::Acquire)) > 250_000 {
            0.0
        } else {
            unit(f32::from_bits(self.0.level.load(Ordering::Relaxed)))
        }
    }
    pub fn set_pcm(&self, samples: &[f32]) {
        let energy: f64 = samples
            .iter()
            .filter(|s| s.is_finite())
            .map(|s| (*s as f64).clamp(-1.0, 1.0).powi(2))
            .sum();
        self.set(if samples.is_empty() {
            0.0
        } else {
            (energy / samples.len() as f64).sqrt() as f32
        });
    }
}

#[derive(Reflect, Clone, Debug, Default)]
struct ExpressionMeta {
    name: String,
    binary: bool,
    blink_override: u8,
    mouth_override: u8,
    look_override: u8,
}
#[derive(Reflect, Clone, Debug)]
struct NodeBind {
    expression: usize,
    index: usize,
    weight: f32,
}
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Clone)]
struct ExpressionNode {
    binds: Vec<NodeBind>,
}
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Clone)]
struct ExpressionCatalog {
    expressions: Vec<ExpressionMeta>,
    material_issues: Vec<String>,
}
#[derive(Clone)]
struct ParsedBind {
    node: usize,
    bind: NodeBind,
}
#[derive(Default, Clone)]
struct ExpressionHandler {
    expressions: Vec<ExpressionMeta>,
    binds: Vec<ParsedBind>,
    materials: ParsedMaterials,
}

fn canonical_preset(name: &str) -> &str {
    match name {
        "a" => "aa",
        "i" => "ih",
        "u" => "ou",
        "e" => "ee",
        "o" => "oh",
        "joy" => "happy",
        "sorrow" => "sad",
        "fun" => "relaxed",
        "blink_l" => "blinkLeft",
        "blink_r" => "blinkRight",
        "lookup" => "lookUp",
        "lookdown" => "lookDown",
        "lookleft" => "lookLeft",
        "lookright" => "lookRight",
        name => name,
    }
}
fn override_kind(value: Option<&Value>) -> u8 {
    match value.and_then(Value::as_str) {
        Some("block") => 1,
        Some("blend") => 2,
        _ => 0,
    }
}
impl ExpressionHandler {
    fn parse(gltf: &gltf::Gltf) -> Self {
        let mut result = Self::default();
        if let Some(expressions) = gltf
            .extension_value("VRMC_vrm")
            .and_then(|v| v.get("expressions"))
        {
            for group in ["preset", "custom"] {
                if let Some(entries) = expressions.get(group).and_then(Value::as_object) {
                    for (name, value) in entries.iter().take(MAX_EXPRESSIONS) {
                        if result.expressions.len() >= MAX_EXPRESSIONS {
                            break;
                        }
                        let name = if group == "preset" {
                            canonical_preset(name)
                        } else {
                            name
                        };
                        result.push_expression(gltf, name, value, false);
                    }
                }
            }
        } else if let Some(groups) = gltf
            .extension_value("VRM")
            .and_then(|v| v.pointer("/blendShapeMaster/blendShapeGroups"))
            .and_then(Value::as_array)
        {
            for value in groups.iter().take(MAX_EXPRESSIONS) {
                let preset = value
                    .get("presetName")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let name = if preset == "unknown" {
                    value.get("name").and_then(Value::as_str).unwrap_or("")
                } else {
                    canonical_preset(preset)
                };
                result.push_expression(gltf, name, value, true);
            }
        }
        result
    }
    fn push_expression(&mut self, gltf: &gltf::Gltf, name: &str, value: &Value, vrm0: bool) {
        if name.is_empty() || name.len() > 256 || self.expressions.iter().any(|e| e.name == name) {
            return;
        }
        let expression = self.expressions.len();
        let first = self.binds.len();
        let key = if vrm0 { "binds" } else { "morphTargetBinds" };
        if let Some(binds) = value.get(key).and_then(Value::as_array) {
            for bind in binds.iter().take(MAX_BINDS) {
                let Some(target) = bind
                    .get(if vrm0 { "mesh" } else { "node" })
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                else {
                    continue;
                };
                let Some(index) = bind
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                else {
                    continue;
                };
                let weight = bind.get("weight").and_then(Value::as_f64).unwrap_or(0.0) as f32
                    / if vrm0 { 100.0 } else { 1.0 };
                if !weight.is_finite() || weight <= 0.0 {
                    continue;
                }
                for node in gltf.nodes().filter(|n| {
                    if vrm0 {
                        n.mesh().is_some_and(|m| m.index() == target)
                    } else {
                        n.index() == target
                    }
                }) {
                    if self.binds.len() >= MAX_BINDS {
                        break;
                    }
                    // A primitive without this target must never receive an invalid bind.
                    if !node
                        .mesh()
                        .is_some_and(|m| m.primitives().any(|p| p.morph_targets().len() > index))
                    {
                        continue;
                    }
                    self.binds.push(ParsedBind {
                        node: node.index(),
                        bind: NodeBind {
                            expression,
                            index,
                            weight: unit(weight),
                        },
                    });
                }
            }
        }
        self.materials
            .parse_expression(gltf, expression, value, vrm0, MAX_BINDS);
        let material_requested = [
            "materialValues",
            "materialColorBinds",
            "textureTransformBinds",
        ]
        .iter()
        .any(|key| {
            value
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty())
        });
        if self.binds.len() > first || material_requested {
            self.expressions.push(ExpressionMeta {
                name: name.to_owned(),
                binary: value
                    .get("isBinary")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                blink_override: override_kind(value.get("overrideBlink")),
                mouth_override: override_kind(value.get("overrideMouth")),
                look_override: override_kind(value.get("overrideLookAt")),
            });
        }
    }
}
impl GltfExtensionHandler for ExpressionHandler {
    fn dyn_clone(&self) -> Box<dyn ErasedGltfExtensionHandler> {
        Box::new(self.clone())
    }
    fn on_root(&mut self, _: &mut LoadContext<'_>, gltf: &gltf::Gltf, _: &GltfLoaderSettings) {
        *self = Self::parse(gltf);
    }
    fn on_gltf_node(
        &mut self,
        _: &mut LoadContext<'_>,
        node: &gltf::Node,
        entity: &mut EntityWorldMut,
    ) {
        let binds = self
            .binds
            .iter()
            .filter(|bind| bind.node == node.index())
            .map(|bind| bind.bind.clone())
            .collect::<Vec<_>>();
        if !binds.is_empty() {
            entity.insert(ExpressionNode { binds });
        }
    }
    fn on_spawn_mesh_and_material(
        &mut self,
        _: &mut LoadContext<'_>,
        _: &gltf::Primitive,
        _: &gltf::Mesh,
        material: &gltf::Material,
        entity: &mut EntityWorldMut,
        _: &str,
    ) {
        if let Some(binds) = material.index().and_then(|i| self.materials.binds.get(&i)) {
            entity.insert(binds.clone());
        }
    }
    fn on_scene_completed(
        &mut self,
        _: &mut LoadContext<'_>,
        _: &gltf::Scene,
        root: Entity,
        world: &mut World,
    ) {
        // The empty catalog is also a readiness marker for ordinary glTF
        // scenes, so they do not trigger repeated searches for VRM metadata.
        world.entity_mut(root).insert(ExpressionCatalog {
            expressions: self.expressions.clone(),
            material_issues: self.materials.issues.iter().cloned().collect(),
        });
    }
}

/// Face updates precede animation/outline synchronization in PostUpdate.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExpressionSystems;

pub struct ExpressionsPlugin;
impl Plugin for ExpressionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FacialState>()
            .init_resource::<AudioEnvelope>()
            .init_resource::<FacialRuntime>()
            .init_resource::<SpeechTarget>()
            .init_resource::<MaterialExpressionFrame>()
            .register_type::<ExpressionNode>()
            .register_type::<ExpressionCatalog>()
            .register_type::<MaterialExpressionBinds>()
            .configure_sets(
                PostUpdate,
                ExpressionSystems.before(bevy::app::AnimationSystems),
            )
            .add_systems(PostUpdate, update_expressions.in_set(ExpressionSystems));
    }
    fn finish(&self, app: &mut App) {
        if let Some(handlers) = app.world().get_resource::<GltfExtensionHandlers>() {
            handlers
                .0
                .write_blocking()
                .push(Box::new(ExpressionHandler::default()));
        }
    }
}
struct CachedNode {
    entity: Entity,
    primitives: Vec<Entity>,
    binds: Vec<NodeBind>,
    baseline: Vec<(usize, f32)>,
}
#[derive(Resource, Default)]
struct FacialRuntime {
    root: Option<Entity>,
    nodes: Vec<CachedNode>,
    expressions: Vec<ExpressionMeta>,
    blink: BlinkRuntime,
    mouth: f32,
    bindings_ready: bool,
    binding_wait_frames: u8,
}
fn under_root(entity: Entity, root: Entity, parents: &Query<&ChildOf>) -> bool {
    let mut cursor = entity;
    for _ in 0..512 {
        if cursor == root {
            return true;
        }
        let Ok(parent) = parents.get(cursor) else {
            return false;
        };
        cursor = parent.parent();
    }
    false
}
fn blink_interval(interval: f32) -> f32 {
    if interval.is_finite() {
        interval.clamp(1.5, 10.0)
    } else {
        DEFAULT_BLINK_INTERVAL
    }
}
fn blink_curve(blink_time: f32) -> f32 {
    let ease = |value: f32| 0.5 - (std::f32::consts::PI * unit(value)).cos() * 0.5;
    if blink_time < 0.0 {
        0.0
    } else if blink_time <= BLINK_CLOSE_SECONDS {
        ease(blink_time / BLINK_CLOSE_SECONDS)
    } else if blink_time <= BLINK_CLOSE_SECONDS + BLINK_HOLD_SECONDS {
        1.0
    } else if blink_time <= BLINK_TOTAL_SECONDS {
        1.0 - ease((blink_time - BLINK_CLOSE_SECONDS - BLINK_HOLD_SECONDS) / BLINK_OPEN_SECONDS)
    } else {
        0.0
    }
}
fn blink_delay(character: Uuid, ordinal: u64, interval: f32) -> f32 {
    // Mix all UUID bytes and the blink index: stable across launches/spawn
    // order, without the correlated low-byte offsets of a shared scene clock.
    let id = character.as_u128();
    let mut seed = (id as u64) ^ ((id >> 64) as u64).rotate_left(17);
    seed = seed.wrapping_add(ordinal.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    seed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    seed = (seed ^ (seed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    seed ^= seed >> 31;
    let random = (seed >> 40) as f32 / 0x00ff_ffffu32 as f32;
    (blink_interval(interval) * (0.7 + random * 0.7)).clamp(1.0, 12.0)
}
#[derive(Default)]
struct BlinkRuntime {
    character: Option<Uuid>,
    interval: f32,
    enabled: bool,
    elapsed: f32,
    next_at: f32,
    ordinal: u64,
}
impl BlinkRuntime {
    fn update(&mut self, dt: f32, character: Uuid, config: &FacialConfig) -> f32 {
        let interval = blink_interval(config.blink_interval_secs);
        let enabled = config.auto_blink && unit(config.blink_strength) > 0.0;
        if self.character != Some(character) || self.interval != interval || self.enabled != enabled
        {
            *self = Self {
                character: Some(character),
                interval,
                enabled,
                next_at: blink_delay(character, 0, interval),
                ..default()
            };
        }
        if !enabled {
            return 0.0;
        }
        self.elapsed += if dt.is_finite() {
            dt.clamp(0.0, 0.1)
        } else {
            0.0
        };
        if self.elapsed > self.next_at + BLINK_TOTAL_SECONDS {
            // Carry the sub-frame remainder so cadence is stable across FPS.
            self.elapsed -= self.next_at + BLINK_TOTAL_SECONDS;
            self.ordinal = self.ordinal.wrapping_add(1);
            self.next_at = blink_delay(character, self.ordinal, interval);
        }
        blink_curve(self.elapsed - self.next_at)
    }
}
fn is_blink(name: &str) -> bool {
    matches!(name, "blink" | "blinkLeft" | "blinkRight")
}
fn is_mouth(name: &str) -> bool {
    matches!(name, "aa" | "ih" | "ou" | "ee" | "oh")
}
fn expression_weights(
    expressions: &[ExpressionMeta],
    config: &FacialConfig,
    blink: f32,
    mouth: f32,
    vowels: Option<[f32; 5]>,
) -> Vec<f32> {
    let has_blink = expressions.iter().any(|e| e.name == "blink");
    let mut values: Vec<f32> = expressions
        .iter()
        .map(|e| {
            let mut w = unit(config.weights.get(&e.name).copied().unwrap_or(0.0));
            if e.name == config.selected_expression {
                w = w.max(unit(config.expression_weight));
            }
            if config.auto_blink && (e.name == "blink" || (!has_blink && is_blink(&e.name))) {
                w = w.max(unit(blink * unit(config.blink_strength)));
            }
            // Old settings panels inserted zero entries merely by drawing sliders.
            // Only an active manual weight owns the channel; neutral sliders must
            // leave automatic speech available, including in existing saved files.
            let manual = unit(config.weights.get(&e.name).copied().unwrap_or(0.0)) > 0.0
                || e.name == config.selected_expression;
            if config.lip_sync && !manual {
                if let Some(vowels) = vowels {
                    if let Some(index) = ["aa", "ih", "ou", "ee", "oh"]
                        .iter()
                        .position(|name| *name == e.name)
                    {
                        w = w.max(unit(vowels[index]));
                    }
                } else if e.name == config.mouth_expression {
                    w = w.max(unit(mouth));
                }
            }
            if e.binary {
                if w >= 0.5 { 1.0 } else { 0.0 }
            } else {
                w
            }
        })
        .collect();
    let mut factors = [1.0f32; 3];
    for (e, &weight) in expressions.iter().zip(&values) {
        if weight <= 0.0 {
            continue;
        }
        for (factor, kind) in
            factors
                .iter_mut()
                .zip([e.blink_override, e.mouth_override, e.look_override])
        {
            match kind {
                1 => *factor = 0.0,
                2 => *factor = (*factor - weight).max(0.0),
                _ => {}
            }
        }
    }
    for (e, w) in expressions.iter().zip(&mut values) {
        let factor = if is_blink(&e.name) {
            factors[0]
        } else if is_mouth(&e.name) {
            factors[1]
        } else if e.name.starts_with("look") {
            factors[2]
        } else {
            1.0
        };
        *w *= factor;
        // Binary outputs remain discrete even when another expression blends overrides.
        if e.binary {
            *w = if *w >= 0.5 { 1.0 } else { 0.0 };
        }
    }
    values
}
fn compose(weights: &mut [f32], node: &CachedNode, expression_weights: &[f32]) {
    for &(index, baseline) in &node.baseline {
        if let Some(weight) = weights.get_mut(index) {
            *weight = baseline;
        }
    }
    for bind in &node.binds {
        if let Some(weight) = weights.get_mut(bind.index) {
            *weight = unit(
                *weight
                    + expression_weights
                        .get(bind.expression)
                        .copied()
                        .unwrap_or(0.0)
                        * bind.weight,
            );
        }
    }
}
fn behavior_protected(expression: &ExpressionMeta, config: &FacialConfig) -> bool {
    crate::behavior::reserved_expression(&expression.name)
        || expression.name == config.mouth_expression
        || expression.name == config.selected_expression
        || config
            .weights
            .get(&expression.name)
            .is_some_and(|weight| unit(*weight) > 0.0)
        || expression.blink_override != 0
        || expression.mouth_override != 0
        || expression.look_override != 0
}
fn material_weights(
    expressions: &[ExpressionMeta],
    config: &FacialConfig,
    manual: &[f32],
    automatic: &BTreeMap<String, f32>,
) -> ResolvedMaterialWeights {
    ResolvedMaterialWeights {
        manual: manual.to_vec(),
        automatic: expressions
            .iter()
            .map(|e| {
                let value = unit(automatic.get(&e.name).copied().unwrap_or(0.0));
                if e.binary {
                    if value >= 0.5 { 1.0 } else { 0.0 }
                } else {
                    value
                }
            })
            .collect(),
        protected: expressions
            .iter()
            .map(|e| behavior_protected(e, config))
            .collect(),
    }
}
/// Behavior is an additional contribution only on channels whose morph indices
/// do not belong to blink, mouth, look, overrides, or a manual expression.
fn compose_behavior(
    weights: &mut [f32],
    node: &CachedNode,
    expressions: &[ExpressionMeta],
    config: &FacialConfig,
    automatic: &BTreeMap<String, f32>,
) {
    if automatic.is_empty() {
        return;
    }
    let protected = |expression: &ExpressionMeta| behavior_protected(expression, config);
    let protected_indices: std::collections::BTreeSet<usize> = node
        .binds
        .iter()
        .filter(|bind| expressions.get(bind.expression).is_none_or(&protected))
        .map(|bind| bind.index)
        .collect();
    for bind in &node.binds {
        let Some(expression) = expressions.get(bind.expression) else {
            continue;
        };
        if protected(expression) || protected_indices.contains(&bind.index) {
            continue;
        }
        let mut value = unit(automatic.get(&expression.name).copied().unwrap_or(0.0));
        if expression.binary {
            value = if value >= 0.5 { 1.0 } else { 0.0 };
        }
        if let Some(weight) = weights.get_mut(bind.index) {
            *weight = unit(*weight + value * bind.weight);
        }
    }
}
fn restore_nodes(
    cache: &FacialRuntime,
    morphs: &mut Query<&mut MorphWeights>,
    meshes: &mut Query<(Entity, &ChildOf, &mut MeshMorphWeights)>,
) {
    for node in &cache.nodes {
        if let Ok(mut morph) = morphs.get_mut(node.entity) {
            for &(index, baseline) in &node.baseline {
                if let Some(weight) = morph.weights_mut().get_mut(index) {
                    *weight = baseline;
                }
            }
        }
        for &primitive in &node.primitives {
            if let Ok((_, parent, mut mesh)) = meshes.get_mut(primitive)
                && parent.parent() == node.entity
                && let MeshMorphWeights::Value { weights } = &mut *mesh
            {
                for &(index, baseline) in &node.baseline {
                    if let Some(weight) = weights.get_mut(index) {
                        *weight = baseline;
                    }
                }
            }
        }
    }
}
fn bind_scene(
    cache: &mut FacialRuntime,
    root: Entity,
    parents: &Query<&ChildOf>,
    catalogs: &Query<(Entity, &ExpressionCatalog)>,
    nodes: &Query<(Entity, &ExpressionNode)>,
    morphs: &Query<&mut MorphWeights>,
    meshes: &Query<(Entity, &ChildOf, &mut MeshMorphWeights)>,
) {
    if cache.bindings_ready || cache.binding_wait_frames >= MAX_BINDING_WAIT_FRAMES {
        return;
    }
    cache.binding_wait_frames += 1;
    let Some((_, catalog)) = catalogs
        .iter()
        .find(|(entity, _)| under_root(*entity, root, parents))
    else {
        return;
    };
    cache.bindings_ready = true;
    cache.expressions = catalog.expressions.clone();
    for (entity, node) in nodes.iter() {
        if !under_root(entity, root, parents) {
            continue;
        }
        let Ok(morph) = morphs.get(entity) else {
            continue;
        };
        let mut baseline = BTreeMap::new();
        for bind in &node.binds {
            if let Some(&weight) = morph.weights().get(bind.index) {
                baseline.entry(bind.index).or_insert(weight);
            }
        }
        cache.nodes.push(CachedNode {
            entity,
            primitives: meshes
                .iter()
                .filter_map(|(primitive, parent, _)| {
                    (parent.parent() == entity).then_some(primitive)
                })
                .collect(),
            binds: node.binds.clone(),
            baseline: baseline.into_iter().collect(),
        });
    }
}

// Bevy injects these independent resources/queries as system parameters.
#[allow(clippy::too_many_arguments)]
fn update_expressions(
    avatar: Res<AvatarLoadState>,
    time: Res<Time>,
    envelope: Res<AudioEnvelope>,
    behavior: Option<Res<crate::behavior::BehaviorState>>,
    visemes: Option<Res<crate::viseme_audio::VisemeState>>,
    mut state: ResMut<FacialState>,
    mut runtime: ResMut<FacialRuntime>,
    parents: Query<&ChildOf>,
    catalogs: Query<(Entity, &ExpressionCatalog)>,
    nodes: Query<(Entity, &ExpressionNode)>,
    mut morphs: Query<&mut MorphWeights>,
    mut meshes: Query<(Entity, &ChildOf, &mut MeshMorphWeights)>,
    members: Query<(Entity, &SceneMemberFacial)>,
    speech: Option<Res<SpeechTarget>>,
    mut extras: Local<BTreeMap<Entity, FacialRuntime>>,
    mut material_frame: Option<ResMut<MaterialExpressionFrame>>,
) {
    if let Some(frame) = material_frame.as_mut() {
        frame.0.clear();
    }
    // Restore departing occupants before any newly focused root captures its baseline.
    extras.retain(|root, cache| {
        let retain = Some(*root) != avatar.active && members.get(*root).is_ok();
        if !retain {
            restore_nodes(cache, &mut morphs, &mut meshes);
        }
        retain
    });
    let focused_character = avatar
        .active
        .and_then(|root| members.get(root).ok().map(|(_, member)| member.character))
        .or_else(|| behavior.as_ref().and_then(|b| b.controller.character()));
    let speaker = speech.as_ref().and_then(|target| target.0);
    let focused_speaks = speaker.is_none() || speaker == focused_character;
    if runtime.root != avatar.active {
        // Restore only the channels this plugin owned, even if an old root is retained.
        for node in &runtime.nodes {
            if let Ok(mut morph) = morphs.get_mut(node.entity) {
                for &(i, v) in &node.baseline {
                    if let Some(w) = morph.weights_mut().get_mut(i) {
                        *w = v;
                    }
                }
            }
            for &primitive in &node.primitives {
                if let Ok((_, parent, mut mesh)) = meshes.get_mut(primitive)
                    && parent.parent() == node.entity
                    && let MeshMorphWeights::Value { weights } = &mut *mesh
                {
                    for &(i, v) in &node.baseline {
                        if let Some(w) = weights.get_mut(i) {
                            *w = v;
                        }
                    }
                }
            }
        }
        *runtime = FacialRuntime {
            root: avatar.active,
            ..default()
        };
        // Do not interrupt an explicitly targeted companion when focus changes.
        if speaker.is_none() {
            envelope.silence();
        }
        state.available.clear();
        state.status = "No VRM expressions in this model".into();
    }
    // The loader promotes active only after WorldInstanceReady. Also tolerate
    // a caller setting active before its scene arrives: retry a bounded number
    // of frames, and stop immediately once the catalog readiness marker exists.
    if !runtime.bindings_ready
        && runtime.binding_wait_frames < MAX_BINDING_WAIT_FRAMES
        && let Some(root) = avatar.active
    {
        runtime.binding_wait_frames += 1;
        if let Some((_, catalog)) = catalogs
            .iter()
            .find(|(entity, _)| under_root(*entity, root, &parents))
        {
            runtime.bindings_ready = true;
            runtime.expressions = catalog.expressions.clone();
            state.available = runtime.expressions.iter().map(|e| e.name.clone()).collect();
            state.status = format!(
                "{} VRM expression definitions (morph/material)",
                state.available.len()
            );
            if !catalog.material_issues.is_empty() {
                state.status.push_str(". Limitations: ");
                state.status.push_str(&catalog.material_issues.join("; "));
            }
            for (entity, node) in &nodes {
                if !under_root(entity, root, &parents) {
                    continue;
                }
                let Ok(morph) = morphs.get(entity) else {
                    continue;
                };
                let mut baseline = BTreeMap::new();
                for bind in &node.binds {
                    if let Some(&w) = morph.weights().get(bind.index) {
                        baseline.entry(bind.index).or_insert(w);
                    }
                }
                runtime.nodes.push(CachedNode {
                    entity,
                    primitives: meshes
                        .iter()
                        .filter_map(|(primitive, parent, _)| {
                            (parent.parent() == entity).then_some(primitive)
                        })
                        .collect(),
                    binds: node.binds.clone(),
                    baseline: baseline.into_iter().collect(),
                });
            }
        }
    }
    let dt = time.delta_secs().clamp(0.0, 0.1);
    let blink = runtime
        .blink
        .update(dt, focused_character.unwrap_or(Uuid::nil()), &state.config);
    let gain = if state.config.mouth_gain.is_finite() {
        state.config.mouth_gain.clamp(0.0, 20.0)
    } else {
        2.0
    };
    let target = if state.config.lip_sync && focused_speaks {
        unit(envelope.level() * gain)
    } else {
        0.0
    };
    let smoothing = 1.0 - (-dt * if target > runtime.mouth { 35.0 } else { 18.0 }).exp();
    runtime.mouth = if focused_speaks {
        runtime.mouth + (target - runtime.mouth) * smoothing
    } else {
        0.0
    };
    let values = expression_weights(
        &runtime.expressions,
        &state.config,
        blink,
        runtime.mouth,
        visemes
            .as_ref()
            .filter(|_| focused_speaks)
            .and_then(|state| state.weights())
            .map(|weights| weights.map(|value| unit(value * gain))),
    );
    let automatic = behavior
        .filter(|_| matches!(avatar.status, crate::loading::LoadStatus::Ready))
        .map(|behavior| {
            behavior.controller.weights_for(
                behavior.controller.character(),
                &state.available,
                &state.config.weights,
                &state.config.selected_expression,
            )
        })
        .unwrap_or_default();
    if let (Some(frame), Some(root)) = (material_frame.as_mut(), avatar.active) {
        frame.0.insert(
            root,
            material_weights(&runtime.expressions, &state.config, &values, &automatic),
        );
    }
    for node in &runtime.nodes {
        if let Ok(mut morph) = morphs.get_mut(node.entity) {
            compose(morph.weights_mut(), node, &values);
            compose_behavior(
                morph.weights_mut(),
                node,
                &runtime.expressions,
                &state.config,
                &automatic,
            );
        }
        // glTF normally creates Reference(node), which renders the updated
        // MorphWeights directly. Also support independently valued primitives.
        for &primitive in &node.primitives {
            if let Ok((_, parent, mut mesh)) = meshes.get_mut(primitive)
                && parent.parent() == node.entity
                && let MeshMorphWeights::Value { weights } = &mut *mesh
            {
                compose(weights, node, &values);
                compose_behavior(
                    weights,
                    node,
                    &runtime.expressions,
                    &state.config,
                    &automatic,
                );
            }
        }
    }
    for (root, member) in &members {
        if Some(root) == avatar.active {
            continue;
        }
        let cache = extras.entry(root).or_insert_with(|| FacialRuntime {
            root: Some(root),
            ..default()
        });
        bind_scene(cache, root, &parents, &catalogs, &nodes, &morphs, &meshes);
        let blink = cache.blink.update(dt, member.character, &member.config);
        let speaks = speaker == Some(member.character);
        let gain = if member.config.mouth_gain.is_finite() {
            member.config.mouth_gain.clamp(0.0, 20.0)
        } else {
            2.0
        };
        let target = if speaks && member.config.lip_sync {
            unit(envelope.level() * gain)
        } else {
            0.0
        };
        let smoothing = 1.0 - (-dt * if target > cache.mouth { 35.0 } else { 18.0 }).exp();
        cache.mouth = if speaks {
            cache.mouth + (target - cache.mouth) * smoothing
        } else {
            0.0
        };
        let values = expression_weights(
            &cache.expressions,
            &member.config,
            blink,
            cache.mouth,
            visemes
                .as_ref()
                .filter(|_| speaks)
                .and_then(|state| state.weights())
                .map(|weights| weights.map(|value| unit(value * gain))),
        );
        if let Some(frame) = material_frame.as_mut() {
            frame.0.insert(
                root,
                material_weights(
                    &cache.expressions,
                    &member.config,
                    &values,
                    &BTreeMap::new(),
                ),
            );
        }
        for node in &cache.nodes {
            if let Ok(mut morph) = morphs.get_mut(node.entity) {
                compose(morph.weights_mut(), node, &values);
            }
            for &primitive in &node.primitives {
                if let Ok((_, parent, mut mesh)) = meshes.get_mut(primitive)
                    && parent.parent() == node.entity
                    && let MeshMorphWeights::Value { weights } = &mut *mesh
                {
                    compose(weights, node, &values);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn material_fixture(expressions: Value, vrm0: bool) -> gltf::Gltf {
        let extension = if vrm0 {
            serde_json::json!({"VRM": {"blendShapeMaster": {"blendShapeGroups": expressions},
                "materialProperties": [{"name":"face", "vectorProperties": {"_Color":[0.5,0.5,0.5,1.0], "_RimColor":[0.1,0.2,0.3,1.0]}}]}})
        } else {
            serde_json::json!({"VRMC_vrm": {"expressions": {"custom": expressions}}})
        };
        let json = serde_json::json!({"asset":{"version":"2.0"}, "extensions":extension,
            "materials":[{"name":"face", "pbrMetallicRoughness":{"baseColorTexture":{"index":0,
                "extensions":{"KHR_texture_transform":{"scale":[2.0,3.0],"offset":[0.1,0.2]}}}}}],
            "textures":[{"source":0}], "images":[{"uri":"offline-fixture.png"}]});
        gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&json).unwrap()).unwrap()
    }
    fn all_material_binds() -> Value {
        serde_json::json!({"tint": {
            "materialColorBinds": [
                {"material":0,"type":"color","targetValue":[0.9,0.3,0.1,0.4]},
                {"material":0,"type":"emissionColor","targetValue":[0.9,0.3,0.1,1.0]},
                {"material":0,"type":"shadeColor","targetValue":[0.9,0.3,0.1,1.0]},
                {"material":0,"type":"matcapColor","targetValue":[0.9,0.3,0.1,1.0]},
                {"material":0,"type":"rimColor","targetValue":[0.9,0.3,0.1,1.0]},
                {"material":0,"type":"outlineColor","targetValue":[0.9,0.3,0.1,1.0]}],
            "textureTransformBinds":[{"material":0,"scale":[4.0,5.0],"offset":[0.3,0.4]}]},
            "other": {"materialColorBinds":[{"material":0,"type":"color","targetValue":[0.1,0.9,0.7,0.8]}]}})
    }
    #[test]
    fn parser_retains_material_only_expressions_and_reports_unsupported_properties() {
        let parsed = ExpressionHandler::parse(&material_fixture(all_material_binds(), false));
        assert_eq!(parsed.expressions.len(), 2);
        assert!(parsed.binds.is_empty());
        assert_eq!(parsed.materials.count(), 8);
        assert_eq!(
            parsed.materials.binds.keys().copied().collect::<Vec<_>>(),
            vec![0]
        );
        assert_eq!(
            parsed.materials.binds[&0].authored_uv,
            Some([2., 3., 0.1, 0.2])
        );
        assert!(
            parsed
                .materials
                .issues
                .iter()
                .any(|s| s.contains("Toon has no UV"))
        );
        let zero = ExpressionHandler::parse(&material_fixture(
            serde_json::json!([
                {"presetName":"joy","materialValues":[
                    {"materialName":"face","propertyName":"_Color","targetValue":[1.0,0.0,0.0,1.0]},
                    {"materialName":"face","propertyName":"_RimColor","targetValue":[1.0,0.0,0.0,1.0]},
                    {"materialName":"face","propertyName":"_MainTex_ST","targetValue":[1.0,1.0,0.0,0.0]},
                    {"materialName":"face","propertyName":"_Metallic","targetValue":[0.5]}]}
            ]),
            true,
        ));
        assert_eq!(zero.expressions[0].name, "happy");
        assert_eq!(zero.materials.count(), 2);
        assert_eq!(
            zero.materials.binds[&0].binds[1].authored,
            Some([0.1, 0.2, 0.3, 1.])
        );
        assert!(
            zero.materials
                .issues
                .iter()
                .any(|s| s.contains("_MainTex_ST"))
        );
        assert!(
            zero.materials
                .issues
                .iter()
                .any(|s| s.contains("_Metallic"))
        );
    }
    #[test]
    fn malformed_ambiguous_and_unsupported_material_binds_have_explicit_status() {
        let mut json = serde_json::json!({"asset":{"version":"2.0"},
        "materials":[{"name":"duplicate"},{"name":"duplicate"}],
        "extensions":{"VRM":{"blendShapeMaster":{"blendShapeGroups":[{"name":"ambiguous","materialValues":[
            {"materialName":"duplicate","propertyName":"_Color","targetValue":[1.,0.,0.,1.]}
        ]}]}}}});
        let parse = |json: &Value| {
            ExpressionHandler::parse(
                &gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(json).unwrap())
                    .unwrap(),
            )
        };
        let ambiguous = parse(&json);
        assert_eq!(ambiguous.materials.count(), 0);
        assert_eq!(ambiguous.expressions.len(), 1);
        assert!(
            ambiguous
                .materials
                .issues
                .iter()
                .any(|s| s.contains("ambiguous"))
        );
        json["extensions"] = serde_json::json!({"VRMC_vrm":{"expressions":{"custom":{"invalid":{
            "materialColorBinds":[
                {"material":99,"type":"color","targetValue":[1.,0.,0.,1.]},
                {"material":0,"type":"color","targetValue":[1.,0.,0.]},
                {"material":0,"type":"futureType","targetValue":[1.,0.,0.,1.]}],
            "textureTransformBinds":[{"material":0,"scale":[1.]}]
        }}}}});
        let malformed = parse(&json);
        assert_eq!(malformed.materials.count(), 0);
        assert_eq!(malformed.materials.issues.len(), 4);
        assert!(
            malformed
                .materials
                .issues
                .iter()
                .any(|s| s.contains("futureType"))
        );
        json["materials"][0]["pbrMetallicRoughness"] = serde_json::json!({"baseColorTexture":{"index":0,
            "extensions":{"KHR_texture_transform":{"rotation":0.5}}}});
        json["textures"] = serde_json::json!([{"source":0}]);
        json["images"] = serde_json::json!([{"uri":"fixture.png"}]);
        json["extensions"] = serde_json::json!({"VRMC_vrm":{"expressions":{"custom":{"uv":{
            "textureTransformBinds":[{"material":0,"scale":[2.,3.]}]
        }}}}});
        let rotated = parse(&json);
        assert_eq!(rotated.materials.count(), 0);
        assert!(
            rotated
                .materials
                .issues
                .iter()
                .any(|s| s.contains("unrotated"))
        );
        json["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"]["KHR_texture_transform"] =
            serde_json::json!({"texCoord":1});
        let uv1 = parse(&json);
        assert_eq!(uv1.materials.count(), 0);
        assert!(uv1.materials.issues.iter().any(|s| s.contains("UV0")));
    }
    #[test]
    fn vrm1_emission_targets_preserve_authored_emission_strength() {
        let json = serde_json::json!({"asset":{"version":"2.0"},
        "materials":[{"extensions":{"KHR_materials_emissive_strength":{"emissiveStrength":3.0}}}],
        "extensions":{"VRMC_vrm":{"expressions":{"preset":{"happy":{"materialColorBinds":[
            {"material":0,"type":"emissionColor","targetValue":[0.2,0.4,0.6,1.]}
        ]}}}}}});
        let parsed = ExpressionHandler::parse(
            &gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&json).unwrap())
                .unwrap(),
        );
        let target = parsed.materials.binds[&0].binds[0].target;
        for (actual, expected) in target.into_iter().zip([0.6, 1.2, 1.8, 1.]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }
    #[test]
    fn material_only_binary_and_override_channels_use_the_shared_weight_rules() {
        let parsed = ExpressionHandler::parse(&material_fixture(
            serde_json::json!({
                "happy":{"isBinary":true,"overrideBlink":"block","overrideMouth":"blend","overrideLookAt":"block",
                    "materialColorBinds":[{"material":0,"type":"color","targetValue":[1.,0.,0.,1.]}]},
                "blink":{"materialColorBinds":[{"material":0,"type":"color","targetValue":[1.,1.,1.,1.]}]},
                "aa":{"materialColorBinds":[{"material":0,"type":"emissionColor","targetValue":[1.,1.,1.,1.]}]},
                "lookUp":{"materialColorBinds":[{"material":0,"type":"rimColor","targetValue":[1.,1.,1.,1.]}]}
            }),
            false,
        ));
        let config = FacialConfig {
            selected_expression: "happy".into(),
            expression_weight: 0.6,
            weights: BTreeMap::from([("lookUp".into(), 1.)]),
            ..default()
        };
        let values = expression_weights(&parsed.expressions, &config, 1., 0.8, None);
        for (meta, value) in parsed.expressions.iter().zip(values) {
            assert_eq!(
                value,
                if meta.name == "happy" { 1. } else { 0. },
                "{}",
                meta.name
            );
        }
    }
    #[test]
    fn material_only_expressions_reach_private_variants_scene_members_and_restore() {
        use crate::shaders::{ShaderMode, ShaderPreview, ShaderPreviewPlugin, ShaderSettings};
        use bevy_shader_mtoon::MtoonMaterial;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Assets::<MtoonMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(Assets::<bevy_shader_mtoon::OutlineMaterial>::default())
            .init_resource::<AvatarLoadState>()
            .init_resource::<crate::visuals::VisualState>()
            .add_plugins((ExpressionsPlugin, ShaderPreviewPlugin));
        let original = MtoonMaterial {
            base_color: Color::linear_rgba(0.5, 0.5, 0.5, 1.),
            emissive_factor: Color::linear_rgb(0.1, 0.2, 0.3),
            shade_factor: Color::linear_rgb(0.2, 0.3, 0.4),
            matcap_factor: Vec3::new(0.2, 0.3, 0.4),
            parametric_rim_color: Color::linear_rgb(0.3, 0.4, 0.5),
            outline_color: Color::linear_rgb(0.1, 0.2, 0.3),
            ..default()
        };
        let source = app
            .world_mut()
            .resource_mut::<Assets<MtoonMaterial>>()
            .add(original.clone());
        let parsed = ExpressionHandler::parse(&material_fixture(all_material_binds(), false));
        let mut occupants = Vec::new();
        for character in [1, 2] {
            let root = app
                .world_mut()
                .spawn((
                    ShaderPreview,
                    SceneMemberFacial {
                        character: Uuid::from_u128(character),
                        config: FacialConfig {
                            weights: BTreeMap::from([("tint".into(), 0.25)]),
                            auto_blink: false,
                            ..default()
                        },
                    },
                    ExpressionCatalog {
                        expressions: parsed.expressions.clone(),
                        material_issues: parsed.materials.issues.iter().cloned().collect(),
                    },
                ))
                .id();
            let mesh = app
                .world_mut()
                .spawn((
                    ChildOf(root),
                    Mesh3d(Handle::default()),
                    MeshMaterial3d(source.clone()),
                    parsed.materials.binds[&0].clone(),
                ))
                .id();
            occupants.push((root, mesh));
        }
        let (first, mesh) = occupants[0];
        let (second, other_mesh) = occupants[1];
        let outline_handle = app
            .world_mut()
            .resource_mut::<Assets<bevy_shader_mtoon::OutlineMaterial>>()
            .add(bevy_shader_mtoon::OutlineMaterial {
                color: original.outline_color,
                width: original.outline_width,
                mode: original.outline_mode,
            });
        let outline = app
            .world_mut()
            .spawn((
                ChildOf(mesh),
                MeshMaterial3d(outline_handle.clone()),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(first);
        app.world_mut().resource_mut::<FacialState>().config = FacialConfig {
            weights: BTreeMap::from([("tint".into(), 0.5), ("other".into(), 0.25)]),
            auto_blink: false,
            ..default()
        };
        let color = |app: &App, mesh| {
            if let Some(handle) = app.world().get::<MeshMaterial3d<MtoonMaterial>>(mesh) {
                app.world()
                    .resource::<Assets<MtoonMaterial>>()
                    .get(&handle.0)
                    .unwrap()
                    .base_color
            } else {
                let handle = app
                    .world()
                    .get::<MeshMaterial3d<StandardMaterial>>(mesh)
                    .unwrap();
                app.world()
                    .resource::<Assets<StandardMaterial>>()
                    .get(&handle.0)
                    .unwrap()
                    .base_color
            }
            .to_linear()
            .to_f32_array()
        };
        let close = |actual: [f32; 4], expected: [f32; 4]| {
            for (a, e) in actual.into_iter().zip(expected) {
                assert!((a - e).abs() < 1e-5, "{actual:?} != {expected:?}");
            }
        };
        for _ in 0..3 {
            app.update();
            close(color(&app, mesh), [0.6, 0.5, 0.35, 0.65]);
        }
        close(color(&app, other_mesh), [0.6, 0.45, 0.4, 0.85]);
        let outline_color = app
            .world()
            .resource::<Assets<bevy_shader_mtoon::OutlineMaterial>>()
            .get(&outline_handle)
            .unwrap()
            .color
            .to_linear();
        assert!((outline_color.red - 0.5).abs() < 1e-5);
        assert_eq!(outline_color.alpha, 1.);
        assert!(
            app.world()
                .resource::<FacialState>()
                .status
                .contains("Toon has no UV")
        );
        let first_handle = app
            .world()
            .get::<MeshMaterial3d<MtoonMaterial>>(mesh)
            .unwrap()
            .0
            .clone();
        let other_handle = app
            .world()
            .get::<MeshMaterial3d<MtoonMaterial>>(other_mesh)
            .unwrap()
            .0
            .clone();
        assert_ne!(first_handle, other_handle);
        {
            let mut visuals = app
                .world_mut()
                .resource_mut::<crate::visuals::VisualState>();
            visuals.settings.material.matcap_strength = 2.;
            visuals.settings.material.emission_strength = 3.;
            visuals.settings.material.mtoon = true;
        }
        app.update();
        let toon = app
            .world()
            .resource::<Assets<MtoonMaterial>>()
            .get(&first_handle)
            .unwrap();
        assert!((toon.emissive_factor.to_linear().red - 1.5).abs() < 1e-5);
        assert!((toon.matcap_factor.x - 1.1).abs() < 1e-5);
        // Tuning's shade baseline changes without swallowing the authored delta.
        let baseline = Color::srgb_from_array(
            app.world()
                .resource::<crate::visuals::VisualState>()
                .settings
                .material
                .shade_color,
        )
        .to_linear()
        .red;
        assert!((toon.shade_factor.to_linear().red - (baseline + 0.35)).abs() < 1e-5);
        for mode in [ShaderMode::Lit, ShaderMode::Unlit] {
            app.world_mut().resource_mut::<ShaderSettings>().mode = mode;
            app.update();
            close(color(&app, mesh), [0.6, 0.5, 0.35, 0.65]);
            let handle = &app
                .world()
                .get::<MeshMaterial3d<StandardMaterial>>(mesh)
                .unwrap()
                .0;
            let standard = app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .get(handle)
                .unwrap();
            assert!((standard.emissive.red - 1.5).abs() < 1e-5);
            assert_eq!(standard.uv_transform.matrix2.x_axis.x, 3.);
            assert_eq!(
                app.world().get::<Visibility>(outline),
                Some(&Visibility::Hidden)
            );
            assert!((standard.uv_transform.translation.x - 0.2).abs() < 1e-5);
        }
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .weights
            .clear();
        app.world_mut()
            .resource_mut::<crate::visuals::VisualState>()
            .settings
            .material = default();
        app.world_mut().resource_mut::<ShaderSettings>().mode = ShaderMode::Toon;
        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<MtoonMaterial>>()
                .get(&first_handle),
            Some(&original)
        );
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(second);
        app.update();
        close(color(&app, mesh), [0.6, 0.45, 0.4, 0.85]);
        assert_eq!(
            app.world()
                .resource::<Assets<MtoonMaterial>>()
                .get(&other_handle),
            Some(&original)
        );
        app.world_mut()
            .entity_mut(first)
            .remove::<SceneMemberFacial>();
        app.world_mut()
            .entity_mut(second)
            .remove::<SceneMemberFacial>();
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<MtoonMaterial>>()
                .get(&first_handle),
            Some(&original)
        );
        assert_eq!(
            app.world().resource::<Assets<MtoonMaterial>>().get(&source),
            Some(&original)
        );
        app.world_mut().despawn(first);
        app.world_mut().despawn(second);
        app.update();
        assert!(
            app.world()
                .resource::<MaterialExpressionFrame>()
                .0
                .is_empty()
        );
    }
    #[test]
    fn ww4_blink_has_fast_cosine_close_hold_and_slower_open() {
        let close = BLINK_CLOSE_SECONDS;
        let open_at = close + BLINK_HOLD_SECONDS;
        for (time, expected) in [
            (-0.001, 0.0),
            (0.0, 0.0),
            (close / 2.0, 0.5),
            (close, 1.0),
            (close + BLINK_HOLD_SECONDS / 2.0, 1.0),
            (open_at, 1.0),
            (open_at + BLINK_OPEN_SECONDS / 2.0, 0.5),
            (BLINK_TOTAL_SECONDS, 0.0),
            (BLINK_TOTAL_SECONDS + 0.001, 0.0),
        ] {
            assert!((blink_curve(time) - expected).abs() < 0.00001);
        }
        for step in 1..=100 {
            let t = step as f32 / 100.0;
            assert!(blink_curve(close * t) >= blink_curve(close * (t - 0.01)));
            assert!(
                blink_curve(open_at + BLINK_OPEN_SECONDS * t)
                    <= blink_curve(open_at + BLINK_OPEN_SECONDS * (t - 0.01))
            );
        }
        const { assert!(BLINK_OPEN_SECONDS > BLINK_CLOSE_SECONDS * 2.0) };
        assert_eq!(FacialConfig::default().blink_interval_secs, 4.2);
    }
    #[test]
    fn blink_jitter_is_bounded_repeatable_and_changes_per_character_and_blink() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let sequence = |character| {
            (0..64)
                .map(|ordinal| blink_delay(character, ordinal, DEFAULT_BLINK_INTERVAL))
                .collect::<Vec<_>>()
        };
        assert_eq!(sequence(first), sequence(first));
        assert_ne!(sequence(first), sequence(second));
        assert!(sequence(first).windows(2).any(|pair| pair[0] != pair[1]));
        for interval in [f32::NAN, f32::INFINITY, -1.0, 1.5, 4.2, 10.0, 100.0] {
            let safe = blink_interval(interval);
            for ordinal in 0..100 {
                let delay = blink_delay(first, ordinal, interval);
                assert!(delay >= (safe * 0.7).clamp(1.0, 12.0));
                assert!(delay <= (safe * 1.4).clamp(1.0, 12.0));
            }
        }
    }
    #[test]
    fn blink_timers_are_independent_and_reset_on_character_setting_changes() {
        let character = Uuid::from_u128(1);
        let mut config = FacialConfig::default();
        let mut first = BlinkRuntime::default();
        let mut second = BlinkRuntime::default();
        first.update(0.0, character, &config);
        second.update(0.0, Uuid::from_u128(2), &config);
        let second_next = second.next_at;
        for _ in 0..1000 {
            first.update(0.05, character, &config);
        }
        assert_eq!(second.elapsed, 0.0);
        assert_eq!(second.ordinal, 0);
        assert_eq!(second.next_at, second_next);
        assert!(first.ordinal > 1);

        config.auto_blink = false;
        assert_eq!(first.update(0.05, character, &config), 0.0);
        assert_eq!(first.elapsed, 0.0);
        config.auto_blink = true;
        assert_eq!(first.update(0.0, character, &config), 0.0);
        assert_eq!(first.ordinal, 0);
        config.blink_interval_secs = 7.0;
        first.update(0.0, character, &config);
        assert_eq!(first.next_at, blink_delay(character, 0, 7.0));
        let replacement = Uuid::from_u128(3);
        first.update(0.0, replacement, &config);
        assert_eq!(first.next_at, blink_delay(replacement, 0, 7.0));
        assert_eq!(first.elapsed, 0.0);
    }
    #[test]
    fn blink_cadence_carries_frame_remainder_across_updates() {
        let mut sixty = BlinkRuntime::default();
        let mut one_twenty = BlinkRuntime::default();
        let config = FacialConfig::default();
        let character = Uuid::from_u128(42);
        for _ in 0..3600 {
            sixty.update(1.0 / 60.0, character, &config);
            one_twenty.update(1.0 / 120.0, character, &config);
            one_twenty.update(1.0 / 120.0, character, &config);
        }
        assert_eq!(sixty.ordinal, one_twenty.ordinal);
        assert_eq!(sixty.next_at, one_twenty.next_at);
        assert!((sixty.elapsed - one_twenty.elapsed).abs() < 0.002);
    }
    fn scene_face(app: &mut App, character: Uuid, config: FacialConfig) -> (Entity, Entity) {
        let root = app
            .world_mut()
            .spawn((
                SceneMemberFacial { character, config },
                ExpressionCatalog {
                    material_issues: vec![],
                    expressions: vec![
                        ExpressionMeta {
                            name: "aa".into(),
                            ..default()
                        },
                        ExpressionMeta {
                            name: "happy".into(),
                            ..default()
                        },
                    ],
                },
            ))
            .id();
        let node = app
            .world_mut()
            .spawn((
                ChildOf(root),
                ExpressionNode {
                    binds: vec![
                        NodeBind {
                            expression: 0,
                            index: 0,
                            weight: 1.0,
                        },
                        NodeBind {
                            expression: 1,
                            index: 1,
                            weight: 1.0,
                        },
                    ],
                },
                MorphWeights::new(vec![0.0, 0.0, 0.73], None).unwrap(),
            ))
            .id();
        (root, node)
    }
    #[test]
    fn cached_primitives_are_root_scoped_and_tolerate_removal() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ExpressionsPlugin)
            .init_resource::<AvatarLoadState>();
        let (first, first_node) = scene_face(&mut app, Uuid::from_u128(4), FacialConfig::default());
        let (second, second_node) = scene_face(
            &mut app,
            Uuid::from_u128(5),
            FacialConfig {
                selected_expression: "happy".into(),
                expression_weight: 0.4,
                ..default()
            },
        );
        let first_mesh = app
            .world_mut()
            .spawn((
                ChildOf(first_node),
                MeshMorphWeights::Value {
                    weights: vec![0.0, 0.0, 0.9],
                },
            ))
            .id();
        let second_mesh = app
            .world_mut()
            .spawn((
                ChildOf(second_node),
                MeshMorphWeights::Value {
                    weights: vec![0.0, 0.0, 0.8],
                },
            ))
            .id();
        let unrelated = app
            .world_mut()
            .spawn((
                ChildOf(second),
                MeshMorphWeights::Value {
                    weights: vec![0.7, 0.6, 0.5],
                },
            ))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(first);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .selected_expression = "happy".into();
        app.update();
        let weights = |app: &App, entity| match app.world().get::<MeshMorphWeights>(entity).unwrap()
        {
            MeshMorphWeights::Value { weights } => weights.clone(),
            _ => panic!("fixture has valued morphs"),
        };
        assert_eq!(weights(&app, first_mesh), vec![0.0, 1.0, 0.9]);
        assert_eq!(weights(&app, second_mesh), vec![0.0, 0.4, 0.8]);
        assert_eq!(weights(&app, unrelated), vec![0.7, 0.6, 0.5]);
        assert_eq!(
            app.world().resource::<FacialRuntime>().nodes[0].primitives,
            vec![first_mesh]
        );
        app.world_mut().despawn(first_mesh);
        app.world_mut()
            .entity_mut(second)
            .remove::<SceneMemberFacial>();
        app.update();
        assert_eq!(weights(&app, second_mesh), vec![0.0, 0.0, 0.8]);
        assert_eq!(weights(&app, unrelated), vec![0.7, 0.6, 0.5]);
    }

    #[test]
    fn consumed_audio_drives_morphs_with_legacy_neutral_sliders_and_closes_on_stop() {
        use crate::metered_audio::{SpeechMeter, sync_speech_envelope};
        use bevy::audio::Decodable;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ExpressionsPlugin)
            .init_resource::<AvatarLoadState>()
            .init_resource::<SpeechMeter>()
            .add_systems(PostUpdate, sync_speech_envelope.before(ExpressionSystems))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(100),
            ));
        let config = FacialConfig {
            weights: BTreeMap::from([("aa".into(), 0.0)]),
            ..default()
        };
        app.world_mut().resource_mut::<FacialState>().config = config.clone();
        let (root, node) = scene_face(&mut app, Uuid::from_u128(9), config);
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        let mut wave = Vec::new();
        wave.extend(b"RIFF");
        wave.extend(676u32.to_le_bytes());
        wave.extend(b"WAVEfmt ");
        wave.extend(16u32.to_le_bytes());
        wave.extend(1u16.to_le_bytes());
        wave.extend(1u16.to_le_bytes());
        wave.extend(8_000u32.to_le_bytes());
        wave.extend(16_000u32.to_le_bytes());
        wave.extend(2u16.to_le_bytes());
        wave.extend(16u16.to_le_bytes());
        wave.extend(b"data");
        wave.extend(640u32.to_le_bytes());
        for _ in 0..320 {
            wave.extend(16_384i16.to_le_bytes());
        }
        let meter = app.world().resource::<SpeechMeter>().clone();
        let speech = meter.buffered(AudioSource { bytes: wave.into() });
        let mut decoder = speech.decoder();
        for _ in 0..160 {
            assert_eq!(decoder.next(), Some(0.5));
        }
        app.update();
        assert!(app.world().get::<MorphWeights>(node).unwrap().weights()[0] > 0.5);
        meter.stop();
        for _ in 0..20 {
            app.update();
        }
        assert!(app.world().get::<MorphWeights>(node).unwrap().weights()[0] < 0.001);
    }
    #[test]
    fn scene_faces_isolate_speakers_manual_weights_and_restore_removed_members() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ExpressionsPlugin)
            .init_resource::<AvatarLoadState>()
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(100),
            ));
        let focused = Uuid::from_u128(1);
        let companion = Uuid::from_u128(2);
        let (first, first_node) = scene_face(&mut app, focused, FacialConfig::default());
        let (second, second_node) = scene_face(
            &mut app,
            companion,
            FacialConfig {
                selected_expression: "happy".into(),
                expression_weight: 0.4,
                ..default()
            },
        );
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(first);
        app.update();
        app.world().resource::<AudioEnvelope>().set(1.0);
        app.update();
        assert!(
            app.world()
                .get::<MorphWeights>(first_node)
                .unwrap()
                .weights()[0]
                > 0.5
        );
        assert_eq!(
            app.world()
                .get::<MorphWeights>(second_node)
                .unwrap()
                .weights(),
            &[0.0, 0.4, 0.73]
        );
        app.world_mut().resource_mut::<SpeechTarget>().0 = Some(companion);
        app.world().resource::<AudioEnvelope>().set(1.0);
        app.update();
        assert_eq!(
            app.world()
                .get::<MorphWeights>(first_node)
                .unwrap()
                .weights()[0],
            0.0
        );
        assert!(
            app.world()
                .get::<MorphWeights>(second_node)
                .unwrap()
                .weights()[0]
                > 0.5
        );
        app.world_mut()
            .get_mut::<SceneMemberFacial>(second)
            .unwrap()
            .config
            .weights
            .insert("aa".into(), 0.3);
        app.update();
        assert_eq!(
            app.world()
                .get::<MorphWeights>(second_node)
                .unwrap()
                .weights(),
            &[0.3, 0.4, 0.73]
        );
        app.world_mut()
            .entity_mut(second)
            .remove::<SceneMemberFacial>();
        app.update();
        assert_eq!(
            app.world()
                .get::<MorphWeights>(second_node)
                .unwrap()
                .weights(),
            &[0.0, 0.0, 0.73]
        );
    }
    #[test]
    fn companion_to_focused_transition_does_not_capture_expression_as_baseline() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ExpressionsPlugin)
            .init_resource::<AvatarLoadState>();
        let (root, node) = scene_face(
            &mut app,
            Uuid::from_u128(3),
            FacialConfig {
                selected_expression: "happy".into(),
                ..default()
            },
        );
        app.update();
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights()[1],
            1.0
        );
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights(),
            &[0.0, 0.0, 0.73]
        );
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights()[1],
            1.0
        );
        assert_ne!(
            blink_delay(Uuid::from_u128(1), 0, DEFAULT_BLINK_INTERVAL),
            blink_delay(Uuid::from_u128(2), 0, DEFAULT_BLINK_INTERVAL)
        );
    }

    fn fixture(vrm0: bool) -> gltf::Gltf {
        let ext = if vrm0 {
            serde_json::json!({"VRM":{"blendShapeMaster":{"blendShapeGroups":[{"presetName":"a","binds":[{"mesh":0,"index":0,"weight":50}]}]}}})
        } else {
            serde_json::json!({"VRMC_vrm":{"expressions":{"preset":{"aa":{"morphTargetBinds":[{"node":1,"index":0,"weight":0.5}]}}}}})
        };
        let json = serde_json::json!({"asset":{"version":"2.0"},"extensions":ext,"nodes":[{"mesh":0},{"mesh":0}],"meshes":[{"primitives":[{"attributes":{},"targets":[{}]}]}]});
        gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&json).unwrap()).unwrap()
    }
    #[test]
    fn vrm0_mesh_binding_reaches_both_instances_but_vrm1_node_is_exact() {
        let zero = ExpressionHandler::parse(&fixture(true));
        let one = ExpressionHandler::parse(&fixture(false));
        assert_eq!(
            zero.binds.iter().map(|b| b.node).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            one.binds.iter().map(|b| b.node).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(zero.binds[0].bind.weight, 0.5);
        assert_eq!(zero.expressions[0].name, "aa");
    }
    #[test]
    fn compose_preserves_unowned_channels_and_resets_previous_owned_weight() {
        let node = CachedNode {
            entity: Entity::PLACEHOLDER,
            primitives: vec![],
            baseline: vec![(1, 0.0)],
            binds: vec![NodeBind {
                expression: 0,
                index: 1,
                weight: 0.7,
            }],
        };
        let mut weights = [0.3, 0.0, 0.9];
        compose(&mut weights, &node, &[1.0]);
        assert_eq!(weights, [0.3, 0.7, 0.9]);
        compose(&mut weights, &node, &[0.0]);
        assert_eq!(weights, [0.3, 0.0, 0.9]);
    }
    #[test]
    fn binary_and_override_rules_apply_to_automatic_face_inputs() {
        let expressions = vec![
            ExpressionMeta {
                name: "happy".into(),
                binary: true,
                blink_override: 1,
                mouth_override: 2,
                ..default()
            },
            ExpressionMeta {
                name: "blink".into(),
                ..default()
            },
            ExpressionMeta {
                name: "aa".into(),
                ..default()
            },
        ];
        let config = FacialConfig {
            selected_expression: "happy".into(),
            expression_weight: 0.6,
            ..default()
        };
        assert_eq!(
            expression_weights(&expressions, &config, 1.0, 0.8, None),
            vec![1.0, 0.0, 0.0]
        );
    }
    #[test]
    fn five_vowels_follow_vrm_names_manual_weights_and_override_rules() {
        let mut expressions: Vec<_> = ["aa", "ih", "ou", "ee", "oh"]
            .into_iter()
            .map(|name| ExpressionMeta {
                name: name.into(),
                ..default()
            })
            .collect();
        let mut config = FacialConfig::default();
        let vowels = [0.1, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(
            expression_weights(&expressions, &config, 0.0, 0.9, Some(vowels)),
            vowels
        );
        config.weights.insert("ih".into(), 0.6);
        config.selected_expression = "oh".into();
        config.expression_weight = 0.7;
        assert_eq!(
            expression_weights(&expressions, &config, 0.0, 0.9, Some(vowels)),
            [0.1, 0.6, 0.3, 0.4, 0.7]
        );
        expressions.push(ExpressionMeta {
            name: "happy".into(),
            mouth_override: 1,
            ..default()
        });
        config.weights.insert("happy".into(), 1.0);
        assert_eq!(
            expression_weights(&expressions, &config, 0.0, 0.9, Some(vowels)),
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
        );
        config.weights.clear();
        config.selected_expression.clear();
        config.lip_sync = false;
        assert_eq!(
            expression_weights(&expressions, &config, 0.0, 0.9, Some(vowels)),
            [0.0; 6]
        );
    }
    #[test]
    fn calibrated_vowels_reach_focused_morphs_only_for_the_speaker_and_respect_overrides() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ExpressionsPlugin)
            .init_resource::<AvatarLoadState>()
            .init_resource::<crate::viseme_audio::VisemeState>()
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(100),
            ));
        let character = Uuid::from_u128(5);
        let root = app
            .world_mut()
            .spawn((
                SceneMemberFacial {
                    character,
                    config: FacialConfig::default(),
                },
                ExpressionCatalog {
                    material_issues: vec![],
                    expressions: vec![
                        ExpressionMeta {
                            name: "aa".into(),
                            ..default()
                        },
                        ExpressionMeta {
                            name: "ih".into(),
                            ..default()
                        },
                        ExpressionMeta {
                            name: "happy".into(),
                            mouth_override: 1,
                            ..default()
                        },
                    ],
                },
            ))
            .id();
        let node = app
            .world_mut()
            .spawn((
                ChildOf(root),
                ExpressionNode {
                    binds: (0..3)
                        .map(|i| NodeBind {
                            expression: i,
                            index: i,
                            weight: 1.0,
                        })
                        .collect(),
                },
                MorphWeights::new(vec![0.0; 3], None).unwrap(),
            ))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let morphs = |app: &App| {
            app.world()
                .get::<MorphWeights>(node)
                .unwrap()
                .weights()
                .to_vec()
        };
        let publish = |app: &mut App| {
            app.world_mut()
                .resource_mut::<crate::viseme_audio::VisemeState>()
                .publish_for_test([0.4, 0.2, 0.0, 0.0, 0.0]);
        };
        app.world_mut().resource_mut::<SpeechTarget>().0 = Some(character);
        publish(&mut app);
        app.update();
        let open = morphs(&app);
        assert!(
            (open[0] - 0.8).abs() < 0.001 && (open[1] - 0.4).abs() < 0.001,
            "{open:?}"
        );
        assert_eq!(open[2], 0.0);

        app.world_mut().resource_mut::<SpeechTarget>().0 = Some(Uuid::from_u128(6));
        publish(&mut app);
        app.update();
        assert_eq!(
            morphs(&app),
            vec![0.0; 3],
            "another speaker must not move this mouth"
        );

        app.world_mut().resource_mut::<SpeechTarget>().0 = Some(character);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .selected_expression = "happy".into();
        publish(&mut app);
        app.update();
        assert_eq!(
            morphs(&app),
            vec![0.0, 0.0, 1.0],
            "a selected mouth-blocking expression silences vowels"
        );

        {
            let mut facial = app.world_mut().resource_mut::<FacialState>();
            facial.config.selected_expression.clear();
            facial.config.weights.insert("aa".into(), 0.3);
        }
        publish(&mut app);
        app.update();
        let manual = morphs(&app);
        assert!(
            (manual[0] - 0.3).abs() < 0.001 && (manual[1] - 0.4).abs() < 0.001,
            "{manual:?}"
        );

        {
            let mut facial = app.world_mut().resource_mut::<FacialState>();
            facial.config.weights.clear();
            facial.config.lip_sync = false;
        }
        publish(&mut app);
        app.update();
        assert_eq!(morphs(&app), vec![0.0; 3]);
    }
    #[test]
    fn unavailable_visemes_fall_back_to_selected_amplitude_mouth() {
        let expressions: Vec<_> = ["aa", "ih", "custom"]
            .into_iter()
            .map(|name| ExpressionMeta {
                name: name.into(),
                ..default()
            })
            .collect();
        let config = FacialConfig {
            mouth_expression: "custom".into(),
            ..default()
        };
        assert_eq!(
            expression_weights(&expressions, &config, 0.0, 0.8, None),
            [0.0, 0.0, 0.8]
        );
    }
    #[test]
    fn legacy_zero_manual_sliders_leave_automatic_speech_and_blink_active() {
        let expressions: Vec<_> = ["aa", "ih", "ou", "ee", "oh", "blink"]
            .into_iter()
            .map(|name| ExpressionMeta {
                name: name.into(),
                ..default()
            })
            .collect();
        let mut config = FacialConfig {
            weights: expressions.iter().map(|e| (e.name.clone(), 0.0)).collect(),
            ..default()
        };
        assert_eq!(
            expression_weights(&expressions, &config, 0.7, 0.8, None),
            [0.8, 0.0, 0.0, 0.0, 0.0, 0.7]
        );
        let vowels = [0.1, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(
            expression_weights(&expressions, &config, 0.7, 0.8, Some(vowels)),
            [0.1, 0.2, 0.3, 0.4, 0.5, 0.7]
        );
        config.selected_expression = "aa".into();
        config.expression_weight = 0.0;
        assert_eq!(
            expression_weights(&expressions, &config, 0.0, 0.8, None),
            [0.0; 6]
        );
    }
    #[test]
    fn behavior_blend_never_touches_reserved_shared_custom_mouth_or_override_morphs() {
        let expressions = vec![
            ExpressionMeta {
                name: "happy".into(),
                ..default()
            },
            ExpressionMeta {
                name: "blink".into(),
                ..default()
            },
            ExpressionMeta {
                name: "aa".into(),
                ..default()
            },
            ExpressionMeta {
                name: "customJaw".into(),
                ..default()
            },
            ExpressionMeta {
                name: "overrideEmotion".into(),
                mouth_override: 1,
                ..default()
            },
            ExpressionMeta {
                name: "manualEmotion".into(),
                ..default()
            },
        ];
        let binds = [
            (0, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (0, 5),
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 5),
        ]
        .into_iter()
        .map(|(expression, index)| NodeBind {
            expression,
            index,
            weight: 1.0,
        })
        .collect();
        let node = CachedNode {
            entity: Entity::PLACEHOLDER,
            primitives: vec![],
            binds,
            baseline: vec![],
        };
        let config = FacialConfig {
            mouth_expression: "customJaw".into(),
            weights: BTreeMap::from([("manualEmotion".into(), 0.5)]),
            ..default()
        };
        let automatic = BTreeMap::from([
            ("happy".into(), 0.6),
            ("customJaw".into(), 1.0),
            ("overrideEmotion".into(), 1.0),
        ]);
        let mut weights = [0.0, 0.2, 0.3, 0.4, 0.5, 0.6];
        compose_behavior(&mut weights, &node, &expressions, &config, &automatic);
        assert_eq!(weights, [0.6, 0.2, 0.3, 0.4, 0.5, 0.6]);
    }
    #[test]
    fn behavior_binary_threshold_and_selected_manual_expression_are_preserved() {
        let expressions = vec![ExpressionMeta {
            name: "happy".into(),
            binary: true,
            ..default()
        }];
        let node = CachedNode {
            entity: Entity::PLACEHOLDER,
            primitives: vec![],
            binds: vec![NodeBind {
                expression: 0,
                index: 0,
                weight: 0.8,
            }],
            baseline: vec![],
        };
        let mut weights = [0.1];
        compose_behavior(
            &mut weights,
            &node,
            &expressions,
            &FacialConfig::default(),
            &BTreeMap::from([("happy".into(), 0.49)]),
        );
        assert_eq!(weights, [0.1]);
        compose_behavior(
            &mut weights,
            &node,
            &expressions,
            &FacialConfig::default(),
            &BTreeMap::from([("happy".into(), 0.5)]),
        );
        assert!((weights[0] - 0.9).abs() < 1e-6);
        let config = FacialConfig {
            selected_expression: "happy".into(),
            ..default()
        };
        compose_behavior(
            &mut weights,
            &node,
            &expressions,
            &config,
            &BTreeMap::from([("happy".into(), 1.0)]),
        );
        assert!((weights[0] - 0.9).abs() < 1e-6);
    }
    #[test]
    fn envelope_is_shared_finite_and_silences() {
        let input = AudioEnvelope::default();
        let worker = input.clone();
        worker.set(f32::NAN);
        assert_eq!(input.level(), 0.0);
        worker.set(2.0);
        assert_eq!(input.level(), 1.0);
        worker.set_pcm(&[0.5, -0.5]);
        assert_eq!(input.level(), 0.5);
        input.silence();
        assert_eq!(worker.level(), 0.0);
    }
    #[test]
    fn delayed_scene_bindings_retry_and_missing_catalog_search_is_bounded() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .init_resource::<FacialState>()
            .init_resource::<AudioEnvelope>()
            .init_resource::<FacialRuntime>()
            .add_systems(PostUpdate, update_expressions);
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        assert!(!app.world().resource::<FacialRuntime>().bindings_ready);
        app.world_mut().entity_mut(root).insert(ExpressionCatalog {
            material_issues: vec![],
            expressions: vec![ExpressionMeta {
                name: "happy".into(),
                ..default()
            }],
        });
        let node = app
            .world_mut()
            .spawn((
                ChildOf(root),
                ExpressionNode {
                    binds: vec![NodeBind {
                        expression: 0,
                        index: 0,
                        weight: 1.0,
                    }],
                },
                MorphWeights::new(vec![0.0], None).unwrap(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .selected_expression = "happy".into();
        app.update();
        assert!(app.world().resource::<FacialRuntime>().bindings_ready);
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights(),
            &[1.0]
        );
        let missing = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(missing);
        for _ in 0..MAX_BINDING_WAIT_FRAMES as usize + 5 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<FacialRuntime>().binding_wait_frames,
            MAX_BINDING_WAIT_FRAMES
        );
        assert!(app.world().resource::<FacialRuntime>().nodes.is_empty());
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights(),
            &[0.0]
        );
    }
    #[test]
    fn switching_avatar_restores_owned_channels_and_clears_cache() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .init_resource::<FacialState>()
            .init_resource::<AudioEnvelope>()
            .init_resource::<FacialRuntime>()
            .configure_sets(
                PostUpdate,
                ExpressionSystems.before(bevy::app::AnimationSystems),
            )
            .add_systems(PostUpdate, update_expressions.in_set(ExpressionSystems));
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((
            ChildOf(root),
            ExpressionCatalog {
                material_issues: vec![],
                expressions: vec![ExpressionMeta {
                    name: "happy".into(),
                    ..default()
                }],
            },
        ));
        let node = app
            .world_mut()
            .spawn((
                ChildOf(root),
                ExpressionNode {
                    binds: vec![NodeBind {
                        expression: 0,
                        index: 0,
                        weight: 1.0,
                    }],
                },
                MorphWeights::new(vec![0.0, 0.8], None).unwrap(),
            ))
            .id();
        let mesh = app
            .world_mut()
            .spawn((ChildOf(node), MeshMorphWeights::Reference(node)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .selected_expression = "happy".into();
        app.update();
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights(),
            &[1.0, 0.8]
        );
        assert!(
            matches!(app.world().get::<MeshMorphWeights>(mesh),Some(MeshMorphWeights::Reference(e)) if *e==node)
        );
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(
            app.world().get::<MorphWeights>(node).unwrap().weights(),
            &[0.0, 0.8]
        );
        assert!(app.world().resource::<FacialRuntime>().nodes.is_empty());
        assert!(app.world().resource::<FacialState>().available.is_empty());
    }
}
