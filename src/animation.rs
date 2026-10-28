//! Bounded native BVH, VRMA and FBX import with humanoid pose retargeting.
//! Implements the BVH format independently; no third-party animation code is copied.
//! Rotation normalization and hips-height scaling follow VRMC_vrm_animation's
//! pose compatibility guidance, as used by bevy_vrm1 (MIT OR Apache-2.0):
//! https://github.com/not-elm/bevy_vrm1/tree/v0.7.1/src/vrma/animation
//! Application placement is excluded from all rest-space calculations.
use crate::loading::AvatarLoadState;
use bevy::{
    math::Affine3A,
    mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    prelude::*,
};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
};

#[path = "animation_import.rs"]
mod animation_import;
pub use animation_import::{parse_fbx, parse_vrma};
#[path = "animation_body.rs"]
mod animation_body;
#[path = "animation_clearance.rs"]
mod animation_clearance;

#[path = "animation_lifecycle.rs"]
mod animation_lifecycle;
use animation_lifecycle::MotionReceipt;
pub use animation_lifecycle::{MAX_MOTION_RECEIPTS, MotionObservations, MotionSourceSnapshot};

const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_SAMPLES: usize = 4_000_000;
static IMPORT_BUSY: AtomicBool = AtomicBool::new(false);

/// Admission errors distinguish temporary global-loader contention from invalid
/// paths or unavailable workers. Existing UI APIs preserve their string errors.
#[derive(Debug, PartialEq, Eq)]
pub enum MotionLoadError {
    Busy,
    Failed(String),
}
impl std::fmt::Display for MotionLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("An animation import is already running"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}
impl From<String> for MotionLoadError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}
impl From<&str> for MotionLoadError {
    fn from(value: &str) -> Self {
        Self::Failed(value.into())
    }
}

/// Original imported local transform, before runtime idle/procedural poses.
/// Procedural writers must wait until this snapshot exists before changing bones.
#[derive(Component, Clone, Copy)]
pub struct HumanoidRestTransform(pub Transform);

#[allow(clippy::type_complexity)]
fn capture_humanoid_rest(
    mut commands: Commands,
    bones: Query<(Entity, &Transform), (With<HumanBoneName>, Without<HumanoidRestTransform>)>,
) {
    for (entity, transform) in &bones {
        commands
            .entity(entity)
            .insert(HumanoidRestTransform(*transform));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Xposition,
    Yposition,
    Zposition,
    Xrotation,
    Yrotation,
    Zrotation,
}
#[derive(Debug)]
pub struct BvhJoint {
    pub name: String,
    pub parent: Option<usize>,
    pub offset: Vec3,
    pub bone: Option<HumanBoneName>,
    channels: Vec<Channel>,
    first_channel: usize,
    rest_rotation: Quat,
    rest_translation: Vec3,
    rest_scale: Vec3,
}
#[derive(Debug)]
pub struct BvhClip {
    pub joints: Vec<BvhJoint>,
    pub frame_count: usize,
    pub frame_time: f32,
    channel_count: usize,
    samples: Vec<f32>,
    tracks: Vec<Option<animation_import::RotationTrack>>,
    translation_tracks: Vec<Option<animation_import::TranslationTrack>>,
    rest_world: Vec<Quat>,
    rest_positions: Vec<Vec3>,
    rest_parents: Vec<Affine3A>,
    source_format: &'static str,
    exact_duration: Option<f32>,
}
impl BvhClip {
    pub fn duration(&self) -> f32 {
        self.exact_duration
            .unwrap_or(self.frame_count as f32 * self.frame_time)
    }
    /// Final pose time used when nonlooping playback reaches the clip end.
    /// BVH includes a frame interval after its last sample in `duration()`.
    pub fn last_sample_time(&self) -> f32 {
        self.exact_duration
            .unwrap_or((self.frame_count - 1) as f32 * self.frame_time)
    }
    pub fn format(&self) -> &'static str {
        self.source_format
    }
    pub fn mapped_bones(&self) -> usize {
        self.joints.iter().filter(|j| j.bone.is_some()).count()
    }
    /// Source-model geometry for diagnostics, including unmapped helper joints.
    pub fn sampled_skeleton(&self, seconds: f32, looping: bool) -> Vec<(Vec3, Quat)> {
        let mut local = Vec::new();
        self.sample_local(seconds, looping, &mut local);
        let mut globals: Vec<Affine3A> = Vec::with_capacity(local.len());
        let mut result = Vec::with_capacity(local.len());
        for (index, joint) in self.joints.iter().enumerate() {
            let transform = Transform {
                translation: self.sample_translation(index, seconds, looping),
                rotation: local[index],
                scale: joint.rest_scale,
            }
            .compute_affine();
            let global = joint
                .parent
                .map_or(transform, |parent| globals[parent] * transform);
            let (_, rotation, position) = global.to_scale_rotation_translation();
            globals.push(global);
            result.push((position, rotation));
        }
        result
    }
    pub fn rest_skeleton(&self) -> Vec<(Vec3, Quat)> {
        self.rest_positions
            .iter()
            .copied()
            .zip(self.rest_world.iter().copied())
            .collect()
    }
    /// Whether the source skeleton faces -Z at rest, judged by which side its left
    /// limbs are on. `None` when offsets are unavailable (VRMA) or ambiguous; callers
    /// then use the +Z convention shared by Mixamo, VRMA and VRM 1.0.
    pub fn faces_negative_z(&self) -> Option<bool> {
        let mut positions: Vec<Vec3> = Vec::with_capacity(self.joints.len());
        for (index, joint) in self.joints.iter().enumerate() {
            let position = match joint.parent {
                Some(parent) if parent < index => {
                    let rotation = self
                        .rest_world
                        .get(parent)
                        .copied()
                        .unwrap_or(Quat::IDENTITY);
                    positions[parent] + rotation * joint.offset
                }
                Some(_) => return None,
                None => Vec3::ZERO,
            };
            positions.push(position);
        }
        facing_from_sides(|bone| {
            self.joints
                .iter()
                .position(|joint| joint.bone == Some(bone))
                .map(|index| positions[index])
        })
    }
    #[cfg(test)]
    fn sample_world(&self, seconds: f32, looping: bool, output: &mut Vec<Quat>) {
        self.sample_local(seconds, looping, output);
        for (index, joint) in self.joints.iter().enumerate() {
            output[index] = joint
                .parent
                .map_or(output[index], |parent| output[parent] * output[index]);
        }
        for (rotation, rest) in output.iter_mut().zip(&self.rest_world) {
            *rotation = (*rotation * rest.inverse()).normalize();
        }
    }
    fn sample_local(&self, seconds: f32, looping: bool, output: &mut Vec<Quat>) {
        let frame = (seconds.max(0.0) / self.frame_time).min(self.frame_count as f32);
        let a = (frame.floor() as usize).min(self.frame_count - 1);
        let b = if a + 1 < self.frame_count {
            a + 1
        } else if looping {
            0
        } else {
            a
        };
        let fraction = frame.fract();
        output.resize(self.joints.len(), Quat::IDENTITY);
        for (index, joint) in self.joints.iter().enumerate() {
            let local = |frame: usize| {
                let mut rotation = Quat::IDENTITY;
                for (offset, channel) in joint.channels.iter().enumerate() {
                    let radians = self.samples
                        [frame * self.channel_count + joint.first_channel + offset]
                        .to_radians();
                    rotation *= match channel {
                        Channel::Xrotation => Quat::from_rotation_x(radians),
                        Channel::Yrotation => Quat::from_rotation_y(radians),
                        Channel::Zrotation => Quat::from_rotation_z(radians),
                        _ => Quat::IDENTITY,
                    };
                }
                rotation.normalize()
            };
            let rotation = if self.tracks.is_empty() {
                local(a).slerp(local(b), fraction)
            } else {
                self.tracks[index]
                    .as_ref()
                    .map_or(joint.rest_rotation, |track| track.sample(seconds))
            };
            output[index] = rotation;
        }
    }
    fn sample_translation(&self, index: usize, seconds: f32, looping: bool) -> Vec3 {
        let joint = &self.joints[index];
        if !self.translation_tracks.is_empty() {
            return self.translation_tracks[index]
                .as_ref()
                .map_or(joint.rest_translation, |track| track.sample(seconds));
        }
        let frame = (seconds.max(0.) / self.frame_time).min(self.frame_count as f32);
        let a = (frame.floor() as usize).min(self.frame_count - 1);
        let b = if a + 1 < self.frame_count {
            a + 1
        } else if looping {
            0
        } else {
            a
        };
        self.bvh_translation(index, a)
            .lerp(self.bvh_translation(index, b), frame.fract())
    }
    fn bvh_translation(&self, index: usize, frame: usize) -> Vec3 {
        let joint = &self.joints[index];
        let mut position = joint.offset;
        for (offset, channel) in joint.channels.iter().enumerate() {
            let value = self.samples[frame * self.channel_count + joint.first_channel + offset];
            match channel {
                Channel::Xposition => position.x += value,
                Channel::Yposition => position.y += value,
                Channel::Zposition => position.z += value,
                _ => {}
            }
        }
        position
    }
    fn populate_rest_geometry(&mut self) -> Result<(), String> {
        let mut globals: Vec<GlobalTransform> = Vec::with_capacity(self.joints.len());
        self.rest_positions.clear();
        self.rest_parents.clear();
        self.rest_world.clear();
        for joint in &self.joints {
            let local = Transform {
                translation: joint.rest_translation,
                rotation: joint.rest_rotation,
                scale: joint.rest_scale,
            };
            self.rest_parents.push(
                joint
                    .parent
                    .and_then(|p| globals.get(p))
                    .map(GlobalTransform::affine)
                    .unwrap_or(Affine3A::IDENTITY),
            );
            let global = match joint.parent {
                Some(parent) if parent < globals.len() => globals[parent].mul_transform(local),
                Some(_) => return Err("Animation skeleton is not parent-first".into()),
                None => GlobalTransform::from(local),
            };
            let (_, rotation, position) = global.to_scale_rotation_translation();
            if !position.is_finite()
                || position.abs().max_element() > 10_000_000.
                || !rotation.is_finite()
            {
                return Err("Animation rest skeleton is nonfinite or out of range".into());
            }
            self.rest_world.push(rotation.normalize());
            self.rest_positions.push(position);
            globals.push(global);
        }
        Ok(())
    }
}

/// Left/right humanoid pairs used to tell which way a rest pose faces.
const SIDE_PAIRS: [(HumanBoneName, HumanBoneName); 4] = [
    (HumanBoneName::LeftUpperArm, HumanBoneName::RightUpperArm),
    (HumanBoneName::LeftShoulder, HumanBoneName::RightShoulder),
    (HumanBoneName::LeftUpperLeg, HumanBoneName::RightUpperLeg),
    (HumanBoneName::LeftHand, HumanBoneName::RightHand),
];

/// A Y-up character facing +Z has its left side at +X; facing -Z (VRM 0.x in
/// glTF space) puts it at -X. Returns `Some(true)` for -Z, `None` if unclear.
fn facing_from_sides(position: impl Fn(HumanBoneName) -> Option<Vec3>) -> Option<bool> {
    SIDE_PAIRS.iter().find_map(|&(left, right)| {
        let lateral = position(left)? - position(right)?;
        (lateral.x.abs() > 1e-4 && lateral.x.abs() > lateral.z.abs()).then_some(lateral.x < 0.0)
    })
}

/// Re-expresses a rotation for a skeleton turned 180° about Y (WebWaifu4's
/// VRM 0.x `-x, y, -z, w` conversion).
fn turn_around(rotation: Quat) -> Quat {
    Quat::from_xyzw(-rotation.x, rotation.y, -rotation.z, rotation.w)
}

struct Tokens<'a> {
    input: &'a str,
    cursor: usize,
}
impl<'a> Tokens<'a> {
    fn next(&mut self) -> Result<&'a str, String> {
        let bytes = self.input.as_bytes();
        while self.cursor < bytes.len() && bytes[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
        let start = self.cursor;
        if start == bytes.len() {
            return Err("Unexpected end of BVH".into());
        }
        if matches!(bytes[start], b'{' | b'}') {
            self.cursor += 1;
        } else {
            while self.cursor < bytes.len()
                && !bytes[self.cursor].is_ascii_whitespace()
                && !matches!(bytes[self.cursor], b'{' | b'}')
            {
                self.cursor += 1;
            }
        }
        Ok(&self.input[start..self.cursor])
    }
    fn expect(&mut self, expected: &str) -> Result<(), String> {
        let found = self.next()?;
        if found == expected {
            Ok(())
        } else {
            Err(format!("Expected {expected}, found {found}"))
        }
    }
    fn number(&mut self) -> Result<f32, String> {
        let value: f32 = self
            .next()?
            .parse()
            .map_err(|_| "Invalid BVH numeric value")?;
        if !value.is_finite() || value.abs() > 10_000_000.0 {
            return Err("BVH numeric value is nonfinite or out of range".into());
        }
        Ok(value)
    }
    fn count(&mut self) -> Result<usize, String> {
        self.next()?.parse().map_err(|_| "Invalid BVH count".into())
    }
    fn offset(&mut self) -> Result<Vec3, String> {
        self.expect("OFFSET")?;
        Ok(Vec3::new(self.number()?, self.number()?, self.number()?))
    }
}

