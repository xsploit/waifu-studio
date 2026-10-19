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
    /// Ephemeral consumed-speech envelope for the focused avatar, before gain.
    pub speech_level: f32,
    /// Largest composed vowel/configured-mouth expression weight; not a
    /// measurement of rendered geometry or physical speaker synchronization.
    pub mouth_weight: f32,
}
/// Insert on each scene occupant root. Focused config still comes from FacialState.
#[derive(Component, Clone)]
pub struct SceneMemberFacial {
    pub character: Uuid,
    pub config: FacialConfig,
}
/// Ephemeral scene-reply emotion. The host supplies only validated, opted-in
/// metadata; the face runtime resolves aliases and protects all manual channels.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct SceneMemberEmotion {
    pub emotion: crate::reply_affect::AffectEmotion,
    pub strength: f32,
    pub smoothing_seconds: f32,
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
    look_at: Option<LookAtMeta>,
}

/// VRM look expression presets in the order used by [`LookAtMeta::expression_weights`].
pub const LOOK_EXPRESSIONS: [&str; 4] = ["lookLeft", "lookRight", "lookUp", "lookDown"];
fn is_look(name: &str) -> bool {
    LOOK_EXPRESSIONS.contains(&name)
}

/// Authored eye drive. `Unsupported` means the model asked for a mode this
/// runtime does not interpret, so nothing drives its eyes.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LookAtKind {
    #[default]
    Bone,
    Expression,
    Unsupported,
}

/// One non-weighted Unity AnimationCurve key, as flattened by VRM 0.x
/// (`[time, value, inTangent, outTangent]`).
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct CurveKey {
    pub time: f32,
    pub value: f32,
    pub in_tangent: f32,
    pub out_tangent: f32,
}

/// VRM 1 `rangeMap*` or VRM 0 `lookAt*` degree map. Input is the gaze angle
/// in degrees; output is degrees for bone look-at and a weight for expressions.
/// VRM 1 is linear (empty `curve`); VRM 0 evaluates its authored curve over
/// the normalized input like UniVRM's `CurveMapper`.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct LookRangeMap {
    pub input_max_degrees: f32,
    pub output_scale: f32,
    pub curve: Vec<CurveKey>,
}
impl LookRangeMap {
    fn linear(input_max_degrees: f32, output_scale: f32) -> Self {
        Self {
            input_max_degrees,
            output_scale,
            curve: Vec::new(),
        }
    }
    pub fn map(&self, degrees: f32) -> f32 {
        if !degrees.is_finite() || self.input_max_degrees <= 0. {
            return 0.;
        }
        let t = (degrees.max(0.) / self.input_max_degrees).min(1.);
        let shaped = if self.curve.is_empty() {
            t
        } else {
            evaluate_curve(&self.curve, t)
        };
        let value = shaped * self.output_scale;
        if value.is_finite() { value } else { 0. }
    }
}
/// Unity cubic Hermite evaluation with clamped ends. Keys are validated
/// strictly ascending and finite at parse time.
fn evaluate_curve(keys: &[CurveKey], t: f32) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return t;
    };
    if t <= first.time {
        return first.value;
    }
    if t >= last.time {
        return last.value;
    }
    let Some(pair) = keys.windows(2).find(|pair| t < pair[1].time) else {
        return last.value;
    };
    let (k0, k1) = (pair[0], pair[1]);
    let dt = k1.time - k0.time;
    let s = (t - k0.time) / dt;
    let (s2, s3) = (s * s, s * s * s);
    (2. * s3 - 3. * s2 + 1.) * k0.value
        + (s3 - 2. * s2 + s) * dt * k0.out_tangent
        + (-2. * s3 + 3. * s2) * k1.value
        + (s3 - s2) * dt * k1.in_tangent
}

/// Authored VRM look-at metadata of one avatar.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct LookAtMeta {
    pub kind: LookAtKind,
    pub vrm0: bool,
    pub horizontal_inner: LookRangeMap,
    pub horizontal_outer: LookRangeMap,
    pub vertical_down: LookRangeMap,
    pub vertical_up: LookRangeMap,
    /// Defaults applied or authored data that was not interpreted.
    pub issues: Vec<String>,
}
impl LookAtMeta {
    /// `left_degrees > 0` looks toward the character's left; `up_degrees > 0`
    /// looks up. Both eyes share the outer horizontal map, as in UniVRM's
    /// BlendShape applier and three-vrm's expression applier.
    pub fn expression_weights(&self, left_degrees: f32, up_degrees: f32) -> [f32; 4] {
        if self.kind != LookAtKind::Expression {
            return [0.; 4];
        }
        let side = |degrees: f32| unit(self.horizontal_outer.map(degrees));
        [
            if left_degrees > 0. {
                side(left_degrees)
            } else {
                0.
            },
            if left_degrees < 0. {
                side(-left_degrees)
            } else {
                0.
            },
            if up_degrees > 0. {
                unit(self.vertical_up.map(up_degrees))
            } else {
                0.
            },
            if up_degrees < 0. {
                unit(self.vertical_down.map(-up_degrees))
            } else {
                0.
            },
        ]
    }
    /// Bone look-at output (left, up) in degrees for one eye. Looking toward a
    /// side turns that side's eye outward (outer map) and the other inward.
    pub fn bone_eye_degrees(
        &self,
        left_eye: bool,
        left_degrees: f32,
        up_degrees: f32,
    ) -> (f32, f32) {
        if self.kind != LookAtKind::Bone || !left_degrees.is_finite() || !up_degrees.is_finite() {
            return (0., 0.);
        }
        let (toward_left, toward_right) = if left_eye {
            (&self.horizontal_outer, &self.horizontal_inner)
        } else {
            (&self.horizontal_inner, &self.horizontal_outer)
        };
        let yaw = if left_degrees >= 0. {
            toward_left.map(left_degrees)
        } else {
            -toward_right.map(-left_degrees)
        };
        let pitch = if up_degrees >= 0. {
            self.vertical_up.map(up_degrees)
        } else {
            -self.vertical_down.map(-up_degrees)
        };
        (yaw, pitch)
    }

