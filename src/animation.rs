//! Bounded native BVH, VRMA and FBX import with humanoid pose retargeting.
//! Implements the BVH format independently; no third-party animation code is copied.
//! Rotation normalization and hips-height scaling follow VRMC_vrm_animation's
//! pose compatibility guidance, as used by bevy_vrm1 (MIT OR Apache-2.0):
//! https://github.com/not-elm/bevy_vrm1/tree/v0.7.1/src/vrma/animation
//! Application placement is excluded from all rest-space calculations.
use crate::loading::AvatarLoadState;
use bevy::{math::Affine3A, prelude::*};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[path = "animation_import.rs"]
mod animation_import;
pub use animation_import::{parse_fbx, parse_vrma};

const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_SAMPLES: usize = 4_000_000;
static IMPORT_BUSY: AtomicBool = AtomicBool::new(false);

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
    pub fn format(&self) -> &'static str {
        self.source_format
    }
    pub fn mapped_bones(&self) -> usize {
        self.joints.iter().filter(|j| j.bone.is_some()).count()
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
#[derive(Clone)]
struct ClipPlayback {
    clip: Arc<BvhClip>,
    position: f32,
    looping: bool,
    speed: f32,
    weight: f32,
    playing: bool,
}
#[derive(Resource)]
pub struct AnimationState {
    pub clip: Option<Arc<BvhClip>>,
    pub path: Option<PathBuf>,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    /// Animation influence; zero restores the captured rest pose.
    pub weight: f32,
    pub position: f32,
    pub loading: bool,
    pub error: Option<String>,
    pending: Option<Mutex<mpsc::Receiver<ImportResult>>>,
    stopped: bool,
    revision: u64,
    autoplay_import: bool,
    transition_seconds: f32,
    transition_from: Option<ClipPlayback>,
    import_one_shot: bool,
    one_shot: bool,
    one_shot_finished: bool,
}
impl Default for AnimationState {
    fn default() -> Self {
        Self {
            clip: None,
            path: None,
            playing: false,
            looping: true,
            speed: 1.0,
            weight: 1.0,
            position: 0.0,
            loading: false,
            error: None,
            pending: None,
            stopped: true,
            revision: 0,
            autoplay_import: false,
            transition_seconds: 0.,
            transition_from: None,
            import_one_shot: false,
            one_shot: false,
            one_shot_finished: false,
        }
    }
}
impl AnimationState {
    pub fn load_path(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
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
            return Err("An animation import is already running".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
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
            return Err(format!("Cannot start animation import: {error}"));
        }
        self.pending = Some(Mutex::new(rx));
        self.loading = true;
        self.autoplay_import = true;
        self.import_one_shot = false;
        self.error = None;
        Ok(())
    }
    /// Reply and emotion gestures play once, then crossfade to the bundled idle.
    pub fn load_one_shot(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        self.load_path(path)?;
        self.import_one_shot = true;
        Ok(())
    }
    fn select_one_shot(&mut self, clip: Arc<BvhClip>, path: Option<PathBuf>) {
        self.select_clip(clip, path);
        self.looping = false;
        self.one_shot = true;
    }
    pub fn play(&mut self) {
        if self.clip.is_some() {
            self.playing = true;
            self.stopped = false;
        }
    }
    /// Switch a preloaded motion without file I/O or an asynchronous import.
    /// Starts at time zero with a 0.5-second crossfade from the live prior clip.
    pub fn select_clip(&mut self, clip: Arc<BvhClip>, path: Option<PathBuf>) {
        self.transition_from = if self.is_posing() {
            self.clip.clone().map(|clip| ClipPlayback {
                clip,
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
        self.play();
    }
    pub fn pause(&mut self) {
        self.playing = false;
    }
    pub fn stop(&mut self) {
        self.playing = false;
        self.stopped = true;
        self.autoplay_import = false;
        self.one_shot = false;
        self.one_shot_finished = false;
        self.position = 0.0;
        self.transition_from = None;
        self.transition_seconds = 0.;
    }
    pub fn is_posing(&self) -> bool {
        self.clip.is_some() && (self.playing || !self.stopped)
    }
}

pub struct BvhAnimationPlugin;
impl Plugin for BvhAnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimationState>()
            .init_resource::<PoseCache>()
            .add_systems(
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
            if state.import_one_shot && state.autoplay_import {
                state.select_one_shot(Arc::new(clip), Some(path));
                return;
            }
            // Manual file imports preserve their immediate switch semantics.
            state.transition_seconds = 0.;
            state.transition_from = None;
            state.path = Some(path);
            state.clip = Some(Arc::new(clip));
            state.position = 0.0;
            state.loading = false;
            state.pending = None;
            state.error = None;
            state.revision = state.revision.wrapping_add(1);
            state.one_shot = false;
            state.one_shot_finished = false;
            if state.autoplay_import {
                state.play();
            } else {
                state.stop();
            }
        }
        Some(Ok(Err(error))) => {
            state.error = Some(error);
            state.loading = false;
            state.pending = None;
        }
        Some(Err(mpsc::TryRecvError::Disconnected)) => {
            state.error = Some("Animation import worker exited unexpectedly".into());
            state.loading = false;
            state.pending = None;
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
    bones: Vec<PoseBone>,
    revision: u64,
    applied: bool,
    sampled: Vec<Quat>,
    last_position: f32,
    last_weight: f32,
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
) {
    if cache.root != avatar.active {
        for bone in &cache.bones {
            if let Ok(mut transform) = transforms.get_mut(bone.entity) {
                *transform = bone.idle;
            }
        }
        cache.bones.clear();
        cache.sampled.clear();
        cache.root = avatar.active;
        cache.clip = None;
        cache.applied = false;
        cache.transition = None;
        state.position = 0.0;
    }
    let Some(root) = avatar.active else { return };
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
        let outgoing = if cache.transition.is_some() {
            state.transition_from = None;
            None
        } else {
            state.transition_from.take()
        };
        cache.transition = if state.transition_seconds > 0. && state.transition_seconds.is_finite()
        {
            Some(PoseTransition {
                from,
                outgoing,
                elapsed: 0.,
                duration: state.transition_seconds.clamp(0.001, 5.),
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
                state.position = clip
                    .exact_duration
                    .unwrap_or((clip.frame_count - 1) as f32 * clip.frame_time);
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
    cache.applied = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = "HIERARCHY ROOT hips { OFFSET 0 0 0 CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation JOINT leftUpperArm { OFFSET 1 1 0 CHANNELS 3 Zrotation Xrotation Yrotation End Site { OFFSET 1 0 0 } } } MOTION Frames: 2 Frame Time: 0.1 0 0 0 0 0 0 0 0 0 100 200 300 90 0 0 0 0 0";
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
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(Ok(("emotion.bvh".into(), clip))).unwrap();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            state.pending = Some(Mutex::new(rx));
            state.loading = true;
            state.autoplay_import = true;
            state.import_one_shot = true;
        }
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            assert!(state.one_shot && state.playing && !state.looping && !state.loading);
            assert_eq!(state.transition_seconds, 0.);
            // Even an old/manual loop preference cannot make an emotion repeat.
            state.looping = true;
        }
        for _ in 0..20 {
            app.update();
            if !app.world().resource::<AnimationState>().one_shot {
                break;
            }
        }
        let state = app.world().resource::<AnimationState>();
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
            state.play();
        }
        app.update();
        app.update();
        let previous = app.world().get::<Transform>(hips).unwrap().rotation;
        app.world_mut()
            .resource_mut::<AnimationState>()
            .select_clip(incoming, None);
        app.update();
        let actual = app.world().get::<Transform>(hips).unwrap().rotation;
        // Outgoing advances from 9 to 18 degrees; incoming is -9 degrees.
        // At 20% fade the result is +12.6 degrees, not the frozen +5.4.
        assert!(
            actual.angle_between(Quat::from_rotation_z(12.6_f32.to_radians())) < 0.002,
            "{previous:?} -> {actual:?}"
        );
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().resource::<PoseCache>().transition.is_none());
        assert!(
            app.world()
                .get::<Transform>(hips)
                .unwrap()
                .rotation
                .angle_between(Quat::from_rotation_z(-45_f32.to_radians()))
                < 0.002
        );
        app.world_mut().resource_mut::<AnimationState>().stop();
        app.update();
        assert_eq!(*app.world().get::<Transform>(hips).unwrap(), rest);
    }
}
