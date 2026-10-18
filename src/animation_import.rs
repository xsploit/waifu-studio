//! Independent bounded animation readers. No network, executable, model meshes or textures.
//! VRMA follows glTF 2.0 and VRMC_vrm_animation; FBX imports the first skeletal take.
use super::{BvhClip, BvhJoint, MAX_FILE, MAX_SAMPLES, map_bone};
use bevy::prelude::*;
use serde_json::Value;
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;
use std::collections::{HashMap, HashSet};
use std::io::Read;

const MAX_NODES: usize = 4096;
const MAX_KEYS: usize = 1_000_000;

#[derive(Debug, Clone, Copy)]
enum Interpolation {
    Linear,
    Step,
    Cubic,
}
#[derive(Debug)]
pub(super) struct RotationTrack {
    times: Vec<f32>,
    values: Vec<Quat>,
    incoming: Vec<Vec4>,
    outgoing: Vec<Vec4>,
    interpolation: Interpolation,
}
impl RotationTrack {
    pub(super) fn sample(&self, time: f32) -> Quat {
        let upper = self.times.partition_point(|key| *key <= time);
        if upper == 0 {
            return self.values[0];
        }
        if upper == self.times.len() {
            return self.values[upper - 1];
        }
        let left = upper - 1;
        let span = self.times[upper] - self.times[left];
        let t = ((time - self.times[left]) / span).clamp(0.0, 1.0);
        match self.interpolation {
            Interpolation::Step => self.values[left],
            Interpolation::Linear => self.values[left].slerp(self.values[upper], t),
            Interpolation::Cubic => {
                let t2 = t * t;
                let t3 = t2 * t;
                let value = Vec4::from_array(self.values[left].to_array())
                    * (2.0 * t3 - 3.0 * t2 + 1.0)
                    + self.outgoing[left] * span * (t3 - 2.0 * t2 + t)
                    + Vec4::from_array(self.values[upper].to_array()) * (-2.0 * t3 + 3.0 * t2)
                    + self.incoming[upper] * span * (t3 - t2);
                if value.is_finite()
                    && value.length_squared().is_finite()
                    && value.length_squared() > 1e-12
                {
                    Quat::from_array(value.normalize().to_array())
                } else {
                    self.values[left]
                }
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct TranslationTrack {
    times: Vec<f32>,
    values: Vec<Vec3>,
    incoming: Vec<Vec3>,
    outgoing: Vec<Vec3>,
    interpolation: Interpolation,
}
impl TranslationTrack {
    pub(super) fn sample(&self, time: f32) -> Vec3 {
        let upper = self.times.partition_point(|key| *key <= time);
        if upper == 0 {
            return self.values[0];
        }
        if upper == self.times.len() {
            return self.values[upper - 1];
        }
        let left = upper - 1;
        let span = self.times[upper] - self.times[left];
        let t = ((time - self.times[left]) / span).clamp(0., 1.);
        match self.interpolation {
            Interpolation::Step => self.values[left],
            Interpolation::Linear => self.values[left].lerp(self.values[upper], t),
            Interpolation::Cubic => {
                let t2 = t * t;
                let t3 = t2 * t;
                let result = self.values[left] * (2. * t3 - 3. * t2 + 1.)
                    + self.outgoing[left] * span * (t3 - 2. * t2 + t)
                    + self.values[upper] * (-2. * t3 + 3. * t2)
                    + self.incoming[upper] * span * (t3 - t2);
                if result.is_finite() && result.abs().max_element() <= 10_000_000. {
                    result
                } else {
                    self.values[left]
                }
            }
        }
    }
}
fn array<'a>(root: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    root.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("Missing or invalid {key}"))
}
fn index(root: &Value, key: &str) -> Result<usize, String> {
    root.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| format!("Missing or invalid {key} index"))
}
fn checked_quat(raw: [f32; 4]) -> Result<Quat, String> {
    let quat = Quat::from_array(raw);
    if !quat.is_finite() || !quat.length_squared().is_finite() || quat.length_squared() < 1e-12 {
        return Err("Animation contains an invalid quaternion".into());
    }
    Ok(quat.normalize())
}
fn node_vec3(node: &Value, name: &str, default: Vec3) -> Result<Vec3, String> {
    let Some(value) = node.get(name) else {
        return Ok(default);
    };
    let array = value
        .as_array()
        .filter(|values| values.len() == 3)
        .ok_or("Invalid VRMA node vector")?;
    let mut result = Vec3::ZERO;
    for (index, value) in array.iter().enumerate() {
        result[index] = value.as_f64().ok_or("Invalid VRMA vector component")? as f32;
    }
    if !result.is_finite() || result.abs().max_element() > 10_000_000. {
        return Err("VRMA node vector is nonfinite or out of range".into());
    }
    Ok(result)
}
fn validate_times(times: &[f32]) -> Result<(), String> {
    if times.is_empty()
        || times.len() > MAX_KEYS
        || times
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0 || *v > 86_400.0)
        || times.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("Animation timestamps must be finite, increasing, and within one day".into());
    }
    Ok(())
}
fn float_accessor(
    root: &Value,
    bin: &[u8],
    accessor: usize,
    width: usize,
) -> Result<Vec<f32>, String> {
    let accessors = array(root, "accessors")?;
    let accessor = accessors
        .get(accessor)
        .ok_or("Animation accessor is out of range")?;
    let expected_type = match width {
        1 => "SCALAR",
        3 => "VEC3",
        4 => "VEC4",
        _ => return Err("Unsupported accessor type".into()),
    };
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || accessor.get("type").and_then(Value::as_str) != Some(expected_type)
        || accessor.get("sparse").is_some()
        || accessor.get("normalized").and_then(Value::as_bool) == Some(true)
    {
        return Err("VRMA animation requires nonsparse floating-point accessors".into());
    }
    let count = index(accessor, "count")?;
    if count == 0 || count.checked_mul(width).is_none_or(|n| n > MAX_SAMPLES) {
        return Err("VRMA accessor exceeds the sample limit".into());
    }
    let view = array(root, "bufferViews")?
        .get(index(accessor, "bufferView")?)
        .ok_or("VRMA bufferView is out of range")?;
    if index(view, "buffer")? != 0 {
        return Err("VRMA must use its embedded GLB buffer".into());
    }
    let view_start = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_len = index(view, "byteLength")?;
    let view_end = view_start
        .checked_add(view_len)
        .filter(|end| *end <= bin.len())
        .ok_or("VRMA bufferView exceeds its buffer")?;
    let offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .unwrap_or((width * 4) as u64) as usize;
    if stride < width * 4 || !stride.is_multiple_of(4) || stride > 252 {
        return Err("Invalid VRMA accessor stride".into());
    }
    let start = view_start
        .checked_add(offset)
        .ok_or("VRMA accessor offset overflow")?;
    let end = (count - 1)
        .checked_mul(stride)
        .and_then(|n| start.checked_add(n))
        .and_then(|n| n.checked_add(width * 4))
        .filter(|end| *end <= view_end)
        .ok_or("VRMA accessor exceeds its bufferView")?;
    if start > end {
        return Err("Invalid VRMA accessor range".into());
    }
    let mut values = Vec::with_capacity(count * width);
    for item in 0..count {
        for component in 0..width {
            let pos = start + item * stride + component * 4;
            let value = f32::from_le_bytes(bin[pos..pos + 4].try_into().unwrap());
            if !value.is_finite() || value.abs() > 10_000_000. {
                return Err("VRMA contains nonfinite or out-of-range sample values".into());
            }
            values.push(value);
        }
    }
    Ok(values)
}
fn glb_parts(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    if bytes.len() < 20 || bytes.len() as u64 > MAX_FILE || &bytes[..4] != b"glTF" {
        return Err("VRMA must be a GLB file at most 32 MiB".into());
    }
    let u32_at = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if u32_at(4) != 2 || u32_at(8) as usize != bytes.len() {
        return Err("Invalid GLB version or length".into());
    }
    let mut cursor = 12usize;
    let mut json = None;
    let mut binary = None;
    while cursor < bytes.len() {
        if bytes.len() - cursor < 8 {
            return Err("Truncated GLB chunk".into());
        }
        let length = u32_at(cursor) as usize;
        let kind = u32_at(cursor + 4);
        cursor += 8;
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or("Truncated GLB payload")?;
        if !length.is_multiple_of(4) {
            return Err("Unaligned GLB chunk".into());
        }
        match kind {
            0x4e4f534a if json.is_none() && cursor == 20 => {
                json = Some(
                    serde_json::from_slice::<Value>(&bytes[cursor..end])
                        .map_err(|e| format!("Invalid VRMA JSON: {e}"))?,
                )
            }
            0x004e4942 if binary.is_none() => binary = Some(&bytes[cursor..end]),
            0x4e4f534a | 0x004e4942 => return Err("Invalid or duplicate GLB chunk".into()),
            _ => {}
        }
        cursor = end;
    }
    Ok((
        json.ok_or("GLB has no JSON chunk")?,
        binary.ok_or("GLB has no embedded animation buffer")?,
    ))
}
fn build_clip(
    mut joints: Vec<BvhJoint>,
    mut tracks: Vec<Option<RotationTrack>>,
    mut translations: Vec<Option<TranslationTrack>>,
    format: &'static str,
) -> Result<BvhClip, String> {
    if joints.is_empty()
        || joints.len() > MAX_NODES
        || joints.len() != tracks.len()
        || joints.len() != translations.len()
    {
        return Err("Invalid animation skeleton".into());
    }
    let mut rest_world = vec![Quat::IDENTITY; joints.len()];
    let mut ordered = Vec::with_capacity(joints.len());
    let mut remap = vec![usize::MAX; joints.len()];
    let mut depths = vec![0usize; joints.len()];
    // Parent-first ordering without recursion; refuses cycles and excessive depth.
    for _ in 0..128 {
        for (i, joint) in joints.iter().enumerate() {
            if remap[i] != usize::MAX {
                continue;
            }
            if joint
                .parent
                .is_none_or(|parent| parent < joints.len() && remap[parent] != usize::MAX)
            {
                depths[i] = joint.parent.map_or(0, |parent| depths[parent] + 1);
                if depths[i] > 128 {
                    return Err("Animation skeleton exceeds 128 hierarchy levels".into());
                }
                remap[i] = ordered.len();
                ordered.push(i);
            }
        }
        if ordered.len() == joints.len() {
            break;
        }
    }
    if ordered.len() != joints.len() {
        return Err("Animation skeleton contains a cycle or excessive depth".into());
    }
    let mut sorted = Vec::with_capacity(joints.len());
    let mut sorted_tracks = Vec::with_capacity(joints.len());
    let mut sorted_translations = Vec::with_capacity(joints.len());
    // Moving via Option avoids cloning large key arrays.
    let mut moving: Vec<_> = joints.drain(..).map(Some).collect();
    let mut duration = 0.0f32;
    let mut keys = 1usize;
    let mut total = 0usize;
    for old in ordered {
        let mut joint = moving[old].take().unwrap();
        joint.parent = joint.parent.map(|parent| remap[parent]);
        let new = sorted.len();
        rest_world[new] = joint.parent.map_or(joint.rest_rotation, |parent| {
            rest_world[parent] * joint.rest_rotation
        });
        let track = tracks[old].take();
        let translation = translations[old].take();
        if let Some(track) = &track {
            duration = duration.max(*track.times.last().unwrap());
            keys = keys.max(track.times.len());
            total += track.values.len() * 4 + track.incoming.len() * 4 + track.outgoing.len() * 4;
            if total > MAX_SAMPLES {
                return Err("Animation exceeds the 4 million sample limit".into());
            }
        }
        if let Some(track) = &translation {
            duration = duration.max(*track.times.last().unwrap());
            keys = keys.max(track.times.len());
            total += track.values.len() * 3 + track.incoming.len() * 3 + track.outgoing.len() * 3;
        }
        if total > MAX_SAMPLES {
            return Err("Animation exceeds the 4 million sample limit".into());
        }
        sorted.push(joint);
        sorted_tracks.push(track);
        sorted_translations.push(translation);
    }
    if !sorted.iter().any(|j| j.bone.is_some())
        || !sorted.iter().enumerate().any(|(index, joint)| {
            joint.bone.is_some()
                && (sorted_tracks[index].is_some() || sorted_translations[index].is_some())
        })
    {
        return Err("Animation has no recognized animated humanoid pose channels".into());
    }
    let duration = duration.max(1.0 / 60.0);
    let mut clip = BvhClip {
        joints: sorted,
        frame_count: keys.max(2),
        frame_time: duration / (keys.max(2) - 1) as f32,
        channel_count: 0,
        samples: Vec::new(),
        tracks: sorted_tracks,
        translation_tracks: sorted_translations,
        rest_world,
        rest_positions: Vec::new(),
        source_format: format,
        exact_duration: Some(duration),
    };
    clip.populate_rest_geometry()?;
    Ok(clip)
}