    fn parse(vrm1: Option<&Value>, vrm0: Option<&Value>) -> Option<Self> {
        if let Some(vrm1) = vrm1 {
            return Self::parse_vrm1(vrm1.get("lookAt")?);
        }
        Self::parse_vrm0(vrm0?.get("firstPerson")?)
    }
    fn parse_vrm1(look: &Value) -> Option<Self> {
        let mut issues = Vec::new();
        let kind = match look.get("type").map(Value::as_str) {
            None => {
                issues.push("lookAt.type is missing; treated as bone".into());
                LookAtKind::Bone
            }
            Some(Some("bone")) => LookAtKind::Bone,
            Some(Some("expression")) => LookAtKind::Expression,
            Some(other) => {
                issues.push(format!("lookAt.type {other:?} is unsupported"));
                LookAtKind::Unsupported
            }
        };
        let default_output = if kind == LookAtKind::Expression {
            1.
        } else {
            10.
        };
        let mut map = |key: &str| {
            let Some(value) = look.get(key) else {
                issues.push(format!("{key} is missing; using 90° → {default_output}"));
                return LookRangeMap::linear(90., default_output);
            };
            let input = value.get("inputMaxValue").map(Value::as_f64);
            let output = value.get("outputScale").map(Value::as_f64);
            let input = match input {
                None => 90.,
                Some(Some(v)) if v.is_finite() && v > 0. && v <= 180. => v as f32,
                Some(_) => {
                    issues.push(format!("{key}.inputMaxValue is invalid; using 90°"));
                    90.
                }
            };
            let output = match output {
                None => default_output,
                Some(Some(v)) if v.is_finite() && v >= 0. => v as f32,
                Some(_) => {
                    issues.push(format!(
                        "{key}.outputScale is invalid; using {default_output}"
                    ));
                    default_output
                }
            };
            LookRangeMap::linear(input, output)
        };
        let horizontal_inner = map("rangeMapHorizontalInner");
        let horizontal_outer = map("rangeMapHorizontalOuter");
        let vertical_down = map("rangeMapVerticalDown");
        let vertical_up = map("rangeMapVerticalUp");
        Some(Self {
            kind,
            vrm0: false,
            horizontal_inner,
            horizontal_outer,
            vertical_down,
            vertical_up,
            issues,
        })
    }
    fn parse_vrm0(first_person: &Value) -> Option<Self> {
        let mut issues = Vec::new();
        let kind = match first_person.get("lookAtTypeName").map(Value::as_str) {
            None => {
                issues.push("lookAtTypeName is missing; treated as Bone".into());
                LookAtKind::Bone
            }
            Some(Some("Bone")) => LookAtKind::Bone,
            Some(Some("BlendShape")) => LookAtKind::Expression,
            Some(other) => {
                issues.push(format!("lookAtTypeName {other:?} is unsupported"));
                LookAtKind::Unsupported
            }
        };
        // UniVRM CurveMapper defaults: xRange 90; yRange 10° (bone) or 1 (BlendShape);
        // curve keys (0,0,0,1) and (1,1,1,0).
        let default_output = if kind == LookAtKind::Expression {
            1.
        } else {
            10.
        };
        let default_curve = vec![
            CurveKey {
                time: 0.,
                value: 0.,
                in_tangent: 0.,
                out_tangent: 1.,
            },
            CurveKey {
                time: 1.,
                value: 1.,
                in_tangent: 1.,
                out_tangent: 0.,
            },
        ];
        let mut map = |key: &str| {
            let Some(value) = first_person.get(key) else {
                issues.push(format!("{key} is missing; using UniVRM defaults"));
                return LookRangeMap {
                    curve: default_curve.clone(),
                    ..LookRangeMap::linear(90., default_output)
                };
            };
            let mut range = |field: &str, fallback: f32, positive: bool| match value
                .get(field)
                .map(Value::as_f64)
            {
                None => fallback,
                Some(Some(v)) if v.is_finite() && (if positive { v > 0. } else { v >= 0. }) => {
                    v as f32
                }
                Some(_) => {
                    issues.push(format!("{key}.{field} is invalid; using {fallback}"));
                    fallback
                }
            };
            let input = range("xRange", 90., true);
            let output = range("yRange", default_output, false);
            let curve = match value.get("curve") {
                None => default_curve.clone(),
                Some(curve) => parse_curve(curve).unwrap_or_else(|| {
                    issues.push(format!("{key}.curve is malformed; using linear"));
                    Vec::new()
                }),
            };
            LookRangeMap {
                input_max_degrees: input,
                output_scale: output,
                curve,
            }
        };
        let horizontal_inner = map("lookAtHorizontalInner");
        let horizontal_outer = map("lookAtHorizontalOuter");
        let vertical_down = map("lookAtVerticalDown");
        let vertical_up = map("lookAtVerticalUp");
        Some(Self {
            kind,
            vrm0: true,
            horizontal_inner,
            horizontal_outer,
            vertical_down,
            vertical_up,
            issues,
        })
    }
}
/// Accept non-empty, finite, strictly ascending 4-float keys. Weighted
/// tangents and non-clamp wrap modes are not part of the VRM 0 encoding.
fn parse_curve(value: &Value) -> Option<Vec<CurveKey>> {
    let numbers = value
        .as_array()?
        .iter()
        .map(|v| v.as_f64().map(|v| v as f32).filter(|v| v.is_finite()))
        .collect::<Option<Vec<_>>>()?;
    if numbers.is_empty() || numbers.len() % 4 != 0 || numbers.len() > 4 * 64 {
        return None;
    }
    let keys: Vec<_> = numbers
        .as_chunks::<4>()
        .0
        .iter()
        .map(|k| CurveKey {
            time: k[0],
            value: k[1],
            in_tangent: k[2],
            out_tangent: k[3],
        })
        .collect();
    keys.windows(2)
        .all(|pair| pair[0].time < pair[1].time)
        .then_some(keys)
}