pub fn parse_bvh(input: &str) -> Result<BvhClip, String> {
    if input.len() as u64 > MAX_FILE {
        return Err("BVH exceeds the 32 MiB file limit".into());
    }
    let mut tokens = Tokens {
        input: input.trim_start_matches('\u{feff}'),
        cursor: 0,
    };
    tokens.expect("HIERARCHY")?;
    tokens.expect("ROOT")?;
    let mut joints = Vec::new();
    let mut channels = 0;
    parse_joint(&mut tokens, None, &mut joints, &mut channels, 0)?;
    tokens.expect("MOTION")?;
    tokens.expect("Frames:")?;
    let frame_count = tokens.count()?;
    if frame_count == 0 || frame_count > 100_000 {
        return Err("BVH frame count must be between 1 and 100,000".into());
    }
    tokens.expect("Frame")?;
    tokens.expect("Time:")?;
    let frame_time = tokens.number()?;
    if !(0.000_01..=10.0).contains(&frame_time) {
        return Err("BVH frame time is outside supported bounds".into());
    }
    let sample_count = frame_count
        .checked_mul(channels)
        .filter(|count| *count <= MAX_SAMPLES)
        .ok_or("BVH exceeds the 4 million sample limit")?;
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        samples.push(tokens.number()?);
    }
    if !tokens.input[tokens.cursor..].trim().is_empty() {
        return Err("BVH contains extra motion samples or trailing data".into());
    }
    let mut mapped = HashSet::new();
    for joint in &joints {
        if let Some(bone) = joint.bone
            && !mapped.insert(bone)
        {
            return Err(format!("Ambiguous BVH: multiple joints map to {bone}"));
        }
    }
    if mapped.is_empty() {
        return Err("BVH has no recognized humanoid joint names".into());
    }
    let mut clip = BvhClip {
        joints,
        frame_count,
        frame_time,
        channel_count: channels,
        samples,
        tracks: Vec::new(),
        translation_tracks: Vec::new(),
        rest_world: Vec::new(),
        rest_parents: Vec::new(),
        rest_positions: Vec::new(),
        source_format: "BVH",
        exact_duration: None,
    };
    // BVH position channels include a capture-space origin. Use frame zero as
    // the local translation baseline, retaining subsequent motion deltas.
    for index in 0..clip.joints.len() {
        clip.joints[index].rest_translation = clip.bvh_translation(index, 0);
    }
    clip.populate_rest_geometry()?;
    Ok(clip)
}
fn parse_joint(
    tokens: &mut Tokens<'_>,
    parent: Option<usize>,
    joints: &mut Vec<BvhJoint>,
    channel_count: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if joints.len() >= 256 || depth >= 64 {
        return Err("BVH exceeds 256 joints or 64 hierarchy levels".into());
    }
    let name = tokens.next()?.to_owned();
    if name.len() > 256 || matches!(name.as_str(), "{" | "}") {
        return Err("Invalid BVH joint name".into());
    }
    tokens.expect("{")?;
    let offset = tokens.offset()?;
    tokens.expect("CHANNELS")?;
    let count = tokens.count()?;
    if count > 6 {
        return Err("BVH joint has more than six channels".into());
    }
    let mut channels = Vec::with_capacity(count);
    for _ in 0..count {
        let channel = match tokens.next()? {
            "Xposition" => Channel::Xposition,
            "Yposition" => Channel::Yposition,
            "Zposition" => Channel::Zposition,
            "Xrotation" => Channel::Xrotation,
            "Yrotation" => Channel::Yrotation,
            "Zrotation" => Channel::Zrotation,
            name => return Err(format!("Unsupported BVH channel {name}")),
        };
        if channels.contains(&channel) {
            return Err("Duplicate BVH channel".into());
        }
        channels.push(channel);
    }
    let index = joints.len();
    let first_channel = *channel_count;
    *channel_count += count;
    joints.push(BvhJoint {
        bone: map_bone(&name),
        name,
        parent,
        offset,
        channels,
        first_channel,
        rest_rotation: Quat::IDENTITY,
        rest_translation: offset,
        rest_scale: Vec3::ONE,
    });
    loop {
        match tokens.next()? {
            "JOINT" => parse_joint(tokens, Some(index), joints, channel_count, depth + 1)?,
            "End" => {
                tokens.expect("Site")?;
                tokens.expect("{")?;
                tokens.offset()?;
                tokens.expect("}")?;
            }
            "}" => return Ok(()),
            unexpected => return Err(format!("Unexpected BVH hierarchy token {unexpected}")),
        }
    }
}

/// Canonical VRM and common Mixamo names; matching ignores namespace/punctuation.
pub fn map_bone(name: &str) -> Option<HumanBoneName> {
    use HumanBoneName::*;
    const BONES: &[HumanBoneName] = &[
        Hips,
        Spine,
        Chest,
        UpperChest,
        Neck,
        Head,
        LeftEye,
        RightEye,
        Jaw,
        LeftUpperLeg,
        LeftLowerLeg,
        LeftFoot,
        LeftToes,
        RightUpperLeg,
        RightLowerLeg,
        RightFoot,
        RightToes,
        LeftShoulder,
        LeftUpperArm,
        LeftLowerArm,
        LeftHand,
        RightShoulder,
        RightUpperArm,
        RightLowerArm,
        RightHand,
        LeftThumbMetacarpal,
        LeftThumbProximal,
        LeftThumbDistal,
        LeftIndexProximal,
        LeftIndexIntermediate,
        LeftIndexDistal,
        LeftMiddleProximal,
        LeftMiddleIntermediate,
        LeftMiddleDistal,
        LeftRingProximal,
        LeftRingIntermediate,
        LeftRingDistal,
        LeftLittleProximal,
        LeftLittleIntermediate,
        LeftLittleDistal,
        RightThumbMetacarpal,
        RightThumbProximal,
        RightThumbDistal,
        RightIndexProximal,
        RightIndexIntermediate,
        RightIndexDistal,
        RightMiddleProximal,
        RightMiddleIntermediate,
        RightMiddleDistal,
        RightRingProximal,
        RightRingIntermediate,
        RightRingDistal,
        RightLittleProximal,
        RightLittleIntermediate,
        RightLittleDistal,
    ];
    let normalized: String = name
        .rsplit(':')
        .next()
        .unwrap_or(name)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    let normalized = normalized.strip_prefix("mixamorig").unwrap_or(&normalized);
    if let Some(bone) = BONES
        .iter()
        .find(|bone| bone.as_str().to_ascii_lowercase() == normalized)
    {
        return Some(*bone);
    }
    let alias = match normalized {
        "pelvis" => "hips",
        "spine1" => "chest",
        "spine2" => "upperChest",
        "leftarm" => "leftUpperArm",
        "leftforearm" => "leftLowerArm",
        "rightarm" => "rightUpperArm",
        "rightforearm" => "rightLowerArm",
        "leftupleg" => "leftUpperLeg",
        "leftleg" => "leftLowerLeg",
        "rightupleg" => "rightUpperLeg",
        "rightleg" => "rightLowerLeg",
        "lefttoebase" => "leftToes",
        "righttoebase" => "rightToes",
        _ => "",
    };
    if !alias.is_empty() {
        return BONES.iter().find(|bone| bone.as_str() == alias).copied();
    }
    for side in ["left", "right"] {
        if let Some(finger) = normalized.strip_prefix(&format!("{side}hand")) {
            for (source, target) in [
                ("thumb", "Thumb"),
                ("index", "Index"),
                ("middle", "Middle"),
                ("ring", "Ring"),
                ("pinky", "Little"),
            ] {
                if let Some(number) = finger.strip_prefix(source) {
                    let segment = match (source, number) {
                        ("thumb", "1") => "Metacarpal",
                        ("thumb", "2") => "Proximal",
                        (_, "1") => "Proximal",
                        (_, "2") => "Intermediate",
                        (_, "3") => "Distal",
                        _ => continue,
                    };
                    let target = format!("{side}{target}{segment}");
                    return BONES.iter().find(|bone| bone.as_str() == target).copied();
                }
            }
        }
    }
    None
}

type ImportResult = Result<(PathBuf, BvhClip), String>;

/// Opaque identity for one host-authorized playback request, independent of its
/// asset path. Tokens are unique across AnimationState replacements as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaybackToken(u64);

impl PlaybackToken {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("playback token identities exhausted"),
        )
    }
}