/// Parse the first animation in a standard binary VRM animation (.vrma).
/// Rotation tracks are retained at original timestamps, including cubic tangents.
/// Hips translations retain original timestamps/interpolation; expressions are separate.
pub fn parse_vrma(bytes: &[u8]) -> Result<BvhClip, String> {
    let (root, bin) = glb_parts(bytes)?;
    let buffers = array(&root, "buffers")?;
    if buffers.len() != 1
        || buffers[0].get("uri").is_some()
        || index(&buffers[0], "byteLength")? > bin.len()
    {
        return Err("VRMA must use one embedded buffer; external resources are not read".into());
    }
    let extension = root
        .pointer("/extensions/VRMC_vrm_animation")
        .ok_or("Missing VRMC_vrm_animation extension")?;
    if extension.get("specVersion").and_then(Value::as_str) != Some("1.0") {
        return Err("Only VRMC_vrm_animation 1.0 is supported".into());
    }
    let nodes = array(&root, "nodes")?;
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        return Err("VRMA exceeds 4096 nodes".into());
    }
    let human_bones = extension
        .pointer("/humanoid/humanBones")
        .and_then(Value::as_object)
        .ok_or("Missing VRMA humanoid mapping")?;
    let mut mapping = HashMap::new();
    let mut seen_bones = HashSet::new();
    for (name, mapped) in human_bones {
        let node = index(mapped, "node")?;
        if node >= nodes.len() {
            return Err("VRMA humanoid node is out of range".into());
        }
        if let Some(bone) = map_bone(name)
            && (mapping.insert(node, bone).is_some() || !seen_bones.insert(bone))
        {
            return Err("Ambiguous VRMA humanoid mapping".into());
        }
    }
    let mut joints = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        let mut rest_translation = Vec3::ZERO;
        let mut rest_scale = Vec3::ONE;
        let rotation = if let Some(matrix) = node.get("matrix") {
            let values = matrix
                .as_array()
                .filter(|v| v.len() == 16)
                .ok_or("Invalid VRMA node matrix")?;
            let mut raw = [0.0; 16];
            for (dest, source) in raw.iter_mut().zip(values) {
                *dest = source.as_f64().ok_or("Invalid VRMA matrix number")? as f32;
            }
            let matrix = Mat4::from_cols_array(&raw);
            if !matrix.is_finite()
                || !matrix.determinant().is_finite()
                || matrix.determinant().abs() < 1e-12
                || matrix.x_axis.w != 0.0
                || matrix.y_axis.w != 0.0
                || matrix.z_axis.w != 0.0
                || matrix.w_axis.w != 1.0
            {
                return Err("VRMA node matrix must be finite, affine and invertible".into());
            }
            let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
            if !scale.is_finite() || scale.min_element() <= 0.0 || !translation.is_finite() {
                return Err("Unsupported VRMA node matrix".into());
            }
            rest_translation = translation;
            rest_scale = scale;
            checked_quat(rotation.to_array())?
        } else if let Some(rotation) = node.get("rotation") {
            let values = rotation
                .as_array()
                .filter(|v| v.len() == 4)
                .ok_or("Invalid VRMA node rotation")?;
            let mut raw = [0.0; 4];
            for (dest, source) in raw.iter_mut().zip(values) {
                *dest = source.as_f64().ok_or("Invalid VRMA quaternion number")? as f32;
            }
            checked_quat(raw)?
        } else {
            Quat::IDENTITY
        };
        if node.get("matrix").is_none() {
            rest_translation = node_vec3(node, "translation", Vec3::ZERO)?;
            rest_scale = node_vec3(node, "scale", Vec3::ONE)?;
        }
        if rest_scale.min_element() <= 0.
            || !rest_translation.is_finite()
            || rest_translation.abs().max_element() > 10_000_000.
        {
            return Err("Unsupported VRMA rest translation or scale".into());
        }
        joints.push(BvhJoint {
            name: node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("node")
                .chars()
                .take(256)
                .collect(),
            parent: None,
            offset: rest_translation,
            bone: mapping.get(&i).copied(),
            channels: Vec::new(),
            first_channel: 0,
            rest_rotation: rotation,
            rest_translation,
            rest_scale,
        });
    }
    for (parent, node) in nodes.iter().enumerate() {
        if let Some(children) = node.get("children") {
            for child in children.as_array().ok_or("Invalid VRMA children")? {
                let child = child
                    .as_u64()
                    .and_then(|v| usize::try_from(v).ok())
                    .filter(|n| *n < joints.len())
                    .ok_or("VRMA child index is out of range")?;
                if joints[child].parent.replace(parent).is_some() {
                    return Err("VRMA node has more than one parent".into());
                }
            }
        }
    }
    let animation = array(&root, "animations")?
        .first()
        .ok_or("VRMA has no animation")?;
    let samplers = array(animation, "samplers")?;
    let channels = array(animation, "channels")?;
    if channels.len() > MAX_NODES * 4 {
        return Err("VRMA has too many channels".into());
    }
    let mut tracks: Vec<Option<RotationTrack>> = (0..nodes.len()).map(|_| None).collect();
    let mut translations: Vec<Option<TranslationTrack>> = (0..nodes.len()).map(|_| None).collect();
    let mut total = 0usize;
    for channel in channels {
        let target = channel.get("target").ok_or("Missing VRMA channel target")?;
        let path = target.get("path").and_then(Value::as_str);
        if !matches!(path, Some("rotation" | "translation")) {
            continue;
        }
        let node = index(target, "node")?;
        if node >= joints.len() {
            return Err("VRMA animation target is out of range".into());
        }
        let is_translation = path == Some("translation");
        if is_translation && joints[node].bone != Some(HumanBoneName::Hips) {
            continue;
        }
        if is_translation && translations[node].is_some() {
            return Err("Duplicate VRMA hips translation channel".into());
        }
        if !is_translation && tracks[node].is_some() {
            return Err("Duplicate VRMA rotation channel".into());
        }
        let sampler = samplers
            .get(index(channel, "sampler")?)
            .ok_or("VRMA sampler is out of range")?;
        let interpolation = match sampler
            .get("interpolation")
            .and_then(Value::as_str)
            .unwrap_or("LINEAR")
        {
            "LINEAR" => Interpolation::Linear,
            "STEP" => Interpolation::Step,
            "CUBICSPLINE" => Interpolation::Cubic,
            _ => return Err("Unsupported VRMA interpolation".into()),
        };
        let times = float_accessor(&root, bin, index(sampler, "input")?, 1)?;
        validate_times(&times)?;
        let components = if is_translation { 3 } else { 4 };
        let raw = float_accessor(&root, bin, index(sampler, "output")?, components)?;
        let multiplier = if matches!(interpolation, Interpolation::Cubic) {
            3
        } else {
            1
        };
        if raw.len() != times.len() * components * multiplier {
            return Err("VRMA keyframe count does not match its timestamps".into());
        }
        total += raw.len() + times.len();
        if total > MAX_SAMPLES {
            return Err("VRMA exceeds the 4 million sample limit".into());
        }
        if is_translation {
            let mut values = Vec::with_capacity(times.len());
            let mut incoming = Vec::new();
            let mut outgoing = Vec::new();
            for key in raw.chunks_exact(3 * multiplier) {
                let offset = if multiplier == 3 { 3 } else { 0 };
                values.push(Vec3::from_array(
                    key[offset..offset + 3].try_into().unwrap(),
                ));
                if multiplier == 3 {
                    incoming.push(Vec3::from_array(key[..3].try_into().unwrap()));
                    outgoing.push(Vec3::from_array(key[6..9].try_into().unwrap()));
                }
            }
            translations[node] = Some(TranslationTrack {
                times,
                values,
                incoming,
                outgoing,
                interpolation,
            });
            continue;
        }
        let mut values = Vec::with_capacity(times.len());
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();
        for key in raw.chunks_exact(4 * multiplier) {
            let offset = if multiplier == 3 { 4 } else { 0 };
            values.push(checked_quat(key[offset..offset + 4].try_into().unwrap())?);
            if multiplier == 3 {
                incoming.push(Vec4::from_array(key[..4].try_into().unwrap()));
                outgoing.push(Vec4::from_array(key[8..12].try_into().unwrap()));
            }
        }
        tracks[node] = Some(RotationTrack {
            times,
            values,
            incoming,
            outgoing,
            interpolation,
        });
    }
    build_clip(joints, tracks, translations, "VRMA")
}