/// Authored look-at metadata on the scene root, beside `ExpressionCatalog`.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Clone)]
struct AuthoredLookAt(LookAtMeta);

/// Gaze input for the selected avatar, in anatomical degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookInput {
    pub root: Entity,
    pub left_degrees: f32,
    pub up_degrees: f32,
    /// Applied after authored mapping, so strength does not distort its curve.
    pub strength: f32,
}
impl LookInput {
    fn weights(self, meta: &LookAtMeta) -> [f32; 4] {
        meta.expression_weights(self.left_degrees, self.up_degrees)
            .map(|weight| weight * unit(self.strength))
    }
}
/// Selected-avatar handoff. The face runtime publishes the active root's
/// metadata every frame; gaze writes `input` before the face runtime runs and
/// clears it whenever it does not want expression-driven eyes.
#[derive(Resource, Default, Clone, Debug)]
pub struct LookAtLink {
    pub root: Option<Entity>,
    /// The catalog was found, or its bounded search ended without one.
    pub resolved: bool,
    pub meta: Option<LookAtMeta>,
    /// Which of [`LOOK_EXPRESSIONS`] the active root defines.
    pub expressions: [bool; 4],
    pub input: Option<LookInput>,
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
        result.look_at = LookAtMeta::parse(
            gltf.extension_value("VRMC_vrm"),
            gltf.extension_value("VRM"),
        );
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
        if let Some(look_at) = &self.look_at {
            world
                .entity_mut(root)
                .insert(AuthoredLookAt(look_at.clone()));
        }
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
            .init_resource::<LookAtLink>()
            .register_type::<ExpressionNode>()
            .register_type::<ExpressionCatalog>()
            .register_type::<AuthoredLookAt>()
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
    scene_weights: BTreeMap<String, f32>,
    bindings_ready: bool,
    binding_wait_frames: u8,
    look_at: Option<LookAtMeta>,
}
fn scene_emotion_weights(
    cache: &mut FacialRuntime,
    emotion: Option<&SceneMemberEmotion>,
    dt: f32,
) -> BTreeMap<String, f32> {
    let Some(emotion) = emotion else {
        cache.scene_weights.clear();
        return BTreeMap::new();
    };
    if !emotion.strength.is_finite() || !emotion.smoothing_seconds.is_finite() {
        cache.scene_weights.clear();
        return BTreeMap::new();
    }
    let target = emotion.emotion.aliases().iter().find_map(|alias| {
        cache
            .expressions
            .iter()
            .find(|e| {
                e.name.eq_ignore_ascii_case(alias) && !crate::behavior::reserved_expression(&e.name)
            })
            .map(|e| e.name.clone())
    });
    if let Some(name) = &target {
        cache.scene_weights.entry(name.clone()).or_default();
    }
    let alpha = 1. - (-dt.max(0.) / emotion.smoothing_seconds.clamp(0.02, 5.)).exp();
    for (name, weight) in &mut cache.scene_weights {
        let desired = if Some(name) == target.as_ref() {
            emotion.strength.clamp(0., 0.65)
        } else {
            0.
        };
        *weight += (desired - *weight) * alpha;
    }
    cache.scene_weights.retain(|_, weight| *weight > 0.0001);
    cache.scene_weights.clone()
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
    expression_weights_with_look(expressions, config, blink, mouth, vowels, None)
}
/// `look` holds automatic gaze weights in [`LOOK_EXPRESSIONS`] order. Like
/// speech, an active manual weight owns its look channel, and other
/// expressions' `overrideLookAt` still blocks or blends the result.
fn expression_weights_with_look(
    expressions: &[ExpressionMeta],
    config: &FacialConfig,
    blink: f32,
    mouth: f32,
    vowels: Option<[f32; 5]>,
    look: Option<[f32; 4]>,
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
            if !manual
                && let Some(look) = look
                && let Some(index) = LOOK_EXPRESSIONS.iter().position(|name| *name == e.name)
            {
                w = w.max(unit(look[index]));
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
        } else if is_look(&e.name) {
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
    catalogs: &Query<(Entity, &ExpressionCatalog, Option<&AuthoredLookAt>)>,
    nodes: &Query<(Entity, &ExpressionNode)>,
    morphs: &Query<&mut MorphWeights>,
    meshes: &Query<(Entity, &ChildOf, &mut MeshMorphWeights)>,
) {
    if cache.bindings_ready || cache.binding_wait_frames >= MAX_BINDING_WAIT_FRAMES {
        return;
    }
    cache.binding_wait_frames += 1;
    let Some((_, catalog, _)) = catalogs
        .iter()
        .find(|(entity, _, _)| under_root(*entity, root, parents))
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
    catalogs: Query<(Entity, &ExpressionCatalog, Option<&AuthoredLookAt>)>,
    nodes: Query<(Entity, &ExpressionNode)>,
    mut morphs: Query<&mut MorphWeights>,
    mut meshes: Query<(Entity, &ChildOf, &mut MeshMorphWeights)>,
    members: Query<(Entity, &SceneMemberFacial, Option<&SceneMemberEmotion>)>,
    // Grouped to stay within Bevy's 16-parameter system limit.
    (speech, mut look_link): (Option<Res<SpeechTarget>>, Option<ResMut<LookAtLink>>),
    mut extras: Local<BTreeMap<Entity, FacialRuntime>>,
    mut material_frame: Option<ResMut<MaterialExpressionFrame>>,
) {
    state.speech_level = 0.;
    state.mouth_weight = 0.;
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
        .and_then(|root| {
            members
                .get(root)
                .ok()
                .map(|(_, member, _)| member.character)
        })
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
        if let Some((_, catalog, look_at)) = catalogs
            .iter()
            .find(|(entity, _, _)| under_root(*entity, root, &parents))
        {
            runtime.bindings_ready = true;
            runtime.expressions = catalog.expressions.clone();
            runtime.look_at = look_at.map(|look_at| look_at.0.clone());
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
    state.speech_level = if avatar.active.is_some() && focused_speaks {
        unit(envelope.level())
    } else {
        0.
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
    // Publish the selected root's look-at metadata for gaze; accept gaze input
    // only for this exact root so a stale entity can never drive another face.
    let look = look_link.as_deref_mut().and_then(|link| {
        link.root = avatar.active;
        link.resolved = avatar.active.is_some()
            && (runtime.bindings_ready || runtime.binding_wait_frames >= MAX_BINDING_WAIT_FRAMES);
        if link.meta != runtime.look_at {
            link.meta = runtime.look_at.clone();
        }
        link.expressions =
            LOOK_EXPRESSIONS.map(|name| runtime.expressions.iter().any(|e| e.name == name));
        let input = link
            .input
            .filter(|input| Some(input.root) == avatar.active)?;
        Some(input.weights(runtime.look_at.as_ref()?))
    });
    let values = expression_weights_with_look(
        &runtime.expressions,
        &state.config,
        blink,
        runtime.mouth,
        visemes
            .as_ref()
            .filter(|_| focused_speaks)
            .and_then(|state| state.weights())
            .map(|weights| weights.map(|value| unit(value * gain))),
        look,
    );
    state.mouth_weight = runtime
        .expressions
        .iter()
        .zip(&values)
        .filter(|(expression, _)| {
            is_mouth(&expression.name) || expression.name == state.config.mouth_expression
        })
        .map(|(_, value)| unit(*value))
        .fold(0., f32::max);
    let mut automatic = behavior
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
    let scene_emotion = avatar
        .active
        .and_then(|root| members.get(root).ok())
        .and_then(|(_, _, emotion)| emotion);
    for (name, weight) in scene_emotion_weights(&mut runtime, scene_emotion, dt) {
        automatic
            .entry(name)
            .and_modify(|value| *value = value.max(weight))
            .or_insert(weight);
    }
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
    for (root, member, emotion) in &members {
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
        let automatic = scene_emotion_weights(cache, emotion, dt);
        if let Some(frame) = material_frame.as_mut() {
            frame.0.insert(
                root,
                material_weights(&cache.expressions, &member.config, &values, &automatic),
            );
        }
        for node in &cache.nodes {
            if let Ok(mut morph) = morphs.get_mut(node.entity) {
                compose(morph.weights_mut(), node, &values);
                compose_behavior(
                    morph.weights_mut(),
                    node,
                    &cache.expressions,
                    &member.config,
                    &automatic,
                );
            }
            for &primitive in &node.primitives {
                if let Ok((_, parent, mut mesh)) = meshes.get_mut(primitive)
                    && parent.parent() == node.entity
                    && let MeshMorphWeights::Value { weights } = &mut *mesh
                {
                    compose(weights, node, &values);
                    compose_behavior(
                        weights,
                        node,
                        &cache.expressions,
                        &member.config,
                        &automatic,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scene_emotion_weights_are_local_bounded_and_restored_on_removal() {
        let mut cache = FacialRuntime {
            expressions: vec![ExpressionMeta {
                name: "happy".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let emotion = SceneMemberEmotion {
            emotion: crate::reply_affect::AffectEmotion::Happy,
            strength: 10.,
            smoothing_seconds: 0.02,
        };
        let weights = scene_emotion_weights(&mut cache, Some(&emotion), 1.);
        assert!((weights["happy"] - 0.65).abs() < 0.001);
        let mut other = FacialRuntime::default();
        assert!(scene_emotion_weights(&mut other, Some(&emotion), 1.).is_empty());
        assert!(scene_emotion_weights(&mut cache, None, 1.).is_empty());
        assert!(cache.scene_weights.is_empty());
        let invalid = SceneMemberEmotion {
            strength: f32::NAN,
            ..emotion
        };
        assert!(scene_emotion_weights(&mut cache, Some(&invalid), 1.).is_empty());
    }

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
        assert_eq!(parsed.materials.issues.len(), 1);
        assert!(
            parsed
                .materials
                .issues
                .iter()
                .all(|s| s.contains("Toon only"))
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
        assert_eq!(zero.materials.count(), 3);
        assert_eq!(
            zero.materials.binds[&0].binds[1].authored,
            Some([0.1, 0.2, 0.3, 1.])
        );
        assert_eq!(
            zero.materials.binds[&0].binds[2].property,
            crate::material_expressions::MaterialProperty::Uv
        );
        assert_eq!(zero.materials.binds[&0].binds[2].target, [1., 1., 0., 0.]);
        assert!(
            !zero
                .materials
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
            uv_transform: bevy::math::Affine2::from_scale_angle_translation(
                Vec2::new(2., 3.),
                0.,
                Vec2::new(0.1, 0.2),
            ),
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
            !app.world()
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
        assert_eq!(toon.uv_transform.matrix2.x_axis.x, 3.);
        assert!((toon.uv_transform.translation.x - 0.2).abs() < 1e-5);
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
        let diagnostics = app.world().resource::<FacialState>();
        assert!(diagnostics.speech_level > 0.4);
        assert!(diagnostics.mouth_weight > 0.5);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .lip_sync = false;
        for _ in 0..160 {
            assert_eq!(decoder.next(), Some(0.5));
        }
        app.update();
        let diagnostics = app.world().resource::<FacialState>();
        assert!(
            diagnostics.speech_level > 0.4,
            "audio signal is independent of the lip-sync switch"
        );
        assert_eq!(diagnostics.mouth_weight, 0.);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .lip_sync = true;
        meter.stop();
        for _ in 0..20 {
            app.update();
        }
        assert!(app.world().get::<MorphWeights>(node).unwrap().weights()[0] < 0.001);
        assert_eq!(app.world().resource::<FacialState>().speech_level, 0.);
        assert!(app.world().resource::<FacialState>().mouth_weight < 0.001);
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        assert_eq!(app.world().resource::<FacialState>().speech_level, 0.);
        assert_eq!(app.world().resource::<FacialState>().mouth_weight, 0.);
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
        assert_eq!(app.world().resource::<FacialState>().speech_level, 0.);
        assert_eq!(app.world().resource::<FacialState>().mouth_weight, 0.);
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
    fn look_gltf(extensions: Value) -> gltf::Gltf {
        let json = serde_json::json!({"asset":{"version":"2.0"},"extensions":extensions});
        gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&json).unwrap()).unwrap()
    }
    #[test]
    fn vrm1_look_at_type_and_range_maps_parse_with_explicit_defaults() {
        let parsed = ExpressionHandler::parse(&look_gltf(serde_json::json!({"VRMC_vrm":{
            "expressions":{},
            "lookAt":{"type":"expression",
                "rangeMapHorizontalInner":{"inputMaxValue":30,"outputScale":0.5},
                "rangeMapHorizontalOuter":{"inputMaxValue":20,"outputScale":0.8},
                "rangeMapVerticalDown":{"inputMaxValue":10,"outputScale":1},
                "rangeMapVerticalUp":{"inputMaxValue":-4,"outputScale":1}}}})));
        let meta = parsed.look_at.unwrap();
        assert_eq!(meta.kind, LookAtKind::Expression);
        assert!(!meta.vrm0);
        assert_eq!(meta.horizontal_outer, LookRangeMap::linear(20., 0.8));
        assert_eq!(meta.vertical_up, LookRangeMap::linear(90., 1.));
        assert!(
            meta.issues
                .iter()
                .any(|s| s.contains("rangeMapVerticalUp.inputMaxValue"))
        );
        // Linear, saturating: half range → half output, beyond → output scale.
        assert!((meta.horizontal_outer.map(10.) - 0.4).abs() < 1e-6);
        assert!((meta.horizontal_outer.map(80.) - 0.8).abs() < 1e-6);
        assert_eq!(meta.horizontal_outer.map(-5.), 0.);
        assert_eq!(meta.horizontal_outer.map(f32::NAN), 0.);

        let bone =
            LookAtMeta::parse(Some(&serde_json::json!({"lookAt":{"type":"bone"}})), None).unwrap();
        assert_eq!(bone.kind, LookAtKind::Bone);
        assert_eq!(bone.horizontal_inner, LookRangeMap::linear(90., 10.));
        assert_eq!(bone.issues.len(), 4, "every defaulted map is reported");
        let untyped = LookAtMeta::parse(Some(&serde_json::json!({"lookAt":{}})), None).unwrap();
        assert_eq!(untyped.kind, LookAtKind::Bone);
        assert!(untyped.issues[0].contains("type is missing"));
        let future = LookAtMeta::parse(
            Some(&serde_json::json!({"lookAt":{"type":"blendShapeV2"}})),
            None,
        )
        .unwrap();
        assert_eq!(future.kind, LookAtKind::Unsupported);
        assert_eq!(future.expression_weights(30., 30.), [0.; 4]);
        assert_eq!(future.bone_eye_degrees(true, 30., 30.), (0., 0.));
        // VRM 1 without lookAt is legacy, and never falls through to VRM 0 data.
        assert!(
            LookAtMeta::parse(
                Some(&serde_json::json!({})),
                Some(&serde_json::json!({"firstPerson":{"lookAtTypeName":"BlendShape"}}))
            )
            .is_none()
        );
        assert!(LookAtMeta::parse(None, None).is_none());
    }
    #[test]
    fn vrm0_blend_shape_curves_use_univrm_hermite_mapping_and_report_malformed_data() {
        let parsed = ExpressionHandler::parse(&look_gltf(serde_json::json!({"VRM":{
            "blendShapeMaster":{"blendShapeGroups":[]},
            "firstPerson":{"lookAtTypeName":"BlendShape",
                "lookAtHorizontalOuter":{"curve":[0,0,0,1,1,1,1,0],"xRange":40,"yRange":1},
                "lookAtHorizontalInner":{"curve":[0,0,0,1,0,1,1,0],"xRange":40,"yRange":1},
                "lookAtVerticalDown":{"curve":[0,0,1,1,1,1,1,1],"xRange":20,"yRange":0.5},
                "lookAtVerticalUp":{"xRange":0,"yRange":1}}}})));
        let meta = parsed.look_at.unwrap();
        assert!(meta.vrm0);
        assert_eq!(meta.kind, LookAtKind::Expression);
        // Unit outgoing/incoming tangents make the default curve linear:
        // at s=0.5, h10 + h01 + h11 = 0.125 + 0.5 - 0.125.
        assert!((meta.horizontal_outer.map(20.) - 0.5).abs() < 1e-6);
        // A zero incoming tangent at the final key yields the distinct
        // authored ease-out value, proving this is not just linear mapping.
        let mut eased = meta.horizontal_outer.clone();
        eased.curve[1].in_tangent = 0.;
        assert!((eased.map(20.) - 0.625).abs() < 1e-6);
        assert_eq!(meta.horizontal_outer.map(40.), 1.);
        assert_eq!(meta.horizontal_outer.map(400.), 1.);
        // Non-ascending key times are rejected to linear with a diagnostic.
        assert!(meta.horizontal_inner.curve.is_empty());
        assert!((meta.horizontal_inner.map(10.) - 0.25).abs() < 1e-6);
        assert!(
            meta.issues
                .iter()
                .any(|s| s.contains("lookAtHorizontalInner.curve"))
        );
        // Unit tangents everywhere reproduce the straight line.
        assert!((meta.vertical_down.map(5.) - 0.125).abs() < 1e-6);
        // xRange 0 is invalid and falls back to 90 with the default curve.
        assert_eq!(meta.vertical_up.input_max_degrees, 90.);
        assert!(
            meta.issues
                .iter()
                .any(|s| s.contains("lookAtVerticalUp.xRange"))
        );
        assert_eq!(evaluate_curve(&meta.horizontal_outer.curve, -1.), 0.);
        assert!(parse_curve(&serde_json::json!([0, 0, 0])).is_none());
        assert!(parse_curve(&serde_json::json!([])).is_none());
        assert!(parse_curve(&serde_json::json!([0, "x", 0, 0])).is_none());

        let bone = LookAtMeta::parse(
            None,
            Some(&serde_json::json!({"firstPerson":{
            "lookAtTypeName":"Bone"}})),
        )
        .unwrap();
        assert_eq!(bone.kind, LookAtKind::Bone);
        assert_eq!(bone.horizontal_outer.output_scale, 10.);
        assert_eq!(bone.horizontal_outer.curve.len(), 2);
        assert!(LookAtMeta::parse(None, Some(&serde_json::json!({}))).is_none());
    }
    #[test]
    fn look_directions_map_to_presets_and_bone_eyes_use_inner_outer_per_eye() {
        let meta = LookAtMeta {
            kind: LookAtKind::Expression,
            vrm0: false,
            horizontal_inner: LookRangeMap::linear(10., 0.2),
            horizontal_outer: LookRangeMap::linear(10., 1.),
            vertical_down: LookRangeMap::linear(10., 0.5),
            vertical_up: LookRangeMap::linear(20., 1.),
            issues: vec![],
        };
        assert_eq!(meta.expression_weights(5., 10.), [0.5, 0., 0.5, 0.]);
        assert_eq!(meta.expression_weights(-50., -4.), [0., 1., 0., 0.2]);
        assert_eq!(meta.expression_weights(0., 0.), [0.; 4]);
        assert_eq!(meta.expression_weights(f32::NAN, f32::INFINITY), [0.; 4]);
        assert_eq!(
            meta.bone_eye_degrees(true, 5., 0.),
            (0., 0.),
            "expression rigs never rotate bones"
        );
        let bone = LookAtMeta {
            kind: LookAtKind::Bone,
            horizontal_inner: LookRangeMap::linear(90., 8.),
            horizontal_outer: LookRangeMap::linear(90., 12.),
            vertical_down: LookRangeMap::linear(90., 10.),
            vertical_up: LookRangeMap::linear(90., 6.),
            ..meta
        };
        assert_eq!(bone.expression_weights(45., 45.), [0.; 4]);
        // Looking left turns the left eye outward and the right eye inward.
        assert_eq!(bone.bone_eye_degrees(true, 45., 45.), (6., 3.));
        assert_eq!(bone.bone_eye_degrees(false, 45., -45.), (4., -5.));
        assert_eq!(bone.bone_eye_degrees(true, -90., 0.), (-8., 0.));
        assert_eq!(bone.bone_eye_degrees(false, -180., 0.), (-12., 0.));
    }
    #[test]
    fn gaze_look_weights_yield_to_manual_weights_and_override_look_at() {
        let names = [
            "lookLeft",
            "lookRight",
            "lookUp",
            "lookDown",
            "surprised",
            "lookingGlass",
        ];
        let mut expressions: Vec<_> = names
            .into_iter()
            .map(|name| ExpressionMeta {
                name: name.into(),
                ..default()
            })
            .collect();
        expressions[4].look_override = 1;
        expressions[5].look_override = 0;
        let look = Some([0.6, 0., 0.3, 0.]);
        let mut config = FacialConfig {
            auto_blink: false,
            ..default()
        };
        let weights = |config: &FacialConfig, expressions: &[ExpressionMeta]| {
            expression_weights_with_look(expressions, config, 0., 0., None, look)
        };
        assert_eq!(weights(&config, &expressions), [0.6, 0., 0.3, 0., 0., 0.]);
        // An active manual slider owns its channel; neutral legacy sliders do not.
        config.weights.insert("lookLeft".into(), 0.2);
        config.weights.insert("lookUp".into(), 0.0);
        assert_eq!(weights(&config, &expressions), [0.2, 0., 0.3, 0., 0., 0.]);
        // A custom look-alike name is not a look preset and is not blocked.
        config.weights.insert("lookingGlass".into(), 0.7);
        config.selected_expression = "surprised".into();
        assert_eq!(weights(&config, &expressions), [0., 0., 0., 0., 1., 0.7]);
        expressions[4].look_override = 2;
        config.expression_weight = 0.25;
        let blended = weights(&config, &expressions);
        assert!((blended[0] - 0.15).abs() < 1e-6 && (blended[2] - 0.225).abs() < 1e-6);
        assert_eq!(
            expression_weights(&expressions, &FacialConfig::default(), 0., 0., None),
            expression_weights_with_look(
                &expressions,
                &FacialConfig::default(),
                0.,
                0.,
                None,
                None
            )
        );
    }
    #[test]
    fn gaze_input_drives_only_the_published_root_and_releases_in_the_same_frame() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ExpressionsPlugin)
            .init_resource::<AvatarLoadState>();
        let meta = LookAtMeta::parse(
            Some(&serde_json::json!({"lookAt":{"type":"expression",
                "rangeMapHorizontalOuter":{"inputMaxValue":10,"outputScale":1},
                "rangeMapHorizontalInner":{"inputMaxValue":10,"outputScale":1},
                "rangeMapVerticalDown":{"inputMaxValue":10,"outputScale":1},
                "rangeMapVerticalUp":{"inputMaxValue":10,"outputScale":1}}})),
            None,
        )
        .unwrap();
        let spawn = |app: &mut App| {
            let root = app
                .world_mut()
                .spawn((
                    ExpressionCatalog {
                        material_issues: vec![],
                        expressions: LOOK_EXPRESSIONS
                            .into_iter()
                            .map(|name| ExpressionMeta {
                                name: name.into(),
                                ..default()
                            })
                            .collect(),
                    },
                    AuthoredLookAt(meta.clone()),
                ))
                .id();
            let node = app
                .world_mut()
                .spawn((
                    ChildOf(root),
                    ExpressionNode {
                        binds: (0..4)
                            .map(|i| NodeBind {
                                expression: i,
                                index: i,
                                weight: 1.,
                            })
                            .collect(),
                    },
                    MorphWeights::new(vec![0., 0., 0., 0.2], None).unwrap(),
                ))
                .id();
            (root, node)
        };
        let (first, first_node) = spawn(&mut app);
        let (second, second_node) = spawn(&mut app);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .auto_blink = false;
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(first);
        app.update();
        {
            let link = app.world().resource::<LookAtLink>();
            assert_eq!(link.root, Some(first));
            assert!(link.resolved);
            assert_eq!(link.meta.as_ref(), Some(&meta));
            assert_eq!(link.expressions, [true; 4]);
        }
        let morphs = |app: &App, node| {
            app.world()
                .get::<MorphWeights>(node)
                .unwrap()
                .weights()
                .to_vec()
        };
        let send = |app: &mut App, root, left, up| {
            app.world_mut().resource_mut::<LookAtLink>().input = Some(LookInput {
                root,
                left_degrees: left,
                up_degrees: up,
                strength: 1.,
            });
        };
        send(&mut app, first, -5., -20.);
        app.update();
        assert_eq!(morphs(&app, first_node), vec![0., 0.5, 0., 1.]);
        // Input addressed to another root is stale and must not move this face.
        send(&mut app, second, 10., 10.);
        app.update();
        assert_eq!(morphs(&app, first_node), vec![0., 0., 0., 0.2]);
        // Manual priority holds with live gaze input.
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .weights
            .insert("lookRight".into(), 0.1);
        send(&mut app, first, -10., 0.);
        app.update();
        assert!((morphs(&app, first_node)[1] - 0.1).abs() < 1e-6);
        app.world_mut()
            .resource_mut::<FacialState>()
            .config
            .weights
            .clear();
        send(&mut app, first, 10., 0.);
        app.update();
        assert_eq!(morphs(&app, first_node), vec![1., 0., 0., 0.2]);
        // Clearing input (gaze disabled) restores the baseline the same frame.
        app.world_mut().resource_mut::<LookAtLink>().input = None;
        app.update();
        assert_eq!(morphs(&app, first_node), vec![0., 0., 0., 0.2]);
        // Switching roots restores the old face and republishes metadata.
        send(&mut app, first, 10., 0.);
        app.update();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(second);
        app.update();
        assert_eq!(morphs(&app, first_node), vec![0., 0., 0., 0.2]);
        assert_eq!(morphs(&app, second_node), vec![0., 0., 0., 0.2]);
        assert_eq!(app.world().resource::<LookAtLink>().root, Some(second));
        app.world_mut().resource_mut::<AvatarLoadState>().active = None;
        app.update();
        let link = app.world().resource::<LookAtLink>();
        assert_eq!(
            (link.root, link.resolved, link.meta.is_none()),
            (None, false, true)
        );
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
    #[test]
    fn look_input_strength_scales_after_authored_mapping_and_zero_releases() {
        let meta = LookAtMeta::parse(
            Some(&serde_json::json!({"lookAt": {
                "type": "expression", "rangeMapHorizontalOuter": {
                    "inputMaxValue": 90, "outputScale": 1
                }
            }})),
            None,
        )
        .unwrap();
        let mut input = LookInput {
            root: Entity::PLACEHOLDER,
            left_degrees: 60.,
            up_degrees: 0.,
            strength: 0.3,
        };
        assert!((input.weights(&meta)[0] - 0.2).abs() < 1e-6);
        input.strength = 0.;
        assert_eq!(input.weights(&meta), [0.; 4]);
        input.strength = f32::NAN;
        assert_eq!(input.weights(&meta), [0.; 4]);
    }
}