/// Independently retained terminal observation for one owned playback request.
/// Terminal status never changes; retaining this clone does not retain a clip.
#[derive(Clone, Debug)]
pub struct PlaybackReceipt(Arc<AtomicU8>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaybackOutcome {
    Pending = 0,
    Completed = 1,
    Cancelled = 2,
    Failed = 3,
}

impl PlaybackReceipt {
    fn new() -> Self {
        Self(Arc::new(AtomicU8::new(PlaybackOutcome::Pending as u8)))
    }
    pub fn outcome(&self) -> PlaybackOutcome {
        match self.0.load(Ordering::Acquire) {
            0 => PlaybackOutcome::Pending,
            1 => PlaybackOutcome::Completed,
            2 => PlaybackOutcome::Cancelled,
            _ => PlaybackOutcome::Failed,
        }
    }
    fn finish(&self, outcome: PlaybackOutcome) {
        debug_assert_ne!(outcome, PlaybackOutcome::Pending);
        let _ = self.0.compare_exchange(
            PlaybackOutcome::Pending as u8,
            outcome as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

#[derive(Clone)]
struct ClipPlayback {
    clip: Arc<BvhClip>,
    path: Option<PathBuf>,
    position: f32,
    looping: bool,
    speed: f32,
    weight: f32,
    playing: bool,
}
#[derive(Default)]
struct ParkedPaths {
    watched: HashSet<PathBuf>,
    revoked: HashSet<PathBuf>,
}
#[derive(Resource)]
pub struct AnimationState {
    motion_observations: MotionObservations,
    motion_playback: Option<MotionReceipt>,
    motion_import: Option<MotionReceipt>,
    pub clip: Option<Arc<BvhClip>>,
    pub path: Option<PathBuf>,
    importing_path: Option<PathBuf>,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    /// Animation influence; zero restores the captured rest pose.
    pub weight: f32,
    /// Experimental mesh-derived arm correction; off pending movement-path review.
    pub arm_clearance: bool,
    pub position: f32,
    pub loading: bool,
    pub error: Option<String>,
    pending: Option<Mutex<mpsc::Receiver<ImportResult>>>,
    stopped: bool,
    revision: u64,
    autoplay_import: bool,
    transition_seconds: f32,
    transition_from: Option<ClipPlayback>,
    revoked_paths: HashSet<PathBuf>,
    parked_paths: Option<Arc<Mutex<ParkedPaths>>>,
    import_one_shot: bool,
    import_blend: bool,
    one_shot: bool,
    one_shot_finished: bool,
    manual_hold: bool,
    playback_owner: Option<PlaybackToken>,
    import_owner: Option<PlaybackToken>,
    playback_receipt: Option<PlaybackReceipt>,
    import_receipt: Option<PlaybackReceipt>,
}
impl Default for AnimationState {
    fn default() -> Self {
        Self {
            motion_observations: MotionObservations::default(),
            motion_playback: None,
            motion_import: None,
            clip: None,
            path: None,
            importing_path: None,
            playing: false,
            looping: true,
            speed: 1.0,
            weight: 1.0,
            arm_clearance: false,
            position: 0.0,
            loading: false,
            error: None,
            pending: None,
            stopped: true,
            revision: 0,
            autoplay_import: false,
            transition_seconds: 0.,
            transition_from: None,
            revoked_paths: HashSet::new(),
            parked_paths: None,
            import_one_shot: false,
            import_blend: false,
            one_shot: false,
            one_shot_finished: false,
            manual_hold: false,
            playback_owner: None,
            import_owner: None,
            playback_receipt: None,
            import_receipt: None,
        }
    }
}
impl AnimationState {
    pub fn motion_observations(&self) -> MotionObservations {
        self.motion_observations.clone()
    }
    /// Preserve the visible pose without synthetic play/pause admissions.
    pub fn show_scrub_pose(&mut self) {
        self.stopped = false;
        self.playing = false;
    }
    /// Park logical requests as cancelled while preserving the legacy clip/time,
    /// import delivery and manual playback flags in the parked resource.
    pub fn park_for_desktop(&mut self) -> Self {
        self.cancel_owned_requests();
        if let Some(receipt) = self.motion_playback.take() {
            receipt.finish(PlaybackOutcome::Cancelled);
        }
        if let Some(receipt) = self.motion_import.take() {
            receipt.finish(PlaybackOutcome::Cancelled);
        }
        let mut fresh = Self::default();
        fresh.motion_observations = self.motion_observations.clone();
        fresh.revoked_paths = std::mem::take(&mut self.revoked_paths);
        let paths = Arc::new(Mutex::new(ParkedPaths {
            watched: self
                .path
                .iter()
                .chain(self.importing_path.iter())
                .chain(
                    self.transition_from
                        .iter()
                        .filter_map(|from| from.path.as_ref()),
                )
                .cloned()
                .collect(),
            revoked: HashSet::new(),
        }));
        self.parked_paths = Some(paths.clone());
        fresh.parked_paths = Some(paths);
        std::mem::replace(self, fresh)
    }
    pub fn restore_parked(&mut self, mut previous: Self) {
        self.motion_observations.attach(&mut previous);
        previous
            .revoked_paths
            .extend(std::mem::take(&mut self.revoked_paths));
        if let Some(paths) = previous.parked_paths.take() {
            let revoked: Vec<_> = paths
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .revoked
                .iter()
                .cloned()
                .collect();
            previous.revoke_paths(&revoked);
        }
        if previous.loading {
            previous.motion_import = Some(
                previous
                    .motion_observations
                    .admit(MotionSourceSnapshot::default()),
            );
        }
        if previous.playing {
            previous.motion_playback = Some(
                previous
                    .motion_observations
                    .admit(MotionSourceSnapshot::default()),
            );
            if let Some(receipt) = &previous.motion_playback {
                receipt.start();
            }
        }
        *self = previous;
    }

    /// Obtain a clone while the request occupies the current/import slot. Keep
    /// it to observe terminal status after selection, idle handoff or state drop;
    /// the state maintains no unbounded history indexed by old tokens.
    pub fn owned_playback_receipt(&self, token: PlaybackToken) -> Option<PlaybackReceipt> {
        if self.import_owner == Some(token) {
            self.import_receipt.clone()
        } else if self.playback_owner == Some(token) {
            self.playback_receipt.clone()
        } else {
            None
        }
    }
    fn finish_playback(&mut self, outcome: PlaybackOutcome) {
        if let Some(receipt) = self.motion_playback.take() {
            receipt.finish(outcome);
        }
        if let Some(receipt) = self.playback_receipt.take() {
            receipt.finish(outcome);
        }
        self.playback_owner = None;
    }
    fn finish_import(&mut self, outcome: PlaybackOutcome) {
        if let Some(receipt) = self.motion_import.take() {
            receipt.finish(outcome);
        }
        if let Some(receipt) = self.import_receipt.take() {
            receipt.finish(outcome);
        }
        self.import_owner = None;
    }
    pub fn load_path(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        self.load_attributed_path(path, MotionSourceSnapshot::default())
            .map_err(|error| error.to_string())
    }
    fn load_attributed_path(
        &mut self,
        path: impl AsRef<Path>,
        source: MotionSourceSnapshot,
    ) -> Result<(), MotionLoadError> {
        source.validate()?;
        let path = path.as_ref().to_owned();
        if !path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
            ["bvh", "vrma", "fbx"]
                .iter()
                .any(|extension| s.eq_ignore_ascii_case(extension))
        }) {
            return Err("Choose a .bvh, .vrma or .fbx motion file".into());
        }
        if self.loading
            || IMPORT_BUSY
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(MotionLoadError::Busy);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let importing_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name("studio-animation".into())
            .spawn(move || {
                struct Permit;
                impl Drop for Permit {
                    fn drop(&mut self) {
                        IMPORT_BUSY.store(false, Ordering::Release);
                    }
                }
                let _permit = Permit;
                let result = (|| {
                    let mut input = Vec::new();
                    let file = std::fs::File::open(&path)
                        .map_err(|e| format!("Cannot open animation: {e}"))?;
                    let metadata = file
                        .metadata()
                        .map_err(|e| format!("Cannot inspect animation: {e}"))?;
                    if !metadata.is_file() || metadata.len() > MAX_FILE {
                        return Err("Animation must be a regular file at most 32 MiB".into());
                    }
                    file.take(MAX_FILE + 1)
                        .read_to_end(&mut input)
                        .map_err(|e| format!("Cannot read animation: {e}"))?;
                    if input.len() as u64 > MAX_FILE {
                        return Err("Animation exceeds the 32 MiB file limit".into());
                    }
                    let extension = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let clip = match extension.as_str() {
                        "vrma" => parse_vrma(&input)?,
                        "fbx" => parse_fbx(&input)?,
                        _ => parse_bvh(
                            std::str::from_utf8(&input)
                                .map_err(|_| "BVH must contain UTF-8 text")?,
                        )?,
                    };
                    Ok((path, clip))
                })();
                let _ = tx.send(result);
            });
        if let Err(error) = spawned {
            IMPORT_BUSY.store(false, Ordering::Release);
            return Err(MotionLoadError::Failed(format!(
                "Cannot start animation import: {error}"
            )));
        }
        self.begin_import(importing_path, rx);
        self.motion_import = Some(self.motion_observations.admit(source));
        Ok(())
    }
    fn begin_import(&mut self, path: PathBuf, receiver: mpsc::Receiver<ImportResult>) {
        // Acceptance of a newer import supersedes the prior request even when
        // its clip remains visible until the new import finishes.
        self.finish_playback(PlaybackOutcome::Cancelled);
        self.finish_import(PlaybackOutcome::Cancelled);
        self.pending = Some(Mutex::new(receiver));
        self.importing_path = Some(path);
        self.loading = true;
        self.autoplay_import = true;
        self.import_one_shot = false;
        self.import_blend = false;
        self.error = None;
        self.manual_hold = true;
    }
    /// Playlist transitions use the same rest-space crossfade as preloaded clips.
    pub fn load_playlist(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        self.load_path(path)?;
        self.import_blend = true;
        self.manual_hold = false;
        Ok(())
    }
    /// Reply and emotion gestures play once, then crossfade to the bundled idle.
    pub fn load_one_shot(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        self.try_load_one_shot(path)
            .map_err(|error| error.to_string())
    }
    pub fn try_load_one_shot(&mut self, path: impl AsRef<Path>) -> Result<(), MotionLoadError> {
        self.load_attributed_path(path, MotionSourceSnapshot::default())?;
        self.import_one_shot = true;
        self.manual_hold = false;
        Ok(())
    }
    /// Import a host-authorized one-shot. A failed admission leaves the current
    /// request intact. Cancellation drops delivery, not the bounded I/O worker;
    /// its global import permit stays occupied until that worker exits.
    pub fn load_owned_one_shot(&mut self, path: impl AsRef<Path>) -> Result<PlaybackToken, String> {
        self.load_owned_one_shot_attributed(path, MotionSourceSnapshot::default())
    }
    pub fn load_owned_one_shot_attributed(
        &mut self,
        path: impl AsRef<Path>,
        source: MotionSourceSnapshot,
    ) -> Result<PlaybackToken, String> {
        self.try_load_owned_one_shot_attributed(path, source)
            .map_err(|error| error.to_string())
    }
    /// Typed admission for integrations; Busy creates no new playback receipt.
    pub fn try_load_owned_one_shot_attributed(
        &mut self,
        path: impl AsRef<Path>,
        source: MotionSourceSnapshot,
    ) -> Result<PlaybackToken, MotionLoadError> {
        self.load_attributed_path(path, source)?;
        self.import_one_shot = true;
        self.manual_hold = false;
        let token = PlaybackToken::new();
        self.import_owner = Some(token);
        self.import_receipt = Some(PlaybackReceipt::new());
        Ok(token)
    }
    /// Select a preloaded host-authorized one-shot with a fresh request identity.
    /// Callers enforce manual/desktop ownership before invoking this method.
    pub fn select_owned_one_shot(
        &mut self,
        clip: Arc<BvhClip>,
        path: Option<PathBuf>,
    ) -> PlaybackToken {
        self.select_one_shot(clip, path);
        self.manual_hold = false;
        let token = PlaybackToken::new();
        self.playback_owner = Some(token);
        self.playback_receipt = Some(PlaybackReceipt::new());
        token
    }
    /// Cancel only this request. A stale token cannot stop a newer selection,
    /// manual takeover, or replay of the same file. Cancelling a pending import
    /// preserves the unrelated visible clip and any explicit manual hold.
    pub fn cancel_owned_playback(&mut self, token: PlaybackToken) -> bool {
        let pending = self.import_owner == Some(token);
        let playing = self.playback_owner == Some(token);
        if pending {
            self.pending = None;
            self.importing_path = None;
            self.finish_import(PlaybackOutcome::Cancelled);
            self.loading = false;
            self.import_one_shot = false;
            self.import_blend = false;
        }
        if playing {
            self.stop();
        }
        pending || playing
    }
    pub fn owned_playback_active(&self, token: PlaybackToken) -> bool {
        (self.loading && self.import_owner == Some(token))
            || (self.playback_owner == Some(token) && self.gesture_active())
    }
    /// Ownership transfer (such as parking desktop playback) must invalidate
    /// plugin requests before moving this state out of the live host resource.
    pub fn cancel_owned_requests(&mut self) {
        if let Some(token) = self.import_owner {
            self.cancel_owned_playback(token);
        }
        if let Some(token) = self.playback_owner {
            self.cancel_owned_playback(token);
        }
    }
    /// Explicit controls supersede automatic imports and reaction handoffs.
    /// The visible pose/playback is retained; the caller applies pause/scrub/etc.
    pub fn take_manual_control(&mut self) {
        self.finish_playback(PlaybackOutcome::Cancelled);
        if self.loading && (self.import_one_shot || self.import_blend) {
            self.pending = None;
            self.importing_path = None;
            self.finish_import(PlaybackOutcome::Cancelled);
            self.loading = false;
            self.import_one_shot = false;
            self.import_blend = false;
        }
        self.one_shot = false;
        self.one_shot_finished = false;
        self.manual_hold = true;
    }
    pub fn manual_controlled(&self) -> bool {
        self.manual_hold
    }
    pub fn resume_automatic_control(&mut self) {
        self.manual_hold = false;
    }
    /// Discard work for a departed character/avatar without releasing explicit
    /// user ownership. Dropping the receiver rejects late worker completion.
    pub fn cancel_automatic_motion(&mut self) {
        if self.manual_hold {
            return;
        }
        self.pending = None;
        self.importing_path = None;
        self.finish_import(PlaybackOutcome::Cancelled);
        self.loading = false;
        self.import_one_shot = false;
        self.import_blend = false;
        self.stop();
    }
    /// Revoked pack assets cannot finish a late import or retain a pose. Manual
    /// ownership survives so disabling a pack never resumes automatic playback.
    pub fn revoke_paths(&mut self, paths: &[PathBuf]) -> bool {
        if let Some(parked) = &self.parked_paths {
            let mut parked = parked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for path in paths {
                if parked.watched.contains(path) {
                    parked.revoked.insert(path.clone());
                }
            }
        }
        let current = self.path.as_ref().is_some_and(|p| paths.contains(p));
        let incoming = self.loading
            && self
                .importing_path
                .as_ref()
                .is_some_and(|p| paths.contains(p));
        // An outgoing crossfade may still reference a revoked pack. Its pose
        // contribution ends immediately without cancelling a different import.
        self.revoked_paths.extend(paths.iter().cloned());
        if self
            .transition_from
            .as_ref()
            .is_some_and(|from| from.path.as_ref().is_some_and(|path| paths.contains(path)))
        {
            self.transition_from = None;
            self.transition_seconds = 0.;
        }
        if !current && !incoming {
            return false;
        }
        let preserve_import = self.loading && !incoming;
        let autoplay = self.autoplay_import;
        if incoming {
            self.pending = None;
            self.importing_path = None;
            self.finish_import(PlaybackOutcome::Cancelled);
            self.loading = false;
            self.import_one_shot = false;
            self.import_blend = false;
        }
        if !current {
            return incoming;
        }
        let preserved_owner = if preserve_import {
            self.import_owner.take()
        } else {
            None
        };
        let preserved_motion = if preserve_import {
            self.motion_import.take()
        } else {
            None
        };
        let preserved_receipt = if preserve_import {
            self.import_receipt.take()
        } else {
            None
        };
        self.stop();
        self.clip = None;
        self.path = None;
        self.revision = self.revision.wrapping_add(1);
        if preserve_import {
            self.import_owner = preserved_owner;
            self.import_receipt = preserved_receipt;
            self.motion_import = preserved_motion;
            self.autoplay_import = autoplay;
        }

        true
    }
    /// Play a preloaded reaction once using the normal crossfade and idle handoff.
    pub fn select_one_shot(&mut self, clip: Arc<BvhClip>, path: Option<PathBuf>) {
        self.activate_one_shot(clip, path, None);
    }
    fn activate_one_shot(
        &mut self,
        clip: Arc<BvhClip>,
        path: Option<PathBuf>,
        receipt: Option<MotionReceipt>,
    ) {
        self.activate_clip(clip, path, receipt);
        self.looping = false;
        self.one_shot = true;
    }
    pub fn play(&mut self) {
        if self.clip.is_some() {
            self.finish_playback(PlaybackOutcome::Cancelled);
            self.playing = true;
            self.stopped = false;
            self.motion_playback = Some(
                self.motion_observations
                    .admit(MotionSourceSnapshot::default()),
            );
            if let Some(receipt) = &self.motion_playback {
                receipt.start();
            }
        }
    }
    /// Switch a preloaded motion without file I/O or an asynchronous import.
    /// Starts at time zero with a 0.5-second crossfade from the live prior clip.
    pub fn select_clip(&mut self, clip: Arc<BvhClip>, path: Option<PathBuf>) {
        self.activate_clip(clip, path, None);
    }
    fn activate_clip(
        &mut self,
        clip: Arc<BvhClip>,
        path: Option<PathBuf>,
        receipt: Option<MotionReceipt>,
    ) {
        self.finish_playback(PlaybackOutcome::Cancelled);
        self.finish_import(PlaybackOutcome::Cancelled);
        self.transition_from = if self.is_posing() {
            self.clip.clone().map(|clip| ClipPlayback {
                clip,
                path: self.path.clone(),
                position: self.position,
                looping: self.looping,
                speed: self.speed,
                weight: self.weight,
                playing: self.playing,
            })
        } else {
            None
        };
        self.transition_seconds = 0.5;
        self.pending = None;
        self.clip = Some(clip);
        self.importing_path = None;
        self.path = path;
        self.position = 0.0;
        self.loading = false;
        self.error = None;
        self.revision = self.revision.wrapping_add(1);
        self.autoplay_import = true;
        self.speed = 1.0;
        self.weight = 1.0;
        self.looping = true;
        self.one_shot = false;
        self.one_shot_finished = false;
        self.playing = true;
        self.stopped = false;
        self.motion_playback = Some(receipt.unwrap_or_else(|| {
            self.motion_observations
                .admit(MotionSourceSnapshot::default())
        }));
        if let Some(receipt) = &self.motion_playback {
            receipt.start();
        }
    }
    pub fn pause(&mut self) {
        self.finish_playback(PlaybackOutcome::Cancelled);
        self.playing = false;
    }
    pub fn stop(&mut self) {
        // Keep legacy pending-delivery/token cancellation behavior, but a stop
        // already terminalizes the owned request before any worker completes.
        if let Some(receipt) = &self.motion_import {
            receipt.finish(PlaybackOutcome::Cancelled);
        }
        if let Some(receipt) = &self.import_receipt {
            receipt.finish(PlaybackOutcome::Cancelled);
        }
        self.finish_playback(PlaybackOutcome::Cancelled);
        self.playing = false;
        self.stopped = true;
        self.autoplay_import = false;
        self.one_shot = false;
        self.one_shot_finished = false;
        self.position = 0.0;
        self.transition_from = None;
        self.transition_seconds = 0.;
    }
    /// Automatic playlists must let a requested emotion gesture finish first.
    pub fn gesture_active(&self) -> bool {
        (self.loading && self.import_one_shot) || (self.one_shot && !self.one_shot_finished)
    }
    pub fn is_posing(&self) -> bool {
        self.clip.is_some() && (self.playing || !self.stopped)
    }
}

impl Drop for AnimationState {
    fn drop(&mut self) {
        self.finish_import(PlaybackOutcome::Cancelled);
        self.finish_playback(PlaybackOutcome::Cancelled);
    }
}

pub struct BvhAnimationPlugin;
impl Plugin for BvhAnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimationState>();
        let observations = app
            .world()
            .get_resource::<MotionObservations>()
            .cloned()
            .unwrap_or_else(|| {
                app.world()
                    .resource::<AnimationState>()
                    .motion_observations()
            });
        observations.attach(&mut app.world_mut().resource_mut::<AnimationState>());
        app.insert_resource(observations);
        app.init_resource::<PoseCache>().add_systems(
            Update,
            (capture_humanoid_rest, poll_import, animate_humanoid).chain(),
        );
    }
}
fn poll_import(mut state: ResMut<AnimationState>) {
    let result = state
        .pending
        .as_ref()
        .and_then(|rx| rx.lock().ok())
        .map(|rx| rx.try_recv());
    match result {
        Some(Ok(Ok((path, clip)))) => {
            let owner = state.import_owner.take();
            let receipt = state.import_receipt.take();
            let generic = state.motion_import.take();
            if state.import_one_shot && state.autoplay_import {
                state.activate_one_shot(Arc::new(clip), Some(path), generic);
                state.playback_owner = owner;
                state.playback_receipt = receipt;
                return;
            }
            if let Some(receipt) = receipt {
                receipt.finish(PlaybackOutcome::Cancelled);
            }
            if state.import_blend && state.autoplay_import {
                state.activate_clip(Arc::new(clip), Some(path), generic);
                return;
            }
            // Manual file imports preserve their immediate switch semantics.
            // The visible clip may have been replayed while this import waited.
            state.finish_playback(PlaybackOutcome::Cancelled);
            state.transition_seconds = 0.;
            state.transition_from = None;
            state.path = Some(path);
            state.clip = Some(Arc::new(clip));
            state.position = 0.0;
            state.loading = false;
            state.pending = None;
            state.importing_path = None;
            state.error = None;
            state.revision = state.revision.wrapping_add(1);
            state.one_shot = false;
            state.one_shot_finished = false;
            state.motion_playback = generic;
            if state.autoplay_import {
                state.playing = true;
                state.stopped = false;
                if let Some(receipt) = &state.motion_playback {
                    receipt.start();
                }
            } else {
                state.stop();
            }
        }
        Some(Ok(Err(error))) => {
            state.finish_import(PlaybackOutcome::Failed);
            state.error = Some(error);
            state.loading = false;
            state.pending = None;
            state.importing_path = None;
        }
        Some(Err(mpsc::TryRecvError::Disconnected)) => {
            state.finish_import(PlaybackOutcome::Failed);
            state.error = Some("Animation import worker exited unexpectedly".into());
            state.loading = false;
            state.pending = None;
            state.importing_path = None;
        }
        _ => {}
    }
}
struct PoseBone {
    entity: Entity,
    bone: HumanBoneName,
    rest: Transform,
    /// Visible idle pose to restore when stopping or when a clip omits this bone.
    idle: Transform,
    rest_rotation: Quat,
    rest_position: Vec3,
    model_to_parent: Option<Affine3A>,
    joint: Option<usize>,
    depth: usize,
}
#[derive(Resource, Default)]
struct PoseCache {
    root: Option<Entity>,
    /// Revisions are local to AnimationState; replacing the whole resource may
    /// reuse a revision for a different clip (desktop mode saves/restores it).
    clip: Option<Arc<BvhClip>>,
    path: Option<PathBuf>,
    bones: Vec<PoseBone>,
    revision: u64,
    applied: bool,
    sampled: Vec<Quat>,
    last_position: f32,
    last_weight: f32,
    last_arm_clearance: bool,
    body: animation_body::MeshBodyCache,
    /// The active avatar's rest facing; `Some(true)` = -Z, as VRM 0.x models are.
    target_faces_negative_z: Option<bool>,
    /// Source and avatar face opposite ways, so rotations are turned around Y.
    turn_around: bool,
    transition: Option<PoseTransition>,
}
struct PoseTransition {
    from: Vec<Transform>,
    outgoing: Option<ClipPlayback>,
    elapsed: f32,
    duration: f32,
    /// Includes every source captured by an interrupted mixed-pose snapshot.
    paths: HashSet<PathBuf>,
}