#[derive(Debug)]
enum FbxValue {
    Int(i64),
    Number(f64),
    Text(String),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
}
impl FbxValue {
    fn int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }
    fn number(&self) -> Option<f64> {
        match self {
            Self::Int(v) => Some(*v as f64),
            Self::Number(v) => Some(*v),
            _ => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            _ => None,
        }
    }
}
#[derive(Debug)]
struct FbxNode {
    name: String,
    values: Vec<FbxValue>,
    children: Vec<FbxNode>,
}
impl FbxNode {
    fn child(&self, name: &str) -> Option<&Self> {
        self.children.iter().find(|n| n.name == name)
    }
    fn numbers(&self) -> Result<Vec<f64>, String> {
        let values = self.child("a").unwrap_or(self);
        match values.values.first() {
            Some(FbxValue::Floats(v)) => Ok(v.clone()),
            Some(FbxValue::Ints(v)) => Ok(v.iter().map(|v| *v as f64).collect()),
            _ => values
                .values
                .iter()
                .map(|v| v.number().ok_or("Invalid FBX number array".into()))
                .collect(),
        }
    }
    fn integers(&self) -> Result<Vec<i64>, String> {
        let values = self.child("a").unwrap_or(self);
        match values.values.first() {
            Some(FbxValue::Ints(v)) => Ok(v.clone()),
            _ => values
                .values
                .iter()
                .map(|v| v.int().ok_or("Invalid FBX integer array".into()))
                .collect(),
        }
    }
}
struct BinaryFbx<'a> {
    bytes: &'a [u8],
    cursor: usize,
    wide: bool,
    nodes: usize,
    samples: usize,
}
impl<'a> BinaryFbx<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .cursor
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or("Truncated FBX data")?;
        let value = &self.bytes[self.cursor..end];
        self.cursor = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn word(&mut self) -> Result<usize, String> {
        let raw = if self.wide {
            u64::from_le_bytes(self.take(8)?.try_into().unwrap())
        } else {
            self.u32()? as u64
        };
        usize::try_from(raw).map_err(|_| "FBX offset overflow".into())
    }
    fn property(&mut self) -> Result<Option<FbxValue>, String> {
        let kind = self.u8()?;
        Ok(Some(match kind {
            b'Y' => FbxValue::Int(i16::from_le_bytes(self.take(2)?.try_into().unwrap()) as i64),
            b'C' | b'B' => FbxValue::Int(self.u8()? as i64),
            b'I' => FbxValue::Int(i32::from_le_bytes(self.take(4)?.try_into().unwrap()) as i64),
            b'L' => FbxValue::Int(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            b'F' => FbxValue::Number(f32::from_le_bytes(self.take(4)?.try_into().unwrap()) as f64),
            b'D' => FbxValue::Number(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            b'S' | b'R' => {
                let count = self.u32()? as usize;
                let value = self.take(count)?;
                if kind == b'R' {
                    return Ok(None);
                }
                if count > 1024 * 1024 {
                    return Err("FBX string exceeds 1 MiB".into());
                }
                FbxValue::Text(
                    std::str::from_utf8(value)
                        .map_err(|_| "FBX contains invalid UTF-8 text")?
                        .to_owned(),
                )
            }
            b'f' | b'd' | b'l' | b'i' | b'b' | b'c' => {
                let count = self.u32()? as usize;
                self.samples = self
                    .samples
                    .checked_add(count)
                    .filter(|n| *n <= MAX_SAMPLES)
                    .ok_or("FBX exceeds the 4 million array-element limit")?;
                let encoding = self.u32()?;
                let packed_len = self.u32()? as usize;
                let packed = self.take(packed_len)?;
                let width = match kind {
                    b'd' | b'l' => 8,
                    b'b' | b'c' => 1,
                    _ => 4,
                };
                let expected = count
                    .checked_mul(width)
                    .ok_or("FBX array length overflow")?;
                let storage;
                let raw = match encoding {
                    0 => packed,
                    1 => {
                        let mut decoded = Vec::with_capacity(expected.min(1024 * 1024));
                        flate2::read::ZlibDecoder::new(packed)
                            .take(expected as u64 + 1)
                            .read_to_end(&mut decoded)
                            .map_err(|e| format!("Cannot decode compressed FBX array: {e}"))?;
                        storage = decoded;
                        &storage
                    }
                    _ => return Err("Unsupported FBX array encoding".into()),
                };
                if raw.len() != expected {
                    return Err("FBX array length does not match its declaration".into());
                }
                if matches!(kind, b'f' | b'd') {
                    let mut values = Vec::with_capacity(count);
                    for raw in raw.chunks_exact(width) {
                        let value = if kind == b'd' {
                            f64::from_le_bytes(raw.try_into().unwrap())
                        } else {
                            f32::from_le_bytes(raw.try_into().unwrap()) as f64
                        };
                        if !value.is_finite() {
                            return Err("FBX contains nonfinite values".into());
                        }
                        values.push(value);
                    }
                    FbxValue::Floats(values)
                } else {
                    let values = raw
                        .chunks_exact(width)
                        .map(|raw| match width {
                            8 => i64::from_le_bytes(raw.try_into().unwrap()),
                            4 => i32::from_le_bytes(raw.try_into().unwrap()) as i64,
                            _ => raw[0] as i64,
                        })
                        .collect();
                    FbxValue::Ints(values)
                }
            }
            _ => return Err(format!("Unsupported binary FBX property type {kind}")),
        }))
    }
    fn node(&mut self, depth: usize) -> Result<Option<FbxNode>, String> {
        if depth > 64 || self.nodes >= 100_000 {
            return Err("FBX hierarchy exceeds supported limits".into());
        }
        let end = self.word()?;
        let properties = self.word()?;
        let property_bytes = self.word()?;
        let name_len = self.u8()? as usize;
        if end == 0 && properties == 0 && property_bytes == 0 && name_len == 0 {
            return Ok(None);
        }
        self.nodes += 1;
        if end > self.bytes.len() || end <= self.cursor || properties > MAX_SAMPLES {
            return Err("Invalid FBX node bounds".into());
        }
        let name = std::str::from_utf8(self.take(name_len)?)
            .map_err(|_| "Invalid FBX node name")?
            .to_owned();
        let property_end = self
            .cursor
            .checked_add(property_bytes)
            .filter(|n| *n <= end)
            .ok_or("FBX properties exceed their node")?;
        let mut values = Vec::new();
        for _ in 0..properties {
            if let Some(value) = self.property()? {
                values.push(value);
            }
        }
        if self.cursor != property_end {
            return Err("FBX property length mismatch".into());
        }
        let mut children = Vec::new();
        while self.cursor < end {
            if let Some(node) = self.node(depth + 1)? {
                children.push(node);
            } else {
                break;
            }
        }
        if self.cursor != end {
            return Err("FBX child nodes exceed their parent".into());
        }
        Ok(Some(FbxNode {
            name,
            values,
            children,
        }))
    }
}
#[derive(Debug, PartialEq)]
enum Token {
    Atom(String),
    Colon,
    Comma,
    Open,
    Close,
    Star,
    Line,
}
fn ascii_tokens(text: &str) -> Result<Vec<Token>, String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let token = match bytes[i] {
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'\n' => {
                i += 1;
                Token::Line
            }
            b' ' | b'\r' | b'\t' => {
                i += 1;
                continue;
            }
            b':' => {
                i += 1;
                Token::Colon
            }
            b',' => {
                i += 1;
                Token::Comma
            }
            b'{' => {
                i += 1;
                Token::Open
            }
            b'}' => {
                i += 1;
                Token::Close
            }
            b'*' => {
                i += 1;
                Token::Star
            }
            b'"' => {
                i += 1;
                let mut value = Vec::new();
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\'
                        && i + 1 < bytes.len()
                        && matches!(bytes[i + 1], b'"' | b'\\')
                    {
                        i += 1;
                    }
                    value.push(bytes[i]);
                    i += 1;
                }
                if i == bytes.len() || value.len() > 1024 * 1024 {
                    return Err("Invalid FBX quoted string".into());
                }
                i += 1;
                Token::Atom(String::from_utf8(value).map_err(|_| "Invalid FBX string")?)
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b':' | b',' | b'{' | b'}' | b'*' | b';')
                {
                    i += 1;
                }
                if start == i {
                    return Err("Invalid FBX text token".into());
                }
                Token::Atom(text[start..i].to_owned())
            }
        };
        tokens.push(token);
        if tokens.len() > MAX_SAMPLES {
            return Err("ASCII FBX exceeds the token limit".into());
        }
    }
    Ok(tokens)
}
fn ascii_nodes(
    tokens: &[Token],
    cursor: &mut usize,
    depth: usize,
    count: &mut usize,
) -> Result<Vec<FbxNode>, String> {
    if depth > 64 {
        return Err("ASCII FBX hierarchy is too deep".into());
    }
    let mut nodes = Vec::new();
    while *cursor < tokens.len() {
        match &tokens[*cursor] {
            Token::Line => {
                *cursor += 1;
                continue;
            }
            Token::Close if depth > 0 => {
                *cursor += 1;
                return Ok(nodes);
            }
            _ => {}
        }
        let Some(Token::Atom(name)) = tokens.get(*cursor) else {
            return Err("Expected FBX node name".into());
        };
        let name = name.clone();
        *cursor += 1;
        if tokens.get(*cursor) != Some(&Token::Colon) {
            return Err("Expected ':' in ASCII FBX".into());
        }
        *cursor += 1;
        *count += 1;
        if *count > 100_000 {
            return Err("ASCII FBX contains too many nodes".into());
        }
        let mut values = Vec::new();
        let mut children = Vec::new();
        loop {
            match tokens.get(*cursor) {
                Some(Token::Atom(value)) => {
                    let value = if let Ok(v) = value.parse::<i64>() {
                        FbxValue::Int(v)
                    } else if let Ok(v) = value.parse::<f64>() {
                        if !v.is_finite() {
                            return Err("FBX contains nonfinite numbers".into());
                        }
                        FbxValue::Number(v)
                    } else {
                        FbxValue::Text(value.clone())
                    };
                    values.push(value);
                    *cursor += 1;
                }
                Some(Token::Comma | Token::Star) => {
                    *cursor += 1;
                }
                Some(Token::Open) => {
                    *cursor += 1;
                    children = ascii_nodes(tokens, cursor, depth + 1, count)?;
                    break;
                }
                Some(Token::Line) => {
                    *cursor += 1;
                    // FBX permits array values to wrap after a comma.
                    if name == "a" && tokens.get(cursor.saturating_sub(2)) == Some(&Token::Comma) {
                        continue;
                    }
                    break;
                }
                Some(Token::Close) | None => break,
                _ => return Err("Invalid ASCII FBX property list".into()),
            }
        }
        nodes.push(FbxNode {
            name,
            values,
            children,
        });
    }
    if depth != 0 {
        return Err("Unclosed ASCII FBX node".into());
    }
    Ok(nodes)
}
fn fbx_tree(bytes: &[u8]) -> Result<Vec<FbxNode>, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err("FBX exceeds the 32 MiB file limit".into());
    }
    if bytes.starts_with(b"Kaydara FBX Binary  \0\x1a\0") {
        if bytes.len() < 27 {
            return Err("Truncated binary FBX header".into());
        }
        let version = u32::from_le_bytes(bytes[23..27].try_into().unwrap());
        if !(7100..=7700).contains(&version) {
            return Err(format!(
                "FBX version {version} is unsupported; export FBX 7.1–7.7"
            ));
        }
        let mut reader = BinaryFbx {
            bytes,
            cursor: 27,
            wide: version >= 7500,
            nodes: 0,
            samples: 0,
        };
        let mut nodes = Vec::new();
        while reader.cursor < bytes.len() {
            match reader.node(0)? {
                Some(node) => nodes.push(node),
                None => break,
            }
        }
        Ok(nodes)
    } else {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "FBX is neither binary nor UTF-8 ASCII")?;
        let tokens = ascii_tokens(text)?;
        ascii_nodes(&tokens, &mut 0, 0, &mut 0)
    }
}

fn fbx_property<'a>(node: &'a FbxNode, name: &str) -> Option<&'a FbxNode> {
    node.child("Properties70")
        .or_else(|| node.child("Properties60"))?
        .children
        .iter()
        .find(|p| p.name == "P" && p.values.first().and_then(FbxValue::text) == Some(name))
}
fn property_vec3(node: &FbxNode, name: &str, default: Vec3) -> Result<Vec3, String> {
    let Some(property) = fbx_property(node, name) else {
        return Ok(default);
    };
    if property.values.len() < 3 {
        return Err(format!("Invalid FBX {name}"));
    }
    let values = &property.values[property.values.len() - 3..];
    let mut raw = [0.0; 3];
    for (dest, source) in raw.iter_mut().zip(values) {
        *dest = source.number().ok_or("Invalid FBX transform property")? as f32;
    }
    let result = Vec3::from_array(raw);
    if !result.is_finite() || result.abs().max_element() > 10_000_000.0 {
        return Err("FBX transform is outside supported bounds".into());
    }
    Ok(result)
}
fn property_integer(node: &FbxNode, name: &str, default: i64) -> i64 {
    fbx_property(node, name)
        .and_then(|p| p.values.last())
        .and_then(FbxValue::int)
        .unwrap_or(default)
}
fn fbx_euler(degrees: Vec3, order: i64) -> Result<Quat, String> {
    let axes = match order {
        0 => [0, 1, 2],
        1 => [0, 2, 1],
        2 => [1, 2, 0],
        3 => [1, 0, 2],
        4 => [2, 0, 1],
        5 => [2, 1, 0],
        _ => return Err("FBX spherical rotation order is unsupported".into()),
    };
    let q = [
        Quat::from_rotation_x(degrees.x.to_radians()),
        Quat::from_rotation_y(degrees.y.to_radians()),
        Quat::from_rotation_z(degrees.z.to_radians()),
    ];
    Ok((q[axes[2]] * q[axes[1]] * q[axes[0]]).normalize())
}
fn fbx_basis(settings: Option<&FbxNode>) -> Result<Mat3, String> {
    let Some(settings) = settings else {
        return Ok(Mat3::IDENTITY);
    };
    let axis = |name, sign, default, default_sign| -> Result<Vec3, String> {
        let component = property_integer(settings, name, default);
        let sign = property_integer(settings, sign, default_sign);
        if !(0..=2).contains(&component) || !matches!(sign, -1 | 1) {
            return Err("Invalid FBX coordinate system".into());
        }
        let mut v = Vec3::ZERO;
        v[component as usize] = sign as f32;
        Ok(v)
    };
    let right = axis("CoordAxis", "CoordAxisSign", 0, 1)?;
    let up = axis("UpAxis", "UpAxisSign", 1, 1)?;
    // FBX's front vector points at the viewer, i.e. it is glTF/Bevy +Z ("back").
    // Maya/Mixamo Y-up (Front 2, sign +1) is therefore the identity, and
    // 3ds Max Z-up (Front 1, sign -1) is a proper rotation, not a mirror.
    let back = axis("FrontAxis", "FrontAxisSign", 2, 1)?;
    let basis = Mat3::from_cols(right, up, back).transpose();
    if (basis.determinant().abs() - 1.0).abs() > 0.001 {
        return Err("Degenerate FBX coordinate system".into());
    }
    Ok(basis)
}
struct FbxCurve {
    ticks: Vec<i64>,
    values: Vec<f64>,
    constant: bool,
}
impl FbxCurve {
    fn sample(&self, tick: f64) -> f32 {
        let upper = self.ticks.partition_point(|t| (*t as f64) <= tick);
        if upper == 0 {
            return self.values[0] as f32;
        }
        if upper == self.ticks.len() || self.constant {
            return self.values[upper - 1] as f32;
        }
        let start = self.ticks[upper - 1] as f64;
        let t = ((tick - start) / (self.ticks[upper] as f64 - start)).clamp(0.0, 1.0);
        (self.values[upper - 1] * (1.0 - t) + self.values[upper] * t) as f32
    }
}
/// Native ASCII and binary FBX 7.x skeletal rotation import. Supports compressed
/// arrays, animation stacks, XYZ axis curves, pre/post rotations and axis conversion.
/// Curves are baked at 60 Hz; FBX cubic tangent easing is approximated linearly.
/// Hips translation is retargeted; constraints, geometry, animated scale and additive layers are not applied.
pub fn parse_fbx(bytes: &[u8]) -> Result<BvhClip, String> {
    const TICKS_PER_SECOND: f64 = 46_186_158_000.0;
    let tree = fbx_tree(bytes)?;
    let objects = tree
        .iter()
        .find(|n| n.name == "Objects")
        .ok_or("FBX has no Objects section")?;
    let connections = tree
        .iter()
        .find(|n| n.name == "Connections")
        .ok_or("FBX has no Connections section")?;
    let basis = fbx_basis(tree.iter().find(|n| n.name == "GlobalSettings"))?;
    let convert =
        |q| Quat::from_mat3(&(basis * Mat3::from_quat(q) * basis.transpose())).normalize();
    let mut ids = HashSet::new();
    for object in &objects.children {
        if let Some(id) = object.values.first().and_then(FbxValue::int)
            && !ids.insert(id)
        {
            return Err("FBX contains duplicate object IDs".into());
        }
    }
    let mut links = Vec::new();
    for connection in &connections.children {
        if connection.name != "C" {
            continue;
        }
        let values = &connection.values;
        if values.len() < 3 {
            return Err("Invalid FBX connection".into());
        }
        let child = values[1].int().ok_or("Invalid FBX connection source")?;
        let parent = values[2].int().ok_or("Invalid FBX connection target")?;
        let kind = values[0].text().ok_or("Invalid FBX connection type")?;
        let property = values.get(3).and_then(FbxValue::text).unwrap_or("");
        links.push((child, parent, kind, property));
    }
    // A deterministic single take and layer avoids accidentally mixing takes.
    let stack = objects
        .children
        .iter()
        .find(|n| n.name == "AnimationStack")
        .and_then(|n| n.values.first())
        .and_then(FbxValue::int);
    let layer_ids: HashSet<i64> = objects
        .children
        .iter()
        .filter(|n| n.name == "AnimationLayer")
        .filter_map(|n| n.values.first().and_then(FbxValue::int))
        .collect();
    let active_layers: HashSet<i64> = match stack {
        Some(stack) => links
            .iter()
            .filter(|(_, parent, kind, _)| *parent == stack && *kind == "OO")
            .map(|(child, _, _, _)| *child)
            .filter(|id| layer_ids.contains(id))
            .collect(),
        None => layer_ids,
    };
    if active_layers.len() > 1 {
        return Err("FBX has multiple animation layers; export a single baked layer".into());
    }
    let has_layer_selection = stack.is_some() || !active_layers.is_empty();
    let active_nodes: HashSet<i64> = links
        .iter()
        .filter(|(_, parent, kind, _)| active_layers.contains(parent) && *kind == "OO")
        .map(|(child, _, _, _)| *child)
        .collect();
    let mut joints = Vec::new();
    let mut model_ids = HashMap::new();
    let mut models = Vec::new();
    let mut mapped = HashSet::new();
    for model in objects.children.iter().filter(|n| n.name == "Model") {
        if joints.len() >= MAX_NODES {
            return Err("FBX exceeds 4096 model nodes".into());
        }
        let id = model
            .values
            .first()
            .and_then(FbxValue::int)
            .ok_or("Invalid FBX model ID")?;
        let raw_name = model
            .values
            .get(1)
            .and_then(FbxValue::text)
            .ok_or("Missing FBX model name")?;
        let name = raw_name
            .split('\0')
            .next()
            .unwrap_or(raw_name)
            .strip_prefix("Model::")
            .unwrap_or(raw_name.split('\0').next().unwrap_or(raw_name));
        let bone = map_bone(name);
        if let Some(bone) = bone
            && !mapped.insert(bone)
        {
            return Err(format!("Ambiguous FBX: multiple models map to {bone}"));
        }
        let order = property_integer(model, "RotationOrder", 0);
        let rest = property_vec3(model, "Lcl Rotation", Vec3::ZERO)?;
        let pre = fbx_euler(property_vec3(model, "PreRotation", Vec3::ZERO)?, 0)?;
        let post = fbx_euler(property_vec3(model, "PostRotation", Vec3::ZERO)?, 0)?;
        let rotation = pre * fbx_euler(rest, order)? * post.inverse();
        let translation = basis * property_vec3(model, "Lcl Translation", Vec3::ZERO)?;
        model_ids.insert(id, joints.len());
        joints.push(BvhJoint {
            name: name.chars().take(256).collect(),
            parent: None,
            offset: translation,
            bone,
            channels: Vec::new(),
            first_channel: 0,
            rest_rotation: convert(rotation),
            rest_translation: translation,
            rest_scale: Vec3::ONE,
        });
        models.push((rest, pre, post, order));
    }
    for (child, parent, kind, _) in &links {
        if *kind == "OO"
            && let (Some(&child), Some(&parent)) = (model_ids.get(child), model_ids.get(parent))
            && joints[child].parent.replace(parent).is_some()
        {
            return Err("FBX model has multiple parents".into());
        }
    }
    let mut curves = HashMap::new();
    let mut total_keys = 0usize;
    for curve in objects
        .children
        .iter()
        .filter(|n| n.name == "AnimationCurve")
    {
        let id = curve
            .values
            .first()
            .and_then(FbxValue::int)
            .ok_or("Invalid FBX curve ID")?;
        let ticks = curve
            .child("KeyTime")
            .ok_or("FBX curve has no timestamps")?
            .integers()?;
        let values = curve
            .child("KeyValueFloat")
            .or_else(|| curve.child("KeyValueDouble"))
            .ok_or("FBX curve has no values")?
            .numbers()?;
        total_keys += ticks.len();
        if ticks.is_empty()
            || ticks.len() != values.len()
            || total_keys > MAX_KEYS
            || ticks.windows(2).any(|p| p[0] >= p[1])
            || values
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 10_000_000.0)
        {
            return Err("Invalid or oversized FBX animation curve".into());
        }
        let constant = curve
            .child("KeyAttrFlags")
            .and_then(|n| n.integers().ok())
            .is_some_and(|flags| !flags.is_empty() && flags.iter().all(|flag| flag & 0xe == 2));
        curves.insert(
            id,
            FbxCurve {
                ticks,
                values,
                constant,
            },
        );
    }
    // Bind curve-node rotation groups to model indices, then bind X/Y/Z curves.
    let mut groups: HashMap<i64, (usize, bool)> = HashMap::new();
    for (child, parent, kind, property) in &links {
        if *kind == "OP"
            && matches!(*property, "Lcl Rotation" | "Lcl Translation")
            && (!has_layer_selection || active_nodes.contains(child))
            && let Some(&model) = model_ids.get(parent)
        {
            let translation = *property == "Lcl Translation";
            if !translation || joints[model].bone == Some(HumanBoneName::Hips) {
                groups.insert(*child, (model, translation));
            }
        }
    }
    let mut axes: Vec<[Option<i64>; 3]> = vec![[None; 3]; joints.len()];
    let mut translation_axes: Vec<[Option<i64>; 3]> = vec![[None; 3]; joints.len()];
    for (child, parent, kind, property) in &links {
        if *kind != "OP" || !curves.contains_key(child) {
            continue;
        }
        let Some(&(model, translation)) = groups.get(parent) else {
            continue;
        };
        let axis = match *property {
            "d|X" => 0,
            "d|Y" => 1,
            "d|Z" => 2,
            _ => continue,
        };
        let destination = if translation {
            &mut translation_axes
        } else {
            &mut axes
        };
        if destination[model][axis].replace(*child).is_some() {
            return Err("FBX has overlapping animation pose curves; bake to one layer".into());
        }
    }
    let selected: HashSet<i64> = axes
        .iter()
        .chain(&translation_axes)
        .flatten()
        .filter_map(|v| *v)
        .collect();
    let start = selected
        .iter()
        .filter_map(|id| curves[id].ticks.first().copied())
        .min()
        .ok_or("FBX has no recognized skeletal pose curves")?;
    let end = selected
        .iter()
        .filter_map(|id| curves[id].ticks.last().copied())
        .max()
        .unwrap();
    let duration = ((end as i128 - start as i128) as f64 / TICKS_PER_SECOND) as f32;
    if !duration.is_finite() || duration > 86_400.0 {
        return Err("FBX duration exceeds one day".into());
    }
    let frames = ((duration * 60.0).ceil() as usize + 1).max(2);
    let animated = axes
        .iter()
        .chain(&translation_axes)
        .filter(|axis| axis.iter().any(Option::is_some))
        .count();
    if frames > 100_000
        || frames
            .checked_mul(animated)
            .and_then(|n| n.checked_mul(4))
            .is_none_or(|n| n > MAX_SAMPLES)
    {
        return Err("FBX baked animation exceeds 100,000 frames or 4 million samples".into());
    }
    let duration = duration.max(1.0 / 60.0);
    let mut tracks = Vec::with_capacity(joints.len());
    for (model, axes) in axes.iter().enumerate() {
        if axes.iter().all(Option::is_none) {
            tracks.push(None);
            continue;
        }
        let (rest, pre, post, order) = models[model];
        let mut times = Vec::with_capacity(frames);
        let mut values = Vec::with_capacity(frames);
        for frame in 0..frames {
            let seconds = duration * frame as f32 / (frames - 1) as f32;
            let tick = start as f64 + seconds as f64 * TICKS_PER_SECOND;
            let mut euler = rest;
            for axis in 0..3 {
                if let Some(curve) = axes[axis] {
                    euler[axis] = curves[&curve].sample(tick);
                }
            }
            times.push(seconds);
            values.push(convert(pre * fbx_euler(euler, order)? * post.inverse()));
        }
        tracks.push(Some(RotationTrack {
            times,
            values,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            interpolation: Interpolation::Linear,
        }));
    }
    let mut translations = Vec::with_capacity(joints.len());
    for (model, axes) in translation_axes.iter().enumerate() {
        if axes.iter().all(Option::is_none) {
            translations.push(None);
            continue;
        }
        let mut times = Vec::with_capacity(frames);
        let mut values = Vec::with_capacity(frames);
        let rest = basis.transpose() * joints[model].rest_translation;
        for frame in 0..frames {
            let seconds = duration * frame as f32 / (frames - 1) as f32;
            let tick = start as f64 + seconds as f64 * TICKS_PER_SECOND;
            let mut position = rest;
            for axis in 0..3 {
                if let Some(curve) = axes[axis] {
                    position[axis] = curves[&curve].sample(tick);
                }
            }
            times.push(seconds);
            values.push(basis * position);
        }
        translations.push(Some(TranslationTrack {
            times,
            values,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            interpolation: Interpolation::Linear,
        }));
    }
    build_clip(joints, tracks, translations, "FBX")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn make_glb(json: &Value, binary: &[u8]) -> Vec<u8> {
        let mut json = serde_json::to_vec(json).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut binary = binary.to_owned();
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend((28u32 + json.len() as u32 + binary.len() as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(0x4e4f534au32.to_le_bytes());
        out.extend(json);
        out.extend((binary.len() as u32).to_le_bytes());
        out.extend(0x004e4942u32.to_le_bytes());
        out.extend(binary);
        out
    }
    fn vrma_fixture(interpolation: &str) -> (Value, Vec<u8>) {
        let rest = Quat::from_rotation_x(0.4);
        let end = Quat::from_rotation_y(0.8) * rest;
        let arm_rest = Quat::from_rotation_z(0.3);
        let mut binary = Vec::new();
        for time in [0.0f32, 1.0] {
            binary.extend(time.to_le_bytes());
        }
        let cubic = interpolation == "CUBICSPLINE";
        for value in [rest, end] {
            if cubic {
                binary.extend([0u8; 16]);
            }
            for value in value.to_array() {
                binary.extend(value.to_le_bytes());
            }
            if cubic {
                binary.extend([0u8; 16]);
            }
        }
        let length = binary.len() - 8;
        let json = json!({
            "asset":{"version":"2.0"},
            "extensions":{"VRMC_vrm_animation":{"specVersion":"1.0","humanoid":{"humanBones":{"hips":{"node":1},"leftUpperArm":{"node":0}}}}},
            "nodes":[{"name":"arm","rotation":arm_rest.to_array()}, {"name":"hips","rotation":rest.to_array(),"children":[0]}],
            "buffers":[{"byteLength":binary.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":8},{"buffer":0,"byteOffset":8,"byteLength":length}],
            "accessors":[{"bufferView":0,"componentType":5126,"type":"SCALAR","count":2},{"bufferView":1,"componentType":5126,"type":"VEC4","count":if cubic {6}else{2}}],
            "animations":[{"samplers":[{"input":0,"output":1,"interpolation":interpolation}],"channels":[{"sampler":0,"target":{"node":1,"path":"rotation"}}]}]
        });
        (json, binary)
    }
    #[test]
    fn vrma_uses_original_timestamps_parent_order_and_source_bind_pose() {
        let (json, binary) = vrma_fixture("LINEAR");
        let clip = parse_vrma(&make_glb(&json, &binary)).unwrap();
        assert_eq!(clip.format(), "VRMA");
        assert_eq!(clip.mapped_bones(), 2);
        assert_eq!(clip.duration(), 1.0);
        let mut sampled = Vec::new();
        clip.sample_world(0.0, false, &mut sampled);
        assert!(
            sampled
                .iter()
                .all(|q| q.angle_between(Quat::IDENTITY) < 0.0015)
        );
        clip.sample_world(0.5, false, &mut sampled);
        assert!(
            sampled
                .iter()
                .all(|q| q.angle_between(Quat::from_rotation_y(0.4)) < 0.0015)
        );
        clip.sample_world(1.0, false, &mut sampled);
        assert!(
            sampled
                .iter()
                .all(|q| q.angle_between(Quat::from_rotation_y(0.8)) < 0.0015)
        );
    }
    #[test]
    fn vrma_step_and_cubic_interpolation_preserve_declared_behavior() {
        let (json, binary) = vrma_fixture("STEP");
        let clip = parse_vrma(&make_glb(&json, &binary)).unwrap();
        let mut sampled = Vec::new();
        clip.sample_world(0.5, false, &mut sampled);
        assert!(sampled[0].angle_between(Quat::IDENTITY) < 0.0015);
        let (json, binary) = vrma_fixture("CUBICSPLINE");
        let clip = parse_vrma(&make_glb(&json, &binary)).unwrap();
        clip.sample_world(0.25, false, &mut sampled);
        let rest = Quat::from_rotation_x(0.4);
        let end = Quat::from_rotation_y(0.8) * rest;
        let h = 0.15625;
        let raw = (Vec4::from_array(rest.to_array()) * (1.0 - h)
            + Vec4::from_array(end.to_array()) * h)
            .normalize();
        let expected = Quat::from_array(raw.to_array()) * rest.inverse();
        assert!(sampled[0].angle_between(expected) < 0.0015);
    }
    fn translation_fixture(interpolation: &str) -> (Value, Vec<u8>) {
        let mut binary = Vec::new();
        for value in [0_f32, 1.] {
            binary.extend(value.to_le_bytes());
        }
        let cubic = interpolation == "CUBICSPLINE";
        for position in [Vec3::new(0.2, 1., 0.4), Vec3::new(0.4, 1.5, 0.6)] {
            if cubic {
                binary.extend([0_u8; 12]);
            }
            for component in position.to_array() {
                binary.extend(component.to_le_bytes());
            }
            if cubic {
                binary.extend([0_u8; 12]);
            }
        }
        let json = json!({
            "asset":{"version":"2.0"},
            "extensions":{"VRMC_vrm_animation":{"specVersion":"1.0","humanoid":{"humanBones":{"hips":{"node":0}}}}},
            "nodes":[{"translation":[0.2,1.0,0.4]},{"translation":[0.0,0.5,0.0],"children":[0]}],
            "buffers":[{"byteLength":binary.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":8},{"buffer":0,"byteOffset":8,"byteLength":binary.len()-8}],
            "accessors":[{"bufferView":0,"componentType":5126,"type":"SCALAR","count":2},{"bufferView":1,"componentType":5126,"type":"VEC3","count":if cubic {6}else{2}}],
            "animations":[{"samplers":[{"input":0,"output":1,"interpolation":interpolation}],"channels":[{"sampler":0,"target":{"node":0,"path":"translation"}}]}]
        });
        (json, binary)
    }
    #[test]
    fn vrma_translation_only_clips_preserve_rest_hierarchy_and_declared_interpolation() {
        for (mode, fraction) in [("LINEAR", 0.25), ("STEP", 0.), ("CUBICSPLINE", 0.15625)] {
            let (json, binary) = translation_fixture(mode);
            let clip = parse_vrma(&make_glb(&json, &binary)).unwrap();
            let hips = clip
                .joints
                .iter()
                .position(|joint| joint.bone == Some(HumanBoneName::Hips))
                .unwrap();
            assert!(clip.rest_positions[hips].abs_diff_eq(Vec3::new(0.2, 1.5, 0.4), 1e-6));
            let expected = Vec3::new(0.2, 1., 0.4).lerp(Vec3::new(0.4, 1.5, 0.6), fraction);
            assert!(
                clip.sample_translation(hips, 0.25, false)
                    .abs_diff_eq(expected, 1e-6)
            );
            assert_eq!(clip.duration(), 1.);
        }
        let (mut json, binary) = translation_fixture("LINEAR");
        let duplicate = json["animations"][0]["channels"][0].clone();
        json["animations"][0]["channels"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(parse_vrma(&make_glb(&json, &binary)).is_err());
    }
    #[test]
    fn vrma_refuses_external_buffers_cycles_bad_ranges_and_bad_samples() {
        let (json, binary) = vrma_fixture("LINEAR");
        let mut bad = json.clone();
        bad["buffers"][0]["uri"] = json!("https://example.invalid/animation.bin");
        assert!(parse_vrma(&make_glb(&bad, &binary)).is_err());
        let mut bad = json.clone();
        bad["nodes"][0]["children"] = json!([1]);
        assert!(parse_vrma(&make_glb(&bad, &binary)).is_err());
        let mut bad = json.clone();
        bad["accessors"][1]["byteOffset"] = json!(999999);
        assert!(parse_vrma(&make_glb(&bad, &binary)).is_err());
        let mut bad = json.clone();
        bad["accessors"][1]["count"] = json!(4_000_001);
        assert!(parse_vrma(&make_glb(&bad, &binary)).is_err());
        let mut bad_binary = binary.clone();
        bad_binary[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_vrma(&make_glb(&json, &bad_binary)).is_err());
        let bytes = make_glb(&json, &binary);
        for end in [0, 4, 12, 20, bytes.len() - 1] {
            assert!(parse_vrma(&bytes[..end]).is_err());
        }
    }
    const FBX: &str = r#"; FBX 7.4.0 project file
Objects: {
    Model: 1, "Model::mixamorig:Hips", "LimbNode" {
        Properties70: {
            P: "Lcl Rotation", "Lcl Rotation", "", "A", 0, 0, 0
            P: "PreRotation", "Vector3D", "Vector", "", 0, 0, 30
        }
    }
    Model: 2, "Model::mixamorig:LeftArm", "LimbNode" {
        Properties70: {
            P: "Lcl Rotation", "Lcl Rotation", "", "A", 0, 20, 0
        }
    }
    AnimationStack: 10, "AnimStack::Take 001", "" {
    }
    AnimationLayer: 11, "AnimLayer::BaseLayer", "" {
    }
    AnimationCurveNode: 20, "AnimCurveNode::R", "" {
    }
    AnimationCurve: 30, "AnimCurve::", "" {
        KeyTime: *2 {
            a: 0, 46186158000
        }
        KeyValueFloat: *2 {
            a: 0, 90
        }
    }
}
Connections: {
    C: "OO", 2, 1
    C: "OO", 11, 10
    C: "OO", 20, 11
    C: "OP", 20, 1, "Lcl Rotation"
    C: "OP", 30, 20, "d|Z"
}
"#;
    fn encode_node(node: &FbxNode, offset: usize) -> Vec<u8> {
        let mut properties = Vec::new();
        for property in &node.values {
            match property {
                FbxValue::Int(v) => {
                    properties.push(b'L');
                    properties.extend(v.to_le_bytes());
                }
                FbxValue::Number(v) => {
                    properties.push(b'D');
                    properties.extend(v.to_le_bytes());
                }
                FbxValue::Text(v) => {
                    properties.push(b'S');
                    properties.extend((v.len() as u32).to_le_bytes());
                    properties.extend(v.as_bytes());
                }
                FbxValue::Ints(v) => {
                    let mut encoder =
                        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    for v in v {
                        encoder.write_all(&v.to_le_bytes()).unwrap();
                    }
                    let packed = encoder.finish().unwrap();
                    properties.push(b'l');
                    properties.extend((v.len() as u32).to_le_bytes());
                    properties.extend(1u32.to_le_bytes());
                    properties.extend((packed.len() as u32).to_le_bytes());
                    properties.extend(packed);
                }
                _ => panic!("Test encoder does not need this property"),
            }
        }
        let mut body = node.name.as_bytes().to_vec();
        body.extend(&properties);
        for child in &node.children {
            body.extend(encode_node(child, offset + 13 + body.len()));
        }
        if !node.children.is_empty() {
            body.extend([0u8; 13]);
        }
        let mut out = Vec::new();
        out.extend(((offset + 13 + body.len()) as u32).to_le_bytes());
        out.extend((node.values.len() as u32).to_le_bytes());
        out.extend((properties.len() as u32).to_le_bytes());
        out.push(node.name.len() as u8);
        out.extend(body);
        out
    }
    #[test]
    fn fbx_ascii_and_compressed_binary_retarget_equivalently() {
        let ascii = parse_fbx(FBX.as_bytes()).unwrap();
        let mut tree = fbx_tree(FBX.as_bytes()).unwrap();
        // Turn the ASCII timestamp array into an actual compressed binary FBX array.
        let curve = tree
            .iter_mut()
            .find(|n| n.name == "Objects")
            .unwrap()
            .children
            .iter_mut()
            .find(|n| n.name == "AnimationCurve")
            .unwrap();
        let ticks = curve
            .children
            .iter_mut()
            .find(|n| n.name == "KeyTime")
            .unwrap();
        ticks.values = vec![FbxValue::Ints(vec![0, 46_186_158_000])];
        ticks.children.clear();
        let mut binary = b"Kaydara FBX Binary  \0\x1a\0".to_vec();
        binary.extend(7400u32.to_le_bytes());
        for node in &tree {
            binary.extend(encode_node(node, binary.len()));
        }
        binary.extend([0u8; 13]);
        let binary_clip = parse_fbx(&binary).unwrap();
        assert_eq!(ascii.mapped_bones(), 2);
        assert_eq!(ascii.duration(), 1.0);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for time in [0.0, 0.25, 0.5, 1.0] {
            ascii.sample_world(time, false, &mut a);
            binary_clip.sample_world(time, false, &mut b);
            assert!(a.iter().zip(&b).all(|(a, b)| a.angle_between(*b) < 0.0015));
            let expected = Quat::from_rotation_z(time * std::f32::consts::FRAC_PI_2);
            assert!(a.iter().all(|q| q.angle_between(expected) < 0.0015));
        }
        binary.pop();
        assert!(parse_fbx(&binary).is_err());
    }
    #[test]
    fn fbx_hips_translation_uses_declared_axis_basis_and_local_rest() {
        let text = FBX.replace("P: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\", 0, 0, 0", "P: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\", 0, 0, 0\n            P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\", 0, 100, 0")
            .replace("\"Lcl Rotation\"\n    C: \"OP\", 30", "\"Lcl Translation\"\n    C: \"OP\", 30")
            .replace("a: 0, 90", "a: 100, 120")
            .replace("\"d|Z\"", "\"d|Y\"");
        let clip = parse_fbx(text.as_bytes()).unwrap();
        let hips = clip
            .joints
            .iter()
            .position(|joint| joint.bone == Some(HumanBoneName::Hips))
            .unwrap();
        assert_eq!(clip.joints[hips].rest_translation, Vec3::new(0., 100., 0.));
        assert!(
            clip.sample_translation(hips, 0.5, false)
                .abs_diff_eq(Vec3::new(0., 110., 0.), 1e-5)
        );
        assert_eq!(clip.rest_positions[hips].y, 100.);
    }
    fn global_settings(axes: [(&str, i64); 6]) -> FbxNode {
        let property = |name: &str, value| FbxNode {
            name: "P".into(),
            values: vec![
                FbxValue::Text(name.into()),
                FbxValue::Text("int".into()),
                FbxValue::Text("Integer".into()),
                FbxValue::Text(String::new()),
                FbxValue::Int(value),
            ],
            children: vec![],
        };
        FbxNode {
            name: "GlobalSettings".into(),
            values: vec![],
            children: vec![FbxNode {
                name: "Properties70".into(),
                values: vec![],
                children: axes.iter().map(|(n, v)| property(n, *v)).collect(),
            }],
        }
    }
    #[test]
    fn fbx_axis_systems_convert_without_mirroring() {
        // Values read from WebWaifu4's Mixamo Idle/Wave/Thinking.fbx GlobalSettings.
        let mixamo = global_settings([
            ("UpAxis", 1),
            ("UpAxisSign", 1),
            ("FrontAxis", 2),
            ("FrontAxisSign", 1),
            ("CoordAxis", 0),
            ("CoordAxisSign", 1),
        ]);
        assert!(
            fbx_basis(Some(&mixamo))
                .unwrap()
                .abs_diff_eq(Mat3::IDENTITY, 1e-6)
        );
        let max = global_settings([
            ("UpAxis", 2),
            ("UpAxisSign", 1),
            ("FrontAxis", 1),
            ("FrontAxisSign", -1),
            ("CoordAxis", 0),
            ("CoordAxisSign", 1),
        ]);
        let basis = fbx_basis(Some(&max)).unwrap();
        assert!((basis.determinant() - 1.0).abs() < 1e-6);
        assert!((basis * Vec3::Z).abs_diff_eq(Vec3::Y, 1e-6));
        assert!((basis * Vec3::NEG_Y).abs_diff_eq(Vec3::Z, 1e-6));
        assert!(fbx_basis(None).unwrap().abs_diff_eq(Mat3::IDENTITY, 1e-6));
    }
    #[test]
    fn fbx_rejects_malformed_curves_and_ambiguous_hierarchies() {
        for text in [
            FBX.replace("0, 46186158000", "0, 0"),
            FBX.replace("a: 0, 90", "a: 0"),
            FBX.replace("Model::mixamorig:LeftArm", "Model::mixamorig:Hips"),
            FBX.replace("C: \"OO\", 2, 1", "C: \"OO\", 2, 1\n C: \"OO\", 1, 2"),
        ] {
            assert!(parse_fbx(text.as_bytes()).is_err());
        }
        assert!(parse_fbx(b"Objects: {\n").is_err());
    }
    #[test]
    fn parses_local_animation_references_when_available_without_bundling_assets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../work/bevy_vrm1/assets/vrma");
        for name in [
            "different_pose.vrma",
            "VRMA_01.vrma",
            "VRMA_02.vrma",
            "VRMA_03.vrma",
        ] {
            let path = root.join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                let clip = parse_vrma(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert!(clip.duration() > 0.0);
                assert!(clip.mapped_bones() > 10);
            }
        }
        let root = std::path::Path::new(
            "/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4/public/assets/animations",
        );
        for name in [
            "Idle.fbx",
            "Idle2.fbx",
            "Idle3.fbx",
            "Wave.fbx",
            "Thinking.fbx",
        ] {
            let path = root.join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                let clip = parse_fbx(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert!(clip.duration() > 0.0);
                assert!(clip.mapped_bones() > 10);
            }
        }
    }
}