/// Local rest-space conversion from VRMC_vrm_animation pose compatibility:
/// source local -> normalized humanoid rotation -> destination raw local joint.
/// Globals here are within the imported model, never desktop/world placement.
fn normalized_pose(source_local_rest: Quat, source_model_rest: Quat, pose: Quat) -> Quat {
    (source_model_rest * source_local_rest.inverse() * pose * source_model_rest.inverse())
        .normalize()
}
fn destination_pose(local_rest: Quat, model_rest: Quat, normalized: Quat) -> Quat {
    (local_rest * model_rest.inverse() * normalized * model_rest).normalize()
}
#[allow(clippy::too_many_arguments)]
fn clip_pose(
    clip: &BvhClip,
    index: usize,
    sampled: Quat,
    seconds: f32,
    looping: bool,
    bone: &PoseBone,
    reverse_facing: bool,
    weight: f32,
) -> Transform {
    let source = &clip.joints[index];
    let mut normalized = normalized_pose(source.rest_rotation, clip.rest_world[index], sampled);
    if reverse_facing {
        normalized = turn_around(normalized);
    }
    let mut result = bone.rest;
    let weight = if weight.is_finite() {
        weight.clamp(0., 1.)
    } else {
        1.
    };
    result.rotation = bone.rest.rotation.slerp(
        destination_pose(bone.rest.rotation, bone.rest_rotation, normalized),
        weight,
    );
    if bone.bone == HumanBoneName::Hips {
        let mut delta = clip.sample_translation(index, seconds, looping) - source.rest_translation;
        // Translation tracks live in the source parent frame. Normalize the
        // displacement into model space before facing/height conversion.
        delta = clip.rest_parents[index].transform_vector3(delta);
        if reverse_facing {
            delta.x = -delta.x;
            delta.z = -delta.z;
        }
        // Model-space rest heights exclude the application root's translation,
        // rotation and scale. A zero-height rig cannot provide a reliable ratio.
        let source_height = clip.rest_positions[index].y.abs();
        let target_height = bone.rest_position.y.abs();
        let scaling = if source_height > 1e-5 && target_height > 1e-5 {
            (target_height / source_height).clamp(0.0001, 10_000.)
        } else {
            1.
        };
        let Some(model_to_parent) = bone.model_to_parent else {
            return result;
        };
        let position =
            bone.rest.translation + model_to_parent.transform_vector3(delta * scaling * weight);
        if position.is_finite() && position.abs().max_element() <= 10_000. {
            result.translation = position;
        }
    }
    result
}
fn relative_rotation(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
    rests: &Query<&HumanoidRestTransform>,
) -> Option<(Quat, usize)> {
    let mut current = entity;
    let mut rotation = Quat::IDENTITY;
    for depth in 0..128 {
        if current == root {
            return Some((rotation, depth));
        }
        rotation = rests
            .get(current)
            .map(|rest| rest.0.rotation)
            .or_else(|_| transforms.get(current).map(|transform| transform.rotation))
            .ok()?
            * rotation;
        current = parents.get(current).ok()?.parent();
    }
    None
}
/// Position of `entity` in the avatar root's local frame.
fn relative_position(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
    rests: &Query<&HumanoidRestTransform>,
) -> Option<Vec3> {
    let mut current = entity;
    let mut position = Vec3::ZERO;
    for _ in 0..128 {
        if current == root {
            return Some(position);
        }
        position = rests
            .get(current)
            .map(|rest| rest.0)
            .or_else(|_| transforms.get(current).copied())
            .ok()?
            .transform_point(position);
        current = parents.get(current).ok()?.parent();
    }
    None
}
/// Invert only the imported parent hierarchy; application placement is excluded.
/// Singular/nonfinite armatures cannot safely translate a joint.
fn parent_inverse(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
    rests: &Query<&HumanoidRestTransform>,
) -> Option<Affine3A> {
    let mut current = parents.get(entity).ok()?.parent();
    let mut affine = Affine3A::IDENTITY;
    for _ in 0..128 {
        if current == root {
            let determinant = affine.matrix3.determinant();
            if !affine.is_finite() || !determinant.is_finite() || determinant.abs() < 1e-12 {
                return None;
            }
            let inverse = affine.inverse();
            return inverse.is_finite().then_some(inverse);
        }
        let transform = rests
            .get(current)
            .map(|r| r.0)
            .or_else(|_| transforms.get(current).copied())
            .ok()?;
        affine = transform.compute_affine() * affine;
        current = parents.get(current).ok()?.parent();
    }
    None
}
#[allow(clippy::too_many_arguments)]
fn animate_humanoid(
    time: Res<Time>,
    avatar: Res<AvatarLoadState>,
    mut state: ResMut<AnimationState>,
    mut cache: ResMut<PoseCache>,
    bones: Query<(Entity, &HumanBoneName)>,
    parents: Query<&ChildOf>,
    mut transforms: Query<&mut Transform>,
    rests: Query<&HumanoidRestTransform>,
    meshes: Option<Res<Assets<Mesh>>>,
    bindposes: Option<Res<Assets<SkinnedMeshInverseBindposes>>>,
    mesh_entities: Query<(Entity, &Mesh3d, &SkinnedMesh)>,
) {
    if cache.root != avatar.active {
        if cache.root.is_some() {
            state.cancel_automatic_motion();
        }
        for bone in &cache.bones {
            if let Ok(mut transform) = transforms.get_mut(bone.entity) {
                *transform = bone.idle;
            }
        }
        cache.bones.clear();
        cache.sampled.clear();
        cache.root = avatar.active;
        cache.clip = None;
        cache.path = None;
        cache.applied = false;
        cache.transition = None;
        cache.body = default();
        state.position = 0.0;
    }
    // Revocation also invalidates an outgoing clip already moved into the cache.
    let revoked = std::mem::take(&mut state.revoked_paths);
    let Some(root) = avatar.active else { return };
    let revoked_transition = cache
        .transition
        .as_ref()
        .is_some_and(|transition| transition.paths.iter().any(|path| revoked.contains(path)));
    let revoked_cached_pose = cache
        .path
        .as_ref()
        .is_some_and(|path| revoked.contains(path));
    if revoked_cached_pose {
        cache.clip = None;
        cache.path = None;
    }
    if revoked_transition || revoked_cached_pose {
        cache.transition = None;
        // A same-frame selection may have a clean logical outgoing clip even
        // while the last rendered mixed pose contains a revoked ancestor.
        // Rebase its fade on that clean clip instead of cancelling the request.
        let clean_outgoing = state.transition_from.as_ref().is_some_and(|from| {
            from.path
                .as_ref()
                .is_some_and(|path| !revoked.contains(path))
        });
        if !clean_outgoing {
            state.transition_from = None;
            state.transition_seconds = 0.;
        }
        if state.is_posing() {
            cache.applied = false;
        }
    }
    if state.one_shot_finished && !state.loading {
        static IDLE: LazyLock<Result<Arc<BvhClip>, String>> = LazyLock::new(|| {
            parse_vrma(include_bytes!(
                "../assets/animations/desktop/idle-maid.vrma"
            ))
            .map(Arc::new)
        });
        match &*IDLE {
            Ok(clip) => state.select_clip(
                clip.clone(),
                Some("assets/animations/desktop/idle-maid.vrma".into()),
            ),
            Err(error) => {
                state.stop();
                state.error = Some(format!("Cannot return to idle: {error}"));
            }
        }
    }
    // Build the active mapping lazily, using imported rests captured before idle
    // arm relaxation or any other procedural pose, including on inactive avatars.
    if cache.bones.is_empty() && state.is_posing() {
        for (entity, name) in &bones {
            // Body clips often contain dummy SOMA/BVH eye channels. Eyes belong
            // to the face/gaze runtime, including during blends, stop and reload.
            // Exclude them from the cache entirely so fallback/rest writes cannot
            // undo that ownership even when the incoming clip omits the eyes.
            if matches!(name, HumanBoneName::LeftEye | HumanBoneName::RightEye) {
                continue;
            }
            if let Some((rotation, depth)) =
                relative_rotation(entity, root, &parents, &transforms, &rests)
                && let Some(position) =
                    relative_position(entity, root, &parents, &transforms, &rests)
                && let Ok(rest) = rests.get(entity)
                && let Ok(idle) = transforms.get(entity)
            {
                cache.bones.push(PoseBone {
                    entity,
                    bone: *name,
                    rest: rest.0,
                    idle: *idle,
                    rest_rotation: rotation,
                    rest_position: position,
                    model_to_parent: parent_inverse(entity, root, &parents, &transforms, &rests),
                    joint: None,
                    depth,
                });
            }
        }
        cache.bones.sort_by_key(|bone| bone.depth);
        let facing = facing_from_sides(|name| {
            let bone = cache.bones.iter().find(|bone| bone.bone == name)?;
            relative_position(bone.entity, root, &parents, &transforms, &rests)
        });
        cache.target_faces_negative_z = facing;
        cache.revision = state.revision.wrapping_sub(1);
    }
    if !state.is_posing() {
        cache.transition = None;
        cache.clip = None;
        cache.path = None;
        if cache.applied {
            for bone in &cache.bones {
                if let Ok(mut transform) = transforms.get_mut(bone.entity) {
                    *transform = bone.idle;
                }
            }
            cache.applied = false;
        }
        return;
    }
    let Some(clip) = state.clip.clone() else {
        return;
    };
    if cache.revision != state.revision
        || cache
            .clip
            .as_ref()
            .is_none_or(|cached| !Arc::ptr_eq(cached, &clip))
    {
        let from = cache
            .bones
            .iter()
            .map(|bone| {
                transforms
                    .get(bone.entity)
                    .map_or(bone.rest, |transform| *transform)
            })
            .collect();
        // A rapid second selection starts at the last visible mixed pose, so it
        // cannot jump back to the outgoing clip's unmixed frame.
        let previous = cache.transition.take();
        let outgoing = if previous.is_some() {
            state.transition_from = None;
            None
        } else {
            state.transition_from.take()
        };
        let mut paths = previous.map_or_else(HashSet::new, |transition| transition.paths);
        paths.extend(cache.path.iter().cloned());
        if let Some(outgoing) = &outgoing {
            paths.extend(outgoing.path.iter().cloned());
        }
        // Repeatedly interrupted blends keep bounded source attribution. At the
        // bound, finish the blend rather than retaining an unbounded ancestry.
        cache.transition = if paths.len() <= 64
            && state.transition_seconds > 0.
            && state.transition_seconds.is_finite()
        {
            Some(PoseTransition {
                from,
                outgoing,
                elapsed: 0.,
                duration: state.transition_seconds.clamp(0.001, 5.),
                paths,
            })
        } else {
            None
        };
        state.transition_seconds = 0.;
        // A replacement clip may omit bones animated by its predecessor.
        for bone in &cache.bones {
            if let Ok(mut transform) = transforms.get_mut(bone.entity) {
                *transform = bone.rest;
            }
        }
        cache.applied = false;
        let mapped: HashMap<_, _> = clip
            .joints
            .iter()
            .enumerate()
            .filter_map(|(index, joint)| joint.bone.map(|bone| (bone, index)))
            .collect();
        for bone in &mut cache.bones {
            bone.joint = mapped.get(&bone.bone).copied();
        }
        cache.turn_around = clip.faces_negative_z().unwrap_or(false)
            != cache.target_faces_negative_z.unwrap_or(false);
        cache.revision = state.revision;
        cache.clip = Some(clip.clone());
        cache.path = state.path.clone();
    }
    if state.playing {
        state.stopped = false;
        let speed = if state.speed.is_finite() {
            state.speed.clamp(0.0, 4.0)
        } else {
            1.0
        };
        state.position += time.delta_secs().min(0.25) * speed;
        if state.position >= clip.duration() {
            if state.looping && !state.one_shot {
                state.position %= clip.duration();
            } else {
                state.position = clip.last_sample_time();
                state.finish_playback(PlaybackOutcome::Completed);
                state.pause();
                state.one_shot_finished = state.one_shot;
            }
        }
    }
    if !state.position.is_finite() {
        state.position = 0.0;
    }
    let weight = if state.weight.is_finite() {
        state.weight.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if !state.playing
        && cache.applied
        && cache.transition.is_none()
        && cache.last_position == state.position
        && cache.last_weight == weight
        && cache.last_arm_clearance == state.arm_clearance
    {
        return;
    }
    clip.sample_local(state.position, state.looping, &mut cache.sampled);
    let mut transition = cache.transition.take();
    let mut outgoing_sampled = Vec::new();
    let alpha = if let Some(transition) = &mut transition {
        let dt = time.delta_secs().clamp(0., 0.25);
        transition.elapsed += dt;
        if let Some(outgoing) = &mut transition.outgoing {
            if outgoing.playing {
                let speed = if outgoing.speed.is_finite() {
                    outgoing.speed.clamp(0., 4.)
                } else {
                    1.
                };
                outgoing.position += dt * speed;
                if outgoing.position >= outgoing.clip.duration() {
                    outgoing.position = if outgoing.looping {
                        outgoing.position % outgoing.clip.duration()
                    } else {
                        outgoing.clip.duration()
                    };
                }
            }
            outgoing
                .clip
                .sample_local(outgoing.position, outgoing.looping, &mut outgoing_sampled);
        }
        (transition.elapsed / transition.duration).clamp(0., 1.)
    } else {
        1.
    };
    for (bone_index, bone) in cache.bones.iter().enumerate() {
        let mut pose = bone.joint.map_or(bone.idle, |index| {
            clip_pose(
                &clip,
                index,
                cache.sampled[index],
                state.position,
                state.looping,
                bone,
                cache.turn_around,
                weight,
            )
        });
        if let Some(transition) = &transition {
            let from = if let Some(outgoing) = &transition.outgoing {
                outgoing
                    .clip
                    .joints
                    .iter()
                    .position(|joint| joint.bone == Some(bone.bone))
                    .map_or(bone.idle, |index| {
                        let reverse = outgoing.clip.faces_negative_z().unwrap_or(false)
                            != cache.target_faces_negative_z.unwrap_or(false);
                        clip_pose(
                            &outgoing.clip,
                            index,
                            outgoing_sampled[index],
                            outgoing.position,
                            outgoing.looping,
                            bone,
                            reverse,
                            outgoing.weight,
                        )
                    })
            } else {
                transition.from[bone_index]
            };
            pose.rotation = from.rotation.slerp(pose.rotation, alpha);
            pose.translation = from.translation.lerp(pose.translation, alpha);
        }
        if let Ok(mut transform) = transforms.get_mut(bone.entity) {
            transform.rotation = pose.rotation.normalize();
            transform.translation = pose.translation;
        }
    }
    if alpha < 1. {
        cache.transition = transition;
    }
    cache.last_position = state.position;
    cache.last_weight = weight;
    cache.last_arm_clearance = state.arm_clearance;
    if state.arm_clearance && (weight > 0. || alpha < 1.) {
        let PoseCache { body, bones, .. } = &mut *cache;
        if let (Some(meshes), Some(bindposes)) = (meshes.as_deref(), bindposes.as_deref())
            && body.capture(
                root,
                bones,
                meshes,
                bindposes,
                &mesh_entities,
                &parents,
                &transforms,
            )
            && let Some(points) = body.posed_points(root, &parents, &transforms)
            && let Some(hull) = animation_clearance::Hull::from_points(&points)
        {
            animation_clearance::correct_arms(
                root,
                bones,
                &parents,
                &mut transforms,
                &hull,
                body.arm_radii(),
            );
        }
    }
    cache.applied = true;
}

#[cfg(test)]
#[path = "animation_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_playback_blends_stop_and_switch_leave_both_eyes_untouched() {
        use bevy::time::TimeUpdateStrategy;
        let text = "HIERARCHY\nROOT hips\n{\nOFFSET 0 1 0\nCHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation\nJOINT leftEye\n{\nOFFSET 0.03 0.6 0\nCHANNELS 3 Zrotation Xrotation Yrotation\nEnd Site\n{\nOFFSET 0 0.01 0\n}\n}\nJOINT rightEye\n{\nOFFSET -0.03 0.6 0\nCHANNELS 3 Zrotation Xrotation Yrotation\nEnd Site\n{\nOFFSET 0 0.01 0\n}\n}\n}\nMOTION\nFrames: 2\nFrame Time: 0.1\n0 0 0 0 0 0 0 90 0 0 90 0\n0 0 0 0 0 0 0 -90 0 0 -90 0\n";
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(20),
            ))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let hips = app
            .world_mut()
            .spawn((
                HumanBoneName::Hips,
                Transform::from_xyz(0., 1., 0.),
                ChildOf(root),
            ))
            .id();
        let eyes = [HumanBoneName::LeftEye, HumanBoneName::RightEye].map(|bone| {
            app.world_mut()
                .spawn((bone, Transform::from_xyz(0., 0.6, 0.), ChildOf(hips)))
                .id()
        });
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        let owned = Quat::from_rotation_x(0.13);
        for eye in eyes {
            app.world_mut().get_mut::<Transform>(eye).unwrap().rotation = owned;
        }
        let clip = Arc::new(parse_bvh(text).unwrap());
        assert_eq!(clip.mapped_bones(), 3, "fixture really contains eye tracks");
        for path in ["first.bvh", "second.bvh"] {
            app.world_mut()
                .resource_mut::<AnimationState>()
                .select_clip(clip.clone(), Some(path.into()));
            for _ in 0..15 {
                app.update();
                for eye in eyes {
                    assert!(
                        app.world()
                            .get::<Transform>(eye)
                            .unwrap()
                            .rotation
                            .abs_diff_eq(owned, 1e-6)
                    );
                }
            }
        }
        app.world_mut().resource_mut::<AnimationState>().stop();
        app.update();
        for eye in eyes {
            assert!(
                app.world()
                    .get::<Transform>(eye)
                    .unwrap()
                    .rotation
                    .abs_diff_eq(owned, 1e-6)
            );
        }
    }
    const SAMPLE: &str = "HIERARCHY ROOT hips { OFFSET 0 0 0 CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation JOINT leftUpperArm { OFFSET 1 1 0 CHANNELS 3 Zrotation Xrotation Yrotation End Site { OFFSET 1 0 0 } } } MOTION Frames: 2 Frame Time: 0.1 0 0 0 0 0 0 0 0 0 100 200 300 90 0 0 0 0 0";

    // Control worker completion deterministically while exercising the real
    // import admission state, delivery system and public cancellation API.
    fn deferred_owned_import(
        state: &mut AnimationState,
        path: &Path,
    ) -> (PlaybackToken, mpsc::SyncSender<ImportResult>) {
        let (tx, rx) = mpsc::sync_channel(1);
        state.begin_import(path.to_owned(), rx);
        state.import_one_shot = true;
        state.manual_hold = false;
        let token = PlaybackToken::new();
        state.import_owner = Some(token);
        state.import_receipt = Some(PlaybackReceipt::new());
        (token, tx)
    }

    #[test]
    fn typed_motion_admission_distinguishes_busy_from_invalid_without_new_request() {
        let mut state = AnimationState::default();
        state.loading = true;
        assert_eq!(
            state
                .try_load_owned_one_shot_attributed("fixture.bvh", MotionSourceSnapshot::default()),
            Err(MotionLoadError::Busy)
        );
        assert!(state.import_owner.is_none());
        assert!(state.import_receipt.is_none());
        assert!(matches!(
            state
                .try_load_owned_one_shot_attributed("fixture.txt", MotionSourceSnapshot::default()),
            Err(MotionLoadError::Failed(_))
        ));
        assert!(state.loading);
    }

    #[test]
    fn retained_receipts_cancel_on_replacement_stop_parking_and_state_drop() {
        fn token_is_copy<T: Copy>() {}
        token_is_copy::<PlaybackToken>();
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        let mut state = AnimationState::default();
        let old = state.select_owned_one_shot(clip.clone(), None);
        let old_receipt = state.owned_playback_receipt(old).unwrap();
        let retained = old_receipt.clone();
        assert_eq!(retained.outcome(), PlaybackOutcome::Pending);
        let new = state.select_owned_one_shot(clip.clone(), None);
        let new_receipt = state.owned_playback_receipt(new).unwrap();
        assert_eq!(old_receipt.outcome(), PlaybackOutcome::Cancelled);
        assert_eq!(
            Arc::strong_count(&old_receipt.0),
            2,
            "only independently retained clones remain"
        );
        assert!(!state.cancel_owned_playback(old));
        assert_eq!(new_receipt.outcome(), PlaybackOutcome::Pending);
        state.select_clip(clip.clone(), Some("manual.bvh".into()));
        assert_eq!(new_receipt.outcome(), PlaybackOutcome::Cancelled);
        state.stop();
        assert_eq!(retained.outcome(), PlaybackOutcome::Cancelled);
        assert!(state.owned_playback_receipt(old).is_none());
        for control in 0..4 {
            let token = state.select_owned_one_shot(clip.clone(), None);
            let receipt = state.owned_playback_receipt(token).unwrap();
            match control {
                0 => state.take_manual_control(),
                1 => state.pause(),
                2 => state.stop(),
                _ => state.cancel_owned_requests(),
            }
            assert_eq!(receipt.outcome(), PlaybackOutcome::Cancelled);
        }
        let token = state.select_owned_one_shot(clip, None);
        let receipt = state.owned_playback_receipt(token).unwrap();
        drop(state);
        assert_eq!(receipt.outcome(), PlaybackOutcome::Cancelled);
    }

    #[test]
    fn revoking_old_visible_path_preserves_unrelated_owned_import_receipt() {
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        let (token, tx, receipt) = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.select_clip(Arc::new(parse_bvh(SAMPLE).unwrap()), Some("old.bvh".into()));
            let (token, tx) = deferred_owned_import(&mut state, Path::new("new.bvh"));
            let receipt = state.owned_playback_receipt(token).unwrap();
            assert!(state.revoke_paths(&["old.bvh".into()]));
            assert_eq!(receipt.outcome(), PlaybackOutcome::Pending);
            (token, tx, receipt)
        };
        tx.send(Ok(("new.bvh".into(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        assert!(state.owned_playback_active(token));
        assert_eq!(receipt.outcome(), PlaybackOutcome::Pending);
        state.cancel_owned_requests();
        assert_eq!(receipt.outcome(), PlaybackOutcome::Cancelled);
    }

    #[test]
    fn parse_failure_and_worker_disconnect_publish_failed_receipts() {
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        for disconnected in [false, true] {
            let (token, tx) = deferred_owned_import(
                &mut app.world_mut().resource_mut::<AnimationState>(),
                Path::new("broken.bvh"),
            );
            let receipt = app
                .world()
                .resource::<AnimationState>()
                .owned_playback_receipt(token)
                .unwrap();
            if disconnected {
                drop(tx);
            } else {
                let error = parse_bvh("HIERARCHY ROOT truncated").err().unwrap();
                tx.send(Err(error)).unwrap();
            }
            app.update();
            assert_eq!(receipt.outcome(), PlaybackOutcome::Failed);
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            assert!(state.error.is_some());
            assert!(!state.cancel_owned_playback(token));
            state.select_clip(Arc::new(parse_bvh(SAMPLE).unwrap()), None);
            state.stop();
            assert_eq!(receipt.outcome(), PlaybackOutcome::Failed);
        }
    }

    #[test]
    fn cancelled_pending_import_receipt_rejects_late_delivery_and_cannot_cancel_newer() {
        let mut state = AnimationState::default();
        let (token, tx) = deferred_owned_import(&mut state, Path::new("late.bvh"));
        let receipt = state.owned_playback_receipt(token).unwrap();
        state.cancel_owned_requests();
        assert_eq!(receipt.outcome(), PlaybackOutcome::Cancelled);
        assert!(
            tx.send(Ok(("late.bvh".into(), parse_bvh(SAMPLE).unwrap())))
                .is_err()
        );
        let (new, _tx) = deferred_owned_import(&mut state, Path::new("new.bvh"));
        let new_receipt = state.owned_playback_receipt(new).unwrap();
        assert!(!state.cancel_owned_playback(token));
        assert_eq!(new_receipt.outcome(), PlaybackOutcome::Pending);
        state.stop();
        assert_eq!(new_receipt.outcome(), PlaybackOutcome::Cancelled);
        // The existing stop behavior still permits explicit late-delivery discard.
        assert!(state.cancel_owned_playback(new));
    }

    #[test]
    fn owned_import_cancellation_rejects_late_delivery_and_preserves_visible_playback() {
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        let visible = Arc::new(parse_bvh(SAMPLE).unwrap());
        let path = Path::new("pack/reaction.bvh");
        let (token, tx) = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.select_clip(visible.clone(), Some(path.to_owned()));
            state.position = 0.1;
            deferred_owned_import(&mut state, path)
        };
        app.update(); // Nothing delivered yet; the old clip remains visible.
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            assert!(state.loading && state.playing);
            state.play(); // Replay the visible file while the older import waits.
            assert!(state.cancel_owned_playback(token));
            assert!(!state.cancel_owned_playback(token));
            assert!(!state.loading && state.pending.is_none() && !state.gesture_active());
            assert!(state.playing && state.is_posing());
            assert!(Arc::ptr_eq(state.clip.as_ref().unwrap(), &visible));
            assert_eq!(state.path.as_deref(), Some(path));
            assert_eq!(state.position, 0.1);
        }
        assert!(
            tx.send(Ok((path.to_owned(), parse_bvh(SAMPLE).unwrap())))
                .is_err()
        );
        app.update();
        assert!(app.world().resource::<AnimationState>().playing);
        assert_eq!(
            app.world().resource::<AnimationState>().path.as_deref(),
            Some(path)
        );
    }

    #[test]
    fn owned_import_transfers_token_to_playback_and_cancellation_stops_it() {
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        let path = Path::new("pack/reaction.bvh");
        let (token, tx) =
            deferred_owned_import(&mut app.world_mut().resource_mut::<AnimationState>(), path);
        // Failed busy admission must leave this request's ownership intact.
        assert!(
            app.world_mut()
                .resource_mut::<AnimationState>()
                .load_owned_one_shot("another.bvh")
                .is_err()
        );
        tx.send(Ok((path.to_owned(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        assert!(!state.loading && state.playing && state.gesture_active());
        assert_eq!(state.path.as_deref(), Some(path));
        assert!(state.cancel_owned_playback(token));
        assert!(!state.is_posing() && !state.gesture_active());
        assert!(state.pending.is_none() && state.transition_from.is_none());
        assert!(!state.cancel_owned_playback(token));
    }

    #[test]
    fn stale_tokens_cannot_stop_newer_owned_manual_or_automatic_playback_of_same_file() {
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        let path = Some(PathBuf::from("shared.bvh"));
        let mut state = AnimationState::default();
        let old = state.select_owned_one_shot(clip.clone(), path.clone());
        let new = state.select_owned_one_shot(clip.clone(), path.clone());
        assert_ne!(old, new);
        assert!(!state.cancel_owned_playback(old));
        assert!(state.playing && state.gesture_active());
        assert!(state.cancel_owned_playback(new));

        let old = state.select_owned_one_shot(clip.clone(), path.clone());
        state.select_clip(clip.clone(), path.clone());
        state.take_manual_control();
        state.pause();
        assert!(!state.cancel_owned_playback(old));
        assert!(state.manual_controlled() && state.is_posing() && !state.playing);

        state.resume_automatic_control();
        let old = state.select_owned_one_shot(clip.clone(), path.clone());
        state.select_one_shot(clip.clone(), path.clone());
        assert!(!state.cancel_owned_playback(old));
        assert!(state.playing && state.gesture_active());

        // Taking over the existing clip needs no path/clip change at all.
        let old = state.select_owned_one_shot(clip.clone(), path.clone());
        state.take_manual_control();
        assert!(!state.cancel_owned_playback(old));
        assert!(state.manual_controlled() && state.playing && !state.gesture_active());
        let old = state.select_owned_one_shot(clip.clone(), path.clone());
        state.play();
        assert!(!state.cancel_owned_playback(old));
        assert!(state.playing && Arc::ptr_eq(state.clip.as_ref().unwrap(), &clip));
        assert_eq!(state.path, path);

        let old = state.select_owned_one_shot(clip.clone(), path.clone());
        state = AnimationState::default();
        let new = state.select_owned_one_shot(clip, path);
        assert!(!state.cancel_owned_playback(old));
        assert!(state.playing && state.cancel_owned_playback(new));
    }

    #[test]
    fn same_file_selection_supersedes_deferred_request_and_preserves_newer_import() {
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        let path = Path::new("shared.bvh");
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        let (old, tx) = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            deferred_owned_import(&mut state, path)
        };
        // The worker has finished but its result has not been polled.
        tx.send(Ok((path.to_owned(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.select_clip(clip.clone(), Some(path.to_owned()));
            state.take_manual_control();
            state.position = 0.1;
            state.pause();
            assert!(!state.cancel_owned_playback(old));
        }
        app.update();
        {
            let state = app.world().resource::<AnimationState>();
            assert!(state.manual_controlled() && state.is_posing() && !state.playing);
            assert!(!state.loading && state.pending.is_none());
            assert!(Arc::ptr_eq(state.clip.as_ref().unwrap(), &clip));
            assert_eq!(state.position, 0.1);
        }

        let (new, tx) = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.resume_automatic_control();
            let active = state.select_owned_one_shot(clip.clone(), Some(path.to_owned()));
            let request = deferred_owned_import(&mut state, path);
            assert!(!state.cancel_owned_playback(old));
            assert!(!state.cancel_owned_playback(active));
            assert!(state.loading && state.pending.is_some());
            request
        };
        tx.send(Ok((path.to_owned(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        assert!(!state.cancel_owned_playback(old));
        assert!(state.playing && state.gesture_active());
        assert!(state.cancel_owned_playback(new));
    }

    #[test]
    fn rejected_load_and_failed_worker_do_not_let_tokens_cancel_other_playback() {
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            let owner = state.select_owned_one_shot(clip.clone(), Some("motion.bvh".into()));
            assert!(state.load_owned_one_shot("unsupported.txt").is_err());
            assert!(state.cancel_owned_playback(owner));
            state.select_clip(clip.clone(), Some("idle.bvh".into()));
        }
        let (token, tx) = deferred_owned_import(
            &mut app.world_mut().resource_mut::<AnimationState>(),
            Path::new("bad.bvh"),
        );
        tx.send(Err("Malformed animation".into())).unwrap();
        app.update();
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        assert_eq!(state.error.as_deref(), Some("Malformed animation"));
        assert!(!state.loading && !state.cancel_owned_playback(token));
        assert!(state.playing && Arc::ptr_eq(state.clip.as_ref().unwrap(), &clip));
    }

    #[test]
    fn bundled_companion_clips_are_bounded_stationary_upper_body_motions() {
        for text in [
            include_str!("../assets/animations/companion/talking-nod.bvh"),
            include_str!("../assets/animations/companion/talking-tilt.bvh"),
            include_str!("../assets/animations/companion/idle-look-down.bvh"),
        ] {
            let clip = parse_bvh(text).unwrap();
            assert_eq!(clip.frame_count, 121);
            assert_eq!(clip.mapped_bones(), 5);
            assert!(clip.duration() > 6.0 && clip.duration() < 6.1);
            for sample in clip.samples.chunks(clip.channel_count) {
                assert!(sample[..6].iter().all(|value| *value == 0.0));
            }
        }
    }

    #[test]
    fn triggered_import_plays_once_and_crossfades_back_to_idle() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            HumanBoneName::Hips,
            Transform::from_xyz(0., 1., 0.),
            ChildOf(root),
        ));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let clip = parse_bvh(&SAMPLE.replace("Frame Time: 0.1", "Frame Time: 0.5")).unwrap();
        let (owner, tx) = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            deferred_owned_import(&mut state, Path::new("emotion.bvh"))
        };
        let receipt = app
            .world()
            .resource::<AnimationState>()
            .owned_playback_receipt(owner)
            .unwrap();
        assert_eq!(receipt.outcome(), PlaybackOutcome::Pending);
        tx.send(Ok(("emotion.bvh".into(), clip))).unwrap();
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            assert!(state.one_shot && state.playing && !state.looping && !state.loading);
            assert_eq!(receipt.outcome(), PlaybackOutcome::Pending);
            assert!(Arc::ptr_eq(
                &receipt.0,
                &state.owned_playback_receipt(owner).unwrap().0
            ));
            assert_eq!(state.transition_seconds, 0.);
            // Even an old/manual loop preference cannot make an emotion repeat.
            state.looping = true;
        }
        for _ in 0..20 {
            app.update();
            if app.world().resource::<AnimationState>().one_shot_finished {
                assert_eq!(receipt.outcome(), PlaybackOutcome::Completed);
                assert!(
                    !app.world().resource::<AnimationState>().gesture_active(),
                    "a finished reaction must release talking/idle priority before fallback"
                );
            }
            if !app.world().resource::<AnimationState>().one_shot {
                break;
            }
        }
        let state = app.world().resource::<AnimationState>();
        assert_eq!(receipt.outcome(), PlaybackOutcome::Completed);
        assert!(state.playing && state.looping && !state.one_shot);
        assert_eq!(
            state.path.as_deref(),
            Some(Path::new("assets/animations/desktop/idle-maid.vrma"))
        );
        let transition = app
            .world()
            .resource::<PoseCache>()
            .transition
            .as_ref()
            .unwrap();
        assert!(transition.elapsed > 0. && transition.elapsed < transition.duration);
        assert!(!transition.outgoing.as_ref().unwrap().playing);
        for _ in 0..10 {
            app.update();
        }
        assert!(app.world().resource::<PoseCache>().transition.is_none());
        assert!(app.world().resource::<AnimationState>().playing);
        assert!(
            !app.world_mut()
                .resource_mut::<AnimationState>()
                .cancel_owned_playback(owner)
        );
        assert!(app.world().resource::<AnimationState>().playing);

        // Completing a gesture must not discard a newer import still in flight.
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
            state.select_one_shot(clip, None);
            state.one_shot_finished = true;
            state.pending = Some(Mutex::new(rx));
            state.loading = true;
            state.import_one_shot = false;
        }
        app.update();
        assert!(app.world().resource::<AnimationState>().pending.is_some());
        tx.send(Ok(("manual.bvh".into(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        let state = app.world().resource::<AnimationState>();
        assert_eq!(state.path.as_deref(), Some(Path::new("manual.bvh")));
        assert!(!state.one_shot && !state.one_shot_finished);
        assert_eq!(receipt.outcome(), PlaybackOutcome::Completed);
    }

    #[test]
    fn stopping_or_manually_replacing_a_gesture_cancels_idle_handoff() {
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        let mut state = AnimationState::default();
        state.select_one_shot(clip.clone(), None);
        state.pause();
        assert!(state.one_shot && !state.one_shot_finished);
        state.stop();
        assert!(!state.one_shot && !state.one_shot_finished && !state.is_posing());
        state.select_one_shot(clip.clone(), None);
        state.select_clip(clip, None);
        assert!(state.looping && state.playing && !state.one_shot);
    }
    #[test]
    fn pack_revocation_rejects_late_imports_and_keeps_explicit_manual_ownership() {
        let revoked = PathBuf::from("pack/motion.bvh");
        let mut state = AnimationState::default();
        let (tx, rx) = mpsc::sync_channel(1);
        state.pending = Some(Mutex::new(rx));
        state.importing_path = Some(revoked.clone());
        state.loading = true;
        state.manual_hold = true;
        assert!(!state.revoke_paths(&["other.bvh".into()]));
        assert!(state.loading);
        assert!(state.revoke_paths(std::slice::from_ref(&revoked)));
        assert!(!state.loading && state.pending.is_none() && !state.is_posing());
        assert!(state.manual_controlled());
        assert!(tx.send(Ok((revoked, parse_bvh(SAMPLE).unwrap()))).is_err());
    }
    #[test]
    fn revoked_current_clip_cannot_be_replayed_or_scrubbed() {
        let mut state = AnimationState::default();
        state.select_clip(
            Arc::new(parse_bvh(SAMPLE).unwrap()),
            Some("pack/old.bvh".into()),
        );
        state.take_manual_control();
        assert!(state.revoke_paths(&["pack/old.bvh".into()]));
        assert!(state.clip.is_none() && state.path.is_none());
        state.play();
        state.show_scrub_pose();
        assert!(!state.is_posing() && !state.playing);
        assert!(state.manual_controlled());
    }

    #[test]
    fn revoked_incoming_clip_preserves_unrelated_visible_playback_and_receipt() {
        let mut state = AnimationState::default();
        state.select_clip(
            Arc::new(parse_bvh(SAMPLE).unwrap()),
            Some("local.bvh".into()),
        );
        let active = state.motion_playback.as_ref().unwrap().clone();
        let (tx, rx) = mpsc::sync_channel(1);
        state.pending = Some(Mutex::new(rx));
        state.importing_path = Some("pack/new.bvh".into());
        state.loading = true;
        state.manual_hold = true;
        assert!(state.revoke_paths(&["pack/new.bvh".into()]));
        assert!(state.playing && state.is_posing() && state.manual_controlled());
        assert_eq!(state.path.as_deref(), Some(Path::new("local.bvh")));
        assert_eq!(
            active.id_for_test(),
            state.motion_playback.as_ref().unwrap().id_for_test()
        );
        assert!(!state.loading && state.pending.is_none());
        assert!(
            tx.send(Ok(("pack/new.bvh".into(), parse_bvh(SAMPLE).unwrap())))
                .is_err()
        );
    }
    #[test]
    fn revoking_previous_pack_does_not_discard_an_unrelated_manual_import() {
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .add_systems(Update, poll_import);
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.select_clip(
                Arc::new(parse_bvh(SAMPLE).unwrap()),
                Some("pack/old.bvh".into()),
            );
            state.manual_hold = true;
            state.pending = Some(Mutex::new(rx));
            state.importing_path = Some("local/new.bvh".into());
            state.loading = true;
            assert!(state.revoke_paths(&["pack/old.bvh".into()]));
            assert!(state.loading && !state.is_posing());
        }
        tx.send(Ok(("local/new.bvh".into(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        let state = app.world().resource::<AnimationState>();
        assert_eq!(state.path.as_deref(), Some(Path::new("local/new.bvh")));
        assert!(state.playing && state.manual_controlled());
    }
    #[test]
    fn manual_pause_cancels_late_automatic_import_and_can_resume_playlists() {
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        let mut state = AnimationState::default();
        state.select_one_shot(clip, Some("reaction.bvh".into()));
        state.pause();
        let (tx, rx) = mpsc::sync_channel(1);
        state.pending = Some(Mutex::new(rx));
        state.loading = true;
        state.import_one_shot = true;
        state.take_manual_control();
        assert!(state.manual_controlled());
        assert!(!state.loading && state.pending.is_none() && !state.gesture_active());
        assert!(!state.playing && state.is_posing());
        assert_eq!(state.path.as_deref(), Some(Path::new("reaction.bvh")));
        assert!(
            tx.send(Ok(("stale.bvh".into(), parse_bvh(SAMPLE).unwrap())))
                .is_err()
        );
        state.resume_automatic_control();
        assert!(!state.manual_controlled() && !state.gesture_active());
        // No paused one-shot gate is left to block the next playlist selection.
        state.select_clip(
            Arc::new(parse_bvh(SAMPLE).unwrap()),
            Some("talking.bvh".into()),
        );
        assert!(state.playing && state.looping);
    }
    #[test]
    fn samples_declared_rotation_channels_separately_from_hips_translation() {
        let clip = parse_bvh(SAMPLE).unwrap();
        assert_eq!(clip.frame_count, 2);
        assert_eq!(clip.mapped_bones(), 2);
        let mut rotations = Vec::new();
        clip.sample_world(0.05, false, &mut rotations);
        assert!(
            rotations[0].abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 0.0001)
        );
        assert!(rotations[1].abs_diff_eq(rotations[0], 0.0001));
    }
    #[test]
    fn rejects_truncation_nonfinite_counts_channels_and_extra_samples() {
        for input in [
            SAMPLE.replace("Frames: 2", "Frames: 0"),
            SAMPLE.replace("0.1", "NaN"),
            SAMPLE.replace("Yrotation", "Xrotation"),
            format!("{SAMPLE} 1"),
            SAMPLE[..SAMPLE.len() - 1].to_owned(),
        ] {
            assert!(parse_bvh(&input).is_err());
        }
        assert!(parse_bvh(&SAMPLE.replace("Frames: 2", "Frames: 100001")).is_err());
        assert!(parse_bvh(&SAMPLE.replace("leftUpperArm", "hips")).is_err());
    }
    #[test]
    fn recognizes_vrm_and_mixamo_names() {
        assert_eq!(
            map_bone("mixamorig:LeftForeArm"),
            Some(HumanBoneName::LeftLowerArm)
        );
        assert_eq!(
            map_bone("mixamorigRightHandThumb2"),
            Some(HumanBoneName::RightThumbProximal)
        );
        assert_eq!(
            map_bone("left_upper_arm"),
            Some(HumanBoneName::LeftUpperArm)
        );
        assert_eq!(map_bone("not-a-humanoid-bone"), None);
    }
    #[test]
    fn poses_only_active_avatar_and_stop_restores_original_transform() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let other = app.world_mut().spawn(Transform::default()).id();
        let original = Transform::from_xyz(0.0, 1.0, 0.0);
        let bone = app
            .world_mut()
            .spawn((HumanBoneName::Hips, original, ChildOf(root)))
            .id();
        let untouched = app
            .world_mut()
            .spawn((HumanBoneName::Hips, original, ChildOf(other)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.clip = Some(Arc::new(parse_bvh(SAMPLE).unwrap()));
            state.play();
        }
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.position = 0.1;
            state.pause();
        }
        app.update();
        let posed = app.world().get::<Transform>(bone).unwrap();
        assert!(!posed.rotation.abs_diff_eq(original.rotation, 0.001));
        assert_eq!(
            posed.translation,
            original.translation + Vec3::new(100., 200., 300.)
        );
        assert_eq!(*app.world().get::<Transform>(untouched).unwrap(), original);
        app.world_mut().resource_mut::<AnimationState>().stop();
        app.update();
        assert_eq!(*app.world().get::<Transform>(bone).unwrap(), original);
    }
    #[test]
    fn parses_local_reference_when_available_without_bundling_it() {
        let path = Path::new(
            "/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4/public/assets/animations/silly-tavern/surprise.bvh",
        );
        if let Ok(input) = std::fs::read_to_string(path) {
            let clip = parse_bvh(&input).unwrap();
            assert!(clip.frame_count > 1);
            assert!(clip.mapped_bones() > 10);
        }
    }
    #[test]
    fn retargeting_respects_nonidentity_bind_rotations_and_avatar_scope_changes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app
            .world_mut()
            .spawn(Transform::from_rotation(Quat::from_rotation_y(2.0)))
            .id();
        let rest = Transform::from_rotation(Quat::from_rotation_x(0.4));
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, rest, ChildOf(root)))
            .id();
        let arm_rest = Transform::from_rotation(Quat::from_rotation_y(0.3));
        let arm = app
            .world_mut()
            .spawn((HumanBoneName::LeftUpperArm, arm_rest, ChildOf(hips)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.clip = Some(Arc::new(parse_bvh(SAMPLE).unwrap()));
            state.play();
        }
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.position = 0.1;
            state.pause();
        }
        app.update();
        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * rest.rotation;
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .abs_diff_eq(expected, 0.0001)
        );
        assert!(
            app.world()
                .get::<Transform>(arm)
                .unwrap()
                .rotation
                .abs_diff_eq(arm_rest.rotation, 0.0001)
        );
        let replacement = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(replacement);
        app.update();
        assert_eq!(*app.world().get::<Transform>(hips).unwrap(), rest);
        assert_eq!(*app.world().get::<Transform>(arm).unwrap(), arm_rest);
    }
    const TWO_ARMS: &str = "HIERARCHY ROOT hips { OFFSET 0 0 0 CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation JOINT leftUpperArm { OFFSET 1 1 0 CHANNELS 3 Zrotation Xrotation Yrotation End Site { OFFSET 1 0 0 } } JOINT rightUpperArm { OFFSET -1 1 0 CHANNELS 3 Zrotation Xrotation Yrotation End Site { OFFSET -1 0 0 } } } MOTION Frames: 2 Frame Time: 0.1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 90 0 0 0 0 0 0 0 0";
    #[test]
    fn procedural_idle_arms_do_not_become_animation_rest_on_inactive_avatars() {
        let mut clips = vec![
            parse_bvh(TWO_ARMS).unwrap(),
            parse_vrma(include_bytes!(
                "../assets/animations/desktop/idle-maid.vrma"
            ))
            .unwrap(),
        ];
        let wave = Path::new(
            "/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4/public/assets/animations/Wave.fbx",
        );
        if let Ok(bytes) = std::fs::read(wave) {
            clips.push(parse_fbx(&bytes).unwrap());
        }
        for clip in clips {
            let clip = Arc::new(clip);
            let render_pose = |relax: bool| {
                let mut app = App::new();
                app.add_plugins(MinimalPlugins)
                    .init_resource::<AvatarLoadState>()
                    .add_plugins(BvhAnimationPlugin);
                let root = app.world_mut().spawn(Transform::default()).id();
                let hips = app
                    .world_mut()
                    .spawn((
                        HumanBoneName::Hips,
                        Transform::from_xyz(0., 0.9371912, 0.),
                        ChildOf(root),
                    ))
                    .id();
                // Neuro's imported upper-arm rotations are identity and its
                // anatomical left is -X (VRM 0.x).
                let left = app
                    .world_mut()
                    .spawn((
                        HumanBoneName::LeftUpperArm,
                        Transform::from_xyz(-0.09265, 0.35, 0.),
                        ChildOf(hips),
                    ))
                    .id();
                let right = app
                    .world_mut()
                    .spawn((
                        HumanBoneName::RightUpperArm,
                        Transform::from_xyz(0.09265, 0.35, 0.),
                        ChildOf(hips),
                    ))
                    .id();
                // The avatar is not active and no clip has been selected yet.
                app.update();
                assert!(app.world().get::<HumanoidRestTransform>(left).is_some());
                if relax {
                    app.world_mut().get_mut::<Transform>(left).unwrap().rotation =
                        Quat::from_rotation_z(1.2);
                    app.world_mut()
                        .get_mut::<Transform>(right)
                        .unwrap()
                        .rotation = Quat::from_rotation_z(-1.2);
                    app.update();
                }
                app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
                app.update();
                {
                    let mut state = app.world_mut().resource_mut::<AnimationState>();
                    state.clip = Some(Arc::clone(&clip));
                    state.play();
                    state.position = 0.5;
                    state.pause();
                }
                app.update();
                [left, right].map(|entity| app.world().get::<Transform>(entity).unwrap().rotation)
            };
            let original = render_pose(false);
            let relaxed = render_pose(true);
            for (original, relaxed) in original.into_iter().zip(relaxed) {
                assert!(
                    original.abs_diff_eq(relaxed, 1e-5),
                    "{}: {original:?} != {relaxed:?}",
                    clip.format()
                );
            }
        }
    }
    #[test]
    fn real_vrma_preserves_omitted_idle_bones_and_restores_idle_on_stop_and_switch() {
        let mut clip = parse_vrma(include_bytes!(
            "../assets/animations/desktop/idle-maid.vrma"
        ))
        .unwrap();
        for joint in &mut clip.joints {
            if joint.bone == Some(HumanBoneName::LeftUpperArm) {
                joint.bone = None;
            }
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let mut arms = Vec::new();
        for (name, x) in [
            (HumanBoneName::LeftUpperArm, -0.1),
            (HumanBoneName::RightUpperArm, 0.1),
        ] {
            arms.push(
                app.world_mut()
                    .spawn((name, Transform::from_xyz(x, 1.3, 0.), ChildOf(root)))
                    .id(),
            );
        }
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        let mut idle = Vec::new();
        for (index, &entity) in arms.iter().enumerate() {
            let mut transform = app.world_mut().get_mut::<Transform>(entity).unwrap();
            transform.rotation = Quat::from_rotation_z(if index == 0 { 1.2 } else { -1.2 });
            idle.push(*transform);
        }
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.clip = Some(Arc::new(clip));
            state.play();
            state.pause();
        }
        app.update();
        assert_eq!(*app.world().get::<Transform>(arms[0]).unwrap(), idle[0]);
        assert!(
            app.world()
                .get::<Transform>(arms[1])
                .unwrap()
                .rotation
                .angle_between(idle[1].rotation)
                > 0.1
        );
        app.world_mut().resource_mut::<AnimationState>().stop();
        app.update();
        for (&entity, idle) in arms.iter().zip(&idle) {
            assert_eq!(*app.world().get::<Transform>(entity).unwrap(), *idle);
        }
        app.world_mut().resource_mut::<AnimationState>().play();
        app.update();
        let replacement = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(replacement);
        app.update();
        for (&entity, idle) in arms.iter().zip(&idle) {
            assert_eq!(*app.world().get::<Transform>(entity).unwrap(), *idle);
        }
    }
    #[test]
    fn detects_source_facing_from_rest_limb_sides() {
        assert_eq!(parse_bvh(TWO_ARMS).unwrap().faces_negative_z(), Some(false));
        let flipped = TWO_ARMS
            .replace("OFFSET 1 1 0", "OFFSET X 1 0")
            .replace("OFFSET -1 1 0", "OFFSET 1 1 0")
            .replace("OFFSET X 1 0", "OFFSET -1 1 0");
        assert_eq!(parse_bvh(&flipped).unwrap().faces_negative_z(), Some(true));
        assert_eq!(parse_bvh(SAMPLE).unwrap().faces_negative_z(), None);
        let reference = Path::new(
            "/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4/public/assets/animations/silly-bvh/joy3.bvh",
        );
        if let Ok(input) = std::fs::read_to_string(reference) {
            assert_eq!(parse_bvh(&input).unwrap().faces_negative_z(), Some(false));
        }
    }
    /// VRM 0.x avatars face -Z in glTF space while motions face +Z; WebWaifu4
    /// converts with `-x, y, -z, w`. A VRM 1.0 avatar keeps the motion unchanged.
    fn posed_hips_for_left_arm_at(left_x: f32) -> Quat {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app
            .world_mut()
            .spawn(Transform::from_rotation(Quat::from_rotation_y(
                std::f32::consts::PI,
            )))
            .id();
        let hips = app
            .world_mut()
            .spawn((
                HumanBoneName::Hips,
                Transform::from_xyz(0.0, 1.0, 0.0),
                ChildOf(root),
            ))
            .id();
        for (bone, x) in [
            (HumanBoneName::LeftUpperArm, left_x),
            (HumanBoneName::RightUpperArm, -left_x),
        ] {
            app.world_mut()
                .spawn((bone, Transform::from_xyz(x, 0.4, 0.0), ChildOf(hips)));
        }
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.clip = Some(Arc::new(parse_bvh(TWO_ARMS).unwrap()));
            state.play();
        }
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.position = 0.1;
            state.pause();
        }
        app.update();
        app.world().get::<Transform>(hips).unwrap().rotation
    }
    #[test]
    fn vrm0_facing_avatars_receive_turned_around_rotations() {
        let quarter = std::f32::consts::FRAC_PI_2;
        let vrm1 = posed_hips_for_left_arm_at(0.2);
        assert!(
            vrm1.abs_diff_eq(Quat::from_rotation_z(quarter), 0.0001),
            "{vrm1:?}"
        );
        let vrm0 = posed_hips_for_left_arm_at(-0.2);
        assert!(
            vrm0.abs_diff_eq(Quat::from_rotation_z(-quarter), 0.0001),
            "{vrm0:?}"
        );
        assert!(
            turn_around(Quat::from_rotation_x(0.3)).abs_diff_eq(Quat::from_rotation_x(-0.3), 1e-6)
        );
        assert!(
            turn_around(Quat::from_rotation_y(0.3)).abs_diff_eq(Quat::from_rotation_y(0.3), 1e-6)
        );
    }
    #[test]
    fn paused_animation_weight_restores_bind_pose_and_reapplies_at_same_position() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let rest = Transform::from_rotation(Quat::from_rotation_x(0.4));
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, rest, ChildOf(root)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.clip = Some(Arc::new(parse_bvh(SAMPLE).unwrap()));
            state.play();
        }
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.position = 0.1;
            state.pause();
            state.weight = 0.0;
        }
        app.update();
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .abs_diff_eq(rest.rotation, 0.0001)
        );
        app.world_mut().resource_mut::<AnimationState>().weight = 0.5;
        app.update();
        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4) * rest.rotation;
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .abs_diff_eq(expected, 0.0001)
        );
    }

    #[test]
    fn normalized_joint_space_preserves_arbitrary_source_and_destination_rests() {
        let src_local = Quat::from_euler(EulerRot::XYZ, 0.3, -0.6, 0.2);
        let src_model = Quat::from_euler(EulerRot::XYZ, -0.4, 0.1, 0.9) * src_local;
        let dst_local = Quat::from_euler(EulerRot::XYZ, 0.7, 0.2, -0.1);
        let dst_model = Quat::from_rotation_y(-0.8) * dst_local;
        let normalized = Quat::from_rotation_z(0.65);
        let raw_source = src_local * src_model.inverse() * normalized * src_model;
        let actual = destination_pose(
            dst_local,
            dst_model,
            normalized_pose(src_local, src_model, raw_source),
        );
        let expected = dst_local * dst_model.inverse() * normalized * dst_model;
        assert!(actual.angle_between(expected) < 0.001);
        assert!(
            destination_pose(
                dst_local,
                dst_model,
                normalized_pose(src_local, src_model, src_local)
            )
            .angle_between(dst_local)
                < 0.001
        );
    }

    #[test]
    fn bundled_sitting_hips_use_local_rest_delta_not_absolute_source_height() {
        let clip = parse_vrma(include_bytes!(
            "../assets/animations/desktop/idle-sitting.vrma"
        ))
        .unwrap();
        let index = clip
            .joints
            .iter()
            .position(|joint| joint.bone == Some(HumanBoneName::Hips))
            .unwrap();
        let mut sampled = Vec::new();
        clip.sample_local(0., true, &mut sampled);
        let rest = Transform::from_xyz(0., 1.2, 0.).with_rotation(Quat::from_rotation_y(0.4));
        let bone = PoseBone {
            entity: Entity::PLACEHOLDER,
            bone: HumanBoneName::Hips,
            rest,
            idle: rest,
            rest_rotation: rest.rotation,
            rest_position: rest.translation,
            model_to_parent: Some(Affine3A::IDENTITY),
            joint: Some(index),
            depth: 0,
        };
        let pose = clip_pose(&clip, index, sampled[index], 0., true, &bone, false, 1.);
        assert!((clip.joints[index].rest_translation.y - 0.8241627).abs() < 1e-6);
        assert!((pose.translation.y - 1.2).abs() < 1e-6);
        let delta_z = 0.019633185 - 0.0036096585;
        assert!((pose.translation.z - delta_z * (1.2 / 0.8241627)).abs() < 1e-6);
        assert!(pose.rotation.angle_between(rest.rotation) < 0.001);
    }

    #[test]
    fn hips_delta_converts_both_armature_frames_and_rejects_singular_target() {
        let source = "HIERARCHY ROOT Armature { OFFSET 0 0 0 CHANNELS 3 Xrotation Yrotation Zrotation JOINT hips { OFFSET 0 2 0 CHANNELS 6 Xposition Yposition Zposition Xrotation Yrotation Zrotation End Site { OFFSET 0 -1 0 } } } MOTION Frames: 2 Frame Time: 0.1 0 0 0 0 0 0 0 0 0 0 0 0 1 2 3 0 0 0";
        let mut clip = parse_bvh(source).unwrap();
        clip.joints[0].rest_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        clip.joints[0].rest_scale = Vec3::new(2., 0.5, 3.);
        clip.populate_rest_geometry().unwrap();
        let parent = Transform::from_xyz(3., 0.5, -4.)
            .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.3, -0.7, 0.2))
            .with_scale(Vec3::new(0.5, 2., 3.))
            .compute_affine();
        let rest_position = Vec3::new(0., 2., 0.);
        let rest = Transform::from_translation(parent.inverse().transform_point3(rest_position));
        let mut bone = PoseBone {
            entity: Entity::PLACEHOLDER,
            bone: HumanBoneName::Hips,
            rest,
            idle: rest,
            rest_rotation: Quat::IDENTITY,
            rest_position,
            model_to_parent: Some(parent.inverse()),
            joint: Some(1),
            depth: 2,
        };
        for reverse in [false, true] {
            for weight in [0., 0.4, 1.] {
                let pose = clip_pose(&clip, 1, Quat::IDENTITY, 0.1, false, &bone, reverse, weight);
                // Source (1,2,3) -> scaled (2,1,9) -> +90Y (9,1,-2).
                // Target/source model-space hips heights are 2/1.
                let sign = if reverse { -1. } else { 1. };
                let expected = rest_position + Vec3::new(sign * 18., 2., sign * -4.) * weight;
                assert!(
                    parent
                        .transform_point3(pose.translation)
                        .abs_diff_eq(expected, 1e-4)
                );
                assert_eq!(pose.scale, rest.scale);
            }
        }
        bone.model_to_parent = None;
        assert_eq!(
            clip_pose(&clip, 1, Quat::IDENTITY, 0.1, false, &bone, false, 1.).translation,
            rest.translation
        );
    }

    #[test]
    fn swapping_animation_resources_with_equal_revisions_rebuilds_joint_mapping() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let first = Arc::new(parse_bvh(SAMPLE).unwrap());
        let second = Arc::new(parse_bvh("HIERARCHY ROOT spine { OFFSET 0 0 0 CHANNELS 3 Xrotation Yrotation Zrotation JOINT hips { OFFSET 0 1 0 CHANNELS 3 Xrotation Yrotation Zrotation End Site { OFFSET 0 -1 0 } } } MOTION Frames: 2 Frame Time: 0.1 0 0 0 0 0 0 0 0 0 90 0 0").unwrap());
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.select_clip(first, None);
            state.transition_seconds = 0.;
            state.pause();
        }
        app.update();
        app.world_mut().resource_mut::<AnimationState>().position = 0.1;
        app.update();
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .angle_between(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                < 0.001
        );
        let original = app.world_mut().remove_resource::<AnimationState>().unwrap();
        let mut replacement = AnimationState::default();
        replacement.select_clip(second, None);
        replacement.transition_seconds = 0.;
        replacement.position = 0.1;
        replacement.pause();
        assert_eq!(original.revision, replacement.revision);
        app.insert_resource(replacement);
        app.update();
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .angle_between(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
                < 0.001
        );
        app.insert_resource(original);
        app.update();
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .angle_between(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                < 0.001
        );
    }

    #[test]
    fn hips_height_scaling_and_facing_ignore_application_placement_rotation_and_scale() {
        let source = SAMPLE
            .replacen("OFFSET 0 0 0", "OFFSET 0 100 0", 1)
            .replace("100 200 300 90", "10 20 30 90");
        let clip = Arc::new(parse_bvh(&source).unwrap());
        for reverse in [false, true] {
            for placed in [false, true] {
                for armature in [false, true] {
                    let mut app = App::new();
                    app.add_plugins(MinimalPlugins)
                        .init_resource::<AvatarLoadState>()
                        .add_plugins(BvhAnimationPlugin);
                    let placement = if placed {
                        Transform::from_xyz(80., -40., 60.)
                            .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.4, -1.8, 0.2))
                            .with_scale(Vec3::splat(3.))
                    } else {
                        Transform::default()
                    };
                    let root = app.world_mut().spawn(placement).id();
                    let internal_transform = if armature {
                        Transform::from_xyz(0., 0.5, 0.)
                            .with_rotation(Quat::from_rotation_y(0.7))
                            .with_scale(Vec3::new(2., 0.5, 3.))
                    } else {
                        Transform::from_xyz(0., 0.5, 0.)
                    };
                    let internal = app
                        .world_mut()
                        .spawn((internal_transform, ChildOf(root)))
                        .id();
                    let rest = Transform::from_translation(
                        internal_transform
                            .compute_affine()
                            .inverse()
                            .transform_point3(Vec3::new(0., 2., 0.)),
                    );
                    let hips = app
                        .world_mut()
                        .spawn((HumanBoneName::Hips, rest, ChildOf(internal)))
                        .id();
                    let side = if reverse { -0.2 } else { 0.2 };
                    for (name, x) in [
                        (HumanBoneName::LeftUpperArm, side),
                        (HumanBoneName::RightUpperArm, -side),
                    ] {
                        app.world_mut().spawn((
                            name,
                            Transform::from_xyz(x, 0.3, 0.),
                            ChildOf(hips),
                        ));
                    }
                    app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
                    {
                        let mut state = app.world_mut().resource_mut::<AnimationState>();
                        state.clip = Some(clip.clone());
                        state.play();
                    }
                    app.update();
                    {
                        let mut state = app.world_mut().resource_mut::<AnimationState>();
                        state.position = 0.1;
                        state.pause();
                    }
                    app.update();
                    let pose = app.world().get::<Transform>(hips).unwrap();
                    let sign = if reverse { -1. } else { 1. };
                    assert!(
                        internal_transform
                            .transform_point(pose.translation)
                            .abs_diff_eq(Vec3::new(sign * 0.2, 2.4, sign * 0.6), 1e-5),
                        "{pose:?}"
                    );
                    assert!(
                        pose.rotation.angle_between(destination_pose(
                            Quat::IDENTITY,
                            internal_transform.rotation,
                            Quat::from_rotation_z(sign * std::f32::consts::FRAC_PI_2)
                        )) < 0.001
                    );
                    app.world_mut().resource_mut::<AnimationState>().stop();
                    app.update();
                    assert_eq!(*app.world().get::<Transform>(hips).unwrap(), rest);
                    assert_eq!(*app.world().get::<Transform>(root).unwrap(), placement);
                }
            }
        }
    }

    #[test]
    fn desktop_selection_crossfades_live_outgoing_clip_and_stop_restores_rest() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let rest = Transform::from_xyz(0., 1., 0.);
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, rest, ChildOf(root)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let slow = SAMPLE
            .replace("Frame Time: 0.1", "Frame Time: 1.0")
            .replace("100 200 300 90", "0 0 0 90");
        let outgoing = Arc::new(parse_bvh(&slow).unwrap());
        let incoming = Arc::new(parse_bvh(&slow.replace("0 0 0 90", "0 0 0 -90")).unwrap());
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.clip = Some(outgoing);
            state.path = Some("local-a.bvh".into());
            state.play();
        }
        app.update();
        app.update();
        let previous = app.world().get::<Transform>(hips).unwrap().rotation;
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(incoming, Some("local-b.bvh".into()));
        app.update();
        let actual = app.world().get::<Transform>(hips).unwrap().rotation;
        // Outgoing advances from 9 to 18 degrees; incoming is -9 degrees.
        // At 20% fade the result is +12.6 degrees, not the frozen +5.4.
        assert!(
            actual.angle_between(Quat::from_rotation_z(12.6_f32.to_radians())) < 0.002,
            "{previous:?} -> {actual:?}"
        );
        assert!(
            !app.world_mut()
                .resource_mut::<AnimationState>()
                .revoke_paths(&["pack/other.bvh".into()])
        );
        app.update();
        assert!(app.world().resource::<PoseCache>().transition.is_some());
        let mixed = app.world().get::<Transform>(hips).unwrap().rotation;
        assert!(mixed.angle_between(Quat::from_rotation_z(-18_f32.to_radians())) > 0.002);
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().resource::<PoseCache>().transition.is_none());
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .angle_between(Quat::from_rotation_z(-54_f32.to_radians()))
                < 0.002
        );
        app.world_mut().resource_mut::<AnimationState>().stop();
        app.update();
        assert_eq!(*app.world().get::<Transform>(hips).unwrap(), rest);
    }
    #[test]
    fn revocation_clears_an_outgoing_clip_already_owned_by_pose_cache() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let slow = SAMPLE
            .replace("Frame Time: 0.1", "Frame Time: 1.0")
            .replace("100 200 300 90", "0 0 0 90");
        let outgoing = Arc::new(parse_bvh(&slow).unwrap());
        let incoming = Arc::new(parse_bvh(&slow.replace("0 0 0 90", "0 0 0 -90")).unwrap());
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(outgoing, Some("pack/old.bvh".into()));
        app.update();
        app.update();
        for _ in 0..5 {
            app.update();
        }
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(incoming.clone(), Some("local.bvh".into()));
        app.update();
        assert!(
            app.world()
                .resource::<PoseCache>()
                .transition
                .as_ref()
                .unwrap()
                .outgoing
                .is_some()
        );
        // Interrupting a fade captures a mixed pose; retain its full provenance.
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(incoming, Some("local.bvh".into()));
        app.update();
        assert!(
            app.world()
                .resource::<PoseCache>()
                .transition
                .as_ref()
                .unwrap()
                .outgoing
                .is_none()
        );
        assert!(
            !app.world_mut()
                .resource_mut::<AnimationState>()
                .revoke_paths(&["pack/old.bvh".into()])
        );
        app.update();
        assert!(app.world().resource::<PoseCache>().transition.is_none());
        assert!(app.world().resource::<AnimationState>().playing);
        let pose = app.world().get::<Transform>(hips).unwrap();
        assert!(
            pose.rotation
                .angle_between(Quat::from_rotation_z(-18_f32.to_radians()))
                < 0.002
        );
        assert!(
            app.world_mut()
                .resource_mut::<AnimationState>()
                .revoke_paths(&["local.bvh".into()])
        );
        app.update();
        assert_eq!(
            *app.world().get::<Transform>(hips).unwrap(),
            Transform::default()
        );
    }

    #[test]
    fn parked_motion_and_pending_import_cannot_escape_pack_revocation() {
        for revoked_import in [false, true] {
            let mut state = AnimationState::default();
            state.select_clip(
                Arc::new(parse_bvh(SAMPLE).unwrap()),
                Some("pack/old.bvh".into()),
            );
            state.take_manual_control();
            let (tx, rx) = mpsc::sync_channel(1);
            if revoked_import {
                state.pending = Some(Mutex::new(rx));
                state.importing_path = Some("pack/new.bvh".into());
                state.loading = true;
            }
            let parked = state.park_for_desktop();
            state.select_clip(
                Arc::new(parse_bvh(SAMPLE).unwrap()),
                Some("desktop.bvh".into()),
            );
            assert!(!state.revoke_paths(&["pack/old.bvh".into(), "pack/new.bvh".into()]));
            state.restore_parked(parked);
            assert!(state.manual_controlled());
            assert!(state.clip.is_none() && state.path.is_none() && !state.loading);
            state.play();
            assert!(!state.is_posing());
            if revoked_import {
                assert!(
                    tx.send(Ok(("pack/new.bvh".into(), parse_bvh(SAMPLE).unwrap())))
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn parking_and_restoring_preserve_pending_revocations_for_the_pose_cache() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip.clone(), Some("pack/old.bvh".into()));
        app.update();
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip.clone(), Some("local.bvh".into()));
        app.update();
        assert!(app.world().resource::<PoseCache>().transition.is_some());
        let previous = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            assert!(!state.revoke_paths(&["pack/old.bvh".into()]));
            let previous = state.park_for_desktop();
            state.select_clip(clip, Some("desktop.bvh".into()));
            previous
        };
        app.update();
        assert!(
            app.world().resource::<PoseCache>().transition.is_none(),
            "queued invalidation must follow the live pose cache across parking"
        );
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        state.revoke_paths(&["pack/during-restore.bvh".into()]);
        state.restore_parked(previous);
        assert!(
            state
                .revoked_paths
                .contains(Path::new("pack/during-restore.bvh")),
            "restore must merge the live frame's pending invalidations"
        );
    }

    #[test]
    fn revoking_old_ancestry_preserves_a_clean_same_frame_pending_crossfade() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip.clone(), Some("pack/old.bvh".into()));
        app.update();
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip.clone(), Some("local-a.bvh".into()));
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.revoke_paths(&["pack/old.bvh".into()]);
            state.select_clip(clip, Some("local-b.bvh".into()));
        }
        app.update();
        let transition = app
            .world()
            .resource::<PoseCache>()
            .transition
            .as_ref()
            .expect("unrelated new transition must survive");
        assert_eq!(
            transition.outgoing.as_ref().unwrap().path.as_deref(),
            Some(Path::new("local-a.bvh"))
        );
        assert!(!transition.paths.contains(Path::new("pack/old.bvh")));
        assert!(app.world().resource::<AnimationState>().playing);
    }

    #[test]
    fn revoked_current_pose_does_not_pollute_later_local_transitions() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip.clone(), Some("pack/old.bvh".into()));
        for _ in 0..7 {
            app.update();
        }
        assert!(
            app.world_mut()
                .resource_mut::<AnimationState>()
                .revoke_paths(&["pack/old.bvh".into()])
        );
        app.update();
        assert_eq!(
            *app.world().get::<Transform>(hips).unwrap(),
            Transform::default()
        );
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip, Some("local.bvh".into()));
        app.update();
        let transition = app
            .world()
            .resource::<PoseCache>()
            .transition
            .as_ref()
            .expect("new local clip must fade in from idle");
        assert!(
            !transition.paths.contains(Path::new("pack/old.bvh")),
            "a restored idle pose must not inherit a revoked clip's attribution"
        );
        assert!(
            !app.world_mut()
                .resource_mut::<AnimationState>()
                .revoke_paths(&["pack/old.bvh".into()])
        );
        app.update();
        assert!(
            app.world().resource::<PoseCache>().transition.is_some(),
            "re-revoking the pack must preserve the unrelated local fade"
        );
    }
    #[test]
    fn stopped_pose_does_not_pollute_later_local_transitions() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        let hips = app
            .world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let clip = Arc::new(parse_bvh(SAMPLE).unwrap());
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip.clone(), Some("pack/old.bvh".into()));
        for _ in 0..7 {
            app.update();
        }
        app.world_mut().resource_mut::<AnimationState>().stop();
        app.update();
        assert_eq!(
            *app.world().get::<Transform>(hips).unwrap(),
            Transform::default()
        );
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(clip, Some("local.bvh".into()));
        app.update();
        let transition = app
            .world()
            .resource::<PoseCache>()
            .transition
            .as_ref()
            .expect("new local clip must fade in from idle");
        assert!(
            !transition.paths.contains(Path::new("pack/old.bvh")),
            "a stopped pose must not leave stale ancestry in a new local fade"
        );
        assert!(
            !app.world_mut()
                .resource_mut::<AnimationState>()
                .revoke_paths(&["pack/old.bvh".into()])
        );
        app.update();
        assert!(
            app.world().resource::<PoseCache>().transition.is_some(),
            "re-revoking the pack must preserve the unrelated local fade"
        );
    }
}
