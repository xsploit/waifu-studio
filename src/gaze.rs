//! Optional procedural head/neck/eye overlays for the selected avatar.
//!
//! Follows WebWaifu4's gaze behavior: occasional target changes, slow head sway,
//! separate eye micro-motion and counter-rotation, and conditional overlay removal.
//! Raw VRM joints require a rest-space conversion that normalized three-vrm bones
//! provide upstream. Anatomical axes are derived from immutable imported rest
//! transforms, including the VRM 0.x facing convention and intermediate parents.
//! Optional pointer targeting is local to the rendered viewport; morph-only
//! look expressions are not implemented. A compositor bridge may supply a fresh
//! window-relative pointer when native cursor events are unavailable.
use crate::{animation::HumanoidRestTransform, loading::AvatarLoadState};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;

/// Optional compositor cursor in primary-window logical coordinates.
/// Producers clear it outside desktop mode; readers also reject old samples.
#[derive(Resource, Default)]
pub struct GazePointer {
    sample: Option<(Vec2, std::time::Instant)>,
}
impl GazePointer {
    pub fn update(&mut self, cursor: Option<Vec2>, sampled_at: std::time::Instant) {
        self.sample = cursor
            .filter(|cursor| cursor.is_finite())
            .map(|cursor| (cursor, sampled_at));
    }

    fn fresh(&self, now: std::time::Instant) -> Option<Vec2> {
        let (cursor, at) = self.sample?;
        (now.checked_duration_since(at)? <= std::time::Duration::from_millis(500)).then_some(cursor)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GazeTarget {
    #[default]
    Ambient,
    Pointer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GazeConfig {
    pub target: GazeTarget,
    pub enabled: bool,
    pub intensity: f32,
    pub head_follow: f32,
    pub head_drift: f32,
    pub eye_motion: f32,
    pub smoothing_seconds: f32,
}
impl Default for GazeConfig {
    fn default() -> Self {
        Self {
            target: GazeTarget::Ambient,
            enabled: false,
            intensity: 0.45,
            head_follow: 0.55,
            head_drift: 0.35,
            eye_motion: 0.35,
            smoothing_seconds: 1. / 2.6,
        }
    }
}
impl GazeConfig {
    pub fn sanitized(&self) -> Self {
        let defaults = Self::default();
        let bounded = |value: f32, min, max, fallback| {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                fallback
            }
        };
        Self {
            target: self.target,
            enabled: self.enabled,
            intensity: bounded(self.intensity, 0., 1., defaults.intensity),
            head_follow: bounded(self.head_follow, 0., 1., defaults.head_follow),
            head_drift: bounded(self.head_drift, 0., 1., defaults.head_drift),
            eye_motion: bounded(self.eye_motion, 0., 1., defaults.eye_motion),
            smoothing_seconds: bounded(
                self.smoothing_seconds,
                0.05,
                2.5,
                defaults.smoothing_seconds,
            ),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self == &self.sanitized() {
            Ok(())
        } else {
            Err("Gaze controls must be finite: strengths 0–1, smoothing 0.05–2.5 seconds".into())
        }
    }
}

#[derive(Resource, Default)]
pub struct GazeState {
    pub config: GazeConfig,
    pub tracked_bones: usize,
    pub status: String,
}

#[derive(SystemSet, Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum GazeSystems {
    Cleanup,
    Apply,
}

pub struct GazePlugin;
impl Plugin for GazePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GazeState>()
            .init_resource::<GazeCache>()
            .init_resource::<GazePointer>()
            .add_systems(PreUpdate, clear_overlay.in_set(GazeSystems::Cleanup))
            .add_systems(
                PostUpdate,
                apply_gaze
                    .in_set(GazeSystems::Apply)
                    .after(bevy::app::AnimationSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

#[derive(Clone, Copy, Debug)]
enum Part {
    Head,
    Neck,
    Eye,
}

#[derive(Clone, Copy)]
struct Applied {
    base: Quat,
    result: Quat,
}
impl Applied {
    fn remove(self, current: &mut Quat) {
        // Restore only our rotation. Keep translations/scales and any newer
        // animation, physics or manual rotation written by another controller.
        if current.abs_diff_eq(self.result, 1e-6) || current.abs_diff_eq(-self.result, 1e-6) {
            *current = self.base;
        }
    }
}

struct GazeBone {
    entity: Entity,
    part: Part,
    /// Model-space rest rotation, excluding the application placement root.
    model_rest: Quat,
    applied: Option<Applied>,
}

#[derive(Resource, Default)]
struct GazeCache {
    root: Option<Entity>,
    basis: Quat,
    bones: Vec<GazeBone>,
    motion: Motion,
}

struct Motion {
    phase: f32,
    remaining: f32,
    target: Vec2,
    current: Vec2,
    random: u32,
}
impl Default for Motion {
    fn default() -> Self {
        Self {
            phase: 0.,
            remaining: 0.2,
            target: Vec2::ZERO,
            current: Vec2::ZERO,
            random: 0x6d2b79f5,
        }
    }
}
impl Motion {
    fn random_unit(&mut self) -> f32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        (self.random >> 8) as f32 / 16_777_215.
    }
    fn sample(&mut self, config: &GazeConfig, dt: f32, pointer: Option<Vec2>) -> [Quat; 3] {
        let dt = if dt.is_finite() {
            dt.clamp(0., 0.1)
        } else {
            0.
        };
        self.phase = (self.phase + dt) % 1000.;
        self.remaining -= dt;
        let drift = config.intensity * config.head_drift;
        let eyes = config.intensity * config.eye_motion;
        if self.remaining <= 0. {
            self.remaining = 2.4 + self.random_unit() * 3.4;
            self.target = Vec2::new(self.random_unit() * 2. - 1., self.random_unit() * 2. - 1.);
        }
        let pointer = pointer.filter(|point| point.is_finite());
        let target = pointer
            .unwrap_or(self.target)
            .clamp(Vec2::NEG_ONE, Vec2::ONE);
        self.current = self
            .current
            .lerp(target, 1. - (-dt / config.smoothing_seconds).exp());
        let t = self.phase;
        let x = self.current.x * 0.24 * drift
            + ((t * 0.42).sin() + (t * 0.17 + 1.4).sin() * 0.45) * 0.16 * drift;
        let y = self.current.y * 0.1 * drift
            + ((t * 0.31 + 0.9).sin() + (t * 0.13 + 2.1).sin() * 0.35) * 0.06 * drift;
        let follow = drift * (0.55 + config.head_follow * 0.45);
        let yaw = if pointer.is_some() {
            self.current.x * 0.35 * config.intensity * config.head_follow
        } else {
            (x / 0.24).clamp(-1., 1.) * 0.28 * follow
        };
        let pitch = if pointer.is_some() {
            -self.current.y * 0.2 * config.intensity * config.head_follow
        } else {
            (0.035 - (y / 0.1).clamp(-1., 1.) * 0.08) * follow
        };
        let eye_x = ((t * 0.93).sin() + (t * 2.17 + 1.7).sin() * 0.35) * 0.018 * eyes;
        let eye_y = (t * 1.21 + 0.8).sin() * 0.009 * eyes;
        let eye_yaw = if pointer.is_some() {
            (self.current.x * 0.3 - yaw * 0.5 + eye_x).clamp(-0.22, 0.22) * eyes
        } else {
            (eye_x * 4.8 - yaw * 0.8).clamp(-0.22, 0.22) * eyes
        };
        let eye_pitch = if pointer.is_some() {
            (-self.current.y * 0.2 - pitch * 0.5 + eye_y).clamp(-0.15, 0.15) * eyes
        } else {
            (eye_y * 3.2 - pitch * 0.55).clamp(-0.15, 0.15) * eyes
        };
        // YXZ arguments are yaw, pitch, roll (THREE.Euler stores pitch first).
        [
            Quat::from_euler(EulerRot::YXZ, yaw, pitch, -yaw * 0.1),
            Quat::from_euler(EulerRot::YXZ, yaw * 0.35, pitch * 0.3, 0.),
            Quat::from_euler(EulerRot::YXZ, eye_yaw, eye_pitch, 0.),
        ]
    }
}

fn pointer_target(cursor: Vec2, viewport: Rect) -> Option<Vec2> {
    let size = viewport.size();
    if !cursor.is_finite()
        || !viewport.min.is_finite()
        || !viewport.max.is_finite()
        || size.min_element() <= 0.
        || !viewport.contains(cursor)
    {
        return None;
    }
    let unit = (cursor - viewport.min) / size;
    Some(Vec2::new(unit.x * 2. - 1., 1. - unit.y * 2.))
}

fn clear_overlay(mut cache: ResMut<GazeCache>, mut transforms: Query<&mut Transform>) {
    for bone in &mut cache.bones {
        if let Some(applied) = bone.applied.take()
            && let Ok(mut transform) = transforms.get_mut(bone.entity)
        {
            applied.remove(&mut transform.rotation);
        }
    }
}

fn valid_rotation(rotation: Quat) -> bool {
    rotation.is_finite() && (rotation.length_squared() - 1.).abs() < 0.01
}

/// Traverse every imported parent, using immutable humanoid rest snapshots and
/// retaining non-humanoid import corrections. Never includes world placement.
fn model_rest(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
    rests: &Query<&HumanoidRestTransform>,
) -> Option<(Quat, Vec3)> {
    let mut current = entity;
    let mut rotation = Quat::IDENTITY;
    let mut position = Vec3::ZERO;
    for _ in 0..128 {
        if current == root {
            return Some((rotation.normalize(), position));
        }
        let transform = rests
            .get(current)
            .map(|rest| rest.0)
            .or_else(|_| transforms.get(current).copied())
            .ok()?;
        if !valid_rotation(transform.rotation)
            || !transform.translation.is_finite()
            || !transform.scale.is_finite()
            || transform.scale.min_element() <= 1e-6
        {
            return None;
        }
        position = transform.transform_point(position);
        rotation = transform.rotation * rotation;
        current = parents.get(current).ok()?.parent();
    }
    None
}

fn anatomical_basis(position: impl Fn(HumanBoneName) -> Option<Vec3>) -> Option<Quat> {
    use HumanBoneName::*;
    let up = [(Head, Hips), (Head, Neck), (Neck, Hips), (Chest, Hips)]
        .into_iter()
        .find_map(|(top, bottom)| (position(top)? - position(bottom)?).try_normalize())?;
    let left = [
        (LeftUpperLeg, RightUpperLeg),
        (LeftUpperArm, RightUpperArm),
        (LeftEye, RightEye),
    ]
    .into_iter()
    .find_map(|(left, right)| {
        let side = position(left)? - position(right)?;
        (side - up * side.dot(up)).try_normalize()
    })?;
    let forward = left.cross(up).try_normalize()?;
    let basis = Quat::from_mat3(&Mat3::from_cols(left, up, forward));
    valid_rotation(basis).then_some(basis.normalize())
}

fn raw_overlay(model_rest: Quat, basis: Quat, normalized: Quat) -> Quat {
    // A normalized avatar offset lives in anatomical model coordinates.
    // Conjugate through the ENTIRE raw rest hierarchy before postmultiplying
    // the current animated local joint. This also turns VRM0 offsets around Y.
    (model_rest.inverse() * basis * normalized * basis.inverse() * model_rest).normalize()
}

#[allow(clippy::too_many_arguments)]
fn apply_gaze(
    time: Res<Time>,
    external_pointer: Option<Res<GazePointer>>,
    avatar: Option<Res<AvatarLoadState>>,
    mut state: ResMut<GazeState>,
    mut cache: ResMut<GazeCache>,
    bones: Query<(Entity, &HumanBoneName, &HumanoidRestTransform)>,
    parents: Query<&ChildOf>,
    rests: Query<&HumanoidRestTransform>,
    mut transforms: Query<&mut Transform>,
    windows: Query<(Entity, &Window), With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &bevy::camera::RenderTarget), With<crate::visuals::VisualCamera>>,
) {
    let root = avatar.as_ref().and_then(|avatar| avatar.active);
    let config = state.config.sanitized();
    if root != cache.root || !config.enabled || config.intensity <= 0. {
        // Also handles a model or settings change later than PreUpdate.
        for bone in &mut cache.bones {
            if let Some(applied) = bone.applied.take()
                && let Ok(mut transform) = transforms.get_mut(bone.entity)
            {
                applied.remove(&mut transform.rotation);
            }
        }
        *cache = GazeCache::default();
        state.tracked_bones = 0;
        if !config.enabled || config.intensity <= 0. || root.is_none() {
            state.status = if root.is_none() {
                "Load a character to use gaze."
            } else {
                "Procedural gaze is off."
            }
            .into();
            return;
        }
    }
    let Some(root) = root else {
        return;
    };
    if cache.bones.is_empty() {
        let selected: Vec<_> = bones
            .iter()
            .filter(|(entity, _, _)| {
                *entity != root && parents.iter_ancestors(*entity).any(|parent| parent == root)
            })
            .filter_map(|(entity, name, _)| {
                model_rest(entity, root, &parents, &transforms, &rests)
                    .map(|(rotation, position)| (entity, *name, rotation, position))
            })
            .collect();
        let Some(basis) = anatomical_basis(|name| {
            selected
                .iter()
                .find(|(_, bone, _, _)| *bone == name)
                .map(|(_, _, _, position)| *position)
        }) else {
            state.status = "Waiting for rest bones; gaze needs head/hips and left/right anatomical anchors. Mirrored or invalid rest chains are unsupported.".into();
            return;
        };
        for (entity, name, model_rest, _) in selected {
            let part = match name {
                HumanBoneName::Head => Part::Head,
                HumanBoneName::Neck => Part::Neck,
                HumanBoneName::LeftEye | HumanBoneName::RightEye => Part::Eye,
                _ => continue,
            };
            cache.bones.push(GazeBone {
                entity,
                part,
                model_rest,
                applied: None,
            });
        }
        cache.root = Some(root);
        cache.basis = basis;
    }
    let pointer = (config.target == GazeTarget::Pointer)
        .then(|| {
            let (primary, window) = windows.single().ok()?;
            let cursor = external_pointer.as_ref()
                .and_then(|pointer| pointer.fresh(std::time::Instant::now()))
                .or_else(|| window.cursor_position())?;
            cameras
                .iter()
                .filter(|(camera, target)| camera.is_active && matches!(target,
                    bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary))
                    || camera.is_active && matches!(target,
                    bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Entity(entity)) if *entity == primary))
                .find_map(|(camera, _)| pointer_target(cursor, camera.logical_viewport_rect()?))
        })
        .flatten();
    let offsets = cache.motion.sample(&config, time.delta_secs(), pointer);
    let basis = cache.basis;
    let mut eyes = 0;
    let mut applied_count = 0;
    for bone in &mut cache.bones {
        let Ok(mut transform) = transforms.get_mut(bone.entity) else {
            continue;
        };
        if !valid_rotation(transform.rotation) {
            continue;
        }
        let offset = match bone.part {
            Part::Head => offsets[0],
            Part::Neck => offsets[1],
            Part::Eye => {
                eyes += 1;
                offsets[2]
            }
        };
        let base = transform.rotation;
        let result = (base * raw_overlay(bone.model_rest, basis, offset)).normalize();
        transform.rotation = result;
        bone.applied = Some(Applied { base, result });
        applied_count += 1;
    }
    state.tracked_bones = applied_count;
    state.status = if eyes == 0 {
        format!(
            "Gaze overlays {applied_count} head/neck bones; this rig has no eye bones. Morph-only eye tracking is not enabled."
        )
    } else {
        format!(
            "Gaze overlays {applied_count} bones, including {eyes} eyes, on the selected character."
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositor_pointer_expires_and_rejects_invalid_or_future_samples() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut pointer = GazePointer::default();
        pointer.update(Some(Vec2::new(20., 30.)), now);
        assert_eq!(
            pointer.fresh(now + Duration::from_millis(500)),
            Some(Vec2::new(20., 30.))
        );
        assert_eq!(pointer.fresh(now + Duration::from_millis(501)), None);
        assert_eq!(pointer.fresh(now - Duration::from_millis(1)), None);
        pointer.update(Some(Vec2::NAN), now);
        assert_eq!(pointer.fresh(now), None);
        pointer.update(Some(Vec2::ZERO), now);
        pointer.update(None, now);
        assert_eq!(pointer.fresh(now), None);
    }

    #[test]
    fn pointer_uses_avatar_viewport_and_rejects_outside_or_invalid_points() {
        let viewport = Rect::from_corners(Vec2::new(900., 100.), Vec2::new(1200., 700.));
        assert_eq!(
            pointer_target(Vec2::new(1050., 400.), viewport),
            Some(Vec2::ZERO)
        );
        assert_eq!(
            pointer_target(Vec2::new(900., 100.), viewport),
            Some(Vec2::new(-1., 1.))
        );
        assert_eq!(
            pointer_target(Vec2::new(1200., 700.), viewport),
            Some(Vec2::new(1., -1.))
        );
        assert_eq!(pointer_target(Vec2::new(899., 400.), viewport), None);
        assert_eq!(pointer_target(Vec2::new(f32::NAN, 400.), viewport), None);
        assert_eq!(
            pointer_target(Vec2::ZERO, Rect::from_corners(Vec2::ZERO, Vec2::ZERO)),
            None
        );
    }

    #[test]
    fn pointer_head_follow_has_correct_direction_bounds_and_no_drift_dependency() {
        let config = GazeConfig {
            intensity: 1.,
            head_follow: 1.,
            head_drift: 0.,
            eye_motion: 0.,
            smoothing_seconds: 0.05,
            target: GazeTarget::Pointer,
            ..Default::default()
        };
        let mut motion = Motion::default();
        let mut offsets = [Quat::IDENTITY; 3];
        for _ in 0..30 {
            offsets = motion.sample(&config, 0.1, Some(Vec2::new(1., 1.)));
        }
        let (yaw, pitch, _) = offsets[0].to_euler(EulerRot::YXZ);
        assert!((yaw - 0.35).abs() < 1e-5);
        assert!((pitch + 0.2).abs() < 1e-5);
        assert!(offsets[2].abs_diff_eq(Quat::IDENTITY, 1e-6));
        for _ in 0..30 {
            offsets = motion.sample(&config, 0.1, Some(Vec2::new(-10., -10.)));
        }
        let (yaw, pitch, _) = offsets[0].to_euler(EulerRot::YXZ);
        assert!((yaw + 0.35).abs() < 1e-5);
        assert!((pitch - 0.2).abs() < 1e-5);
    }

    #[test]
    fn sparse_settings_default_off_and_bad_numbers_are_bounded() {
        let config: GazeConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.enabled);
        assert_eq!(config.target, GazeTarget::Ambient);
        let invalid = GazeConfig {
            intensity: f32::NAN,
            head_drift: 4.,
            smoothing_seconds: 0.,
            ..config
        };
        assert!(invalid.validate().is_err());
        let fixed = invalid.sanitized();
        assert!(fixed.validate().is_ok());
        assert_eq!(fixed.head_drift, 1.);
        assert_eq!(fixed.smoothing_seconds, 0.05);
    }

    #[test]
    fn conditional_removal_never_overwrites_a_new_animation_rotation() {
        let base = Quat::from_rotation_y(0.7);
        let result = base * Quat::from_rotation_x(0.1);
        let applied = Applied { base, result };
        let mut owned = result;
        applied.remove(&mut owned);
        assert_eq!(owned, base);
        let newer = Quat::from_rotation_z(-0.4);
        let mut current = newer;
        applied.remove(&mut current);
        assert_eq!(current, newer);
    }

    #[test]
    fn raw_joint_conversion_handles_arbitrary_import_parents_and_vrm0_facing() {
        let parent = Quat::from_euler(EulerRot::XYZ, 0.7, -0.6, 0.2);
        let rest = Quat::from_euler(EulerRot::XYZ, -0.3, 0.5, 1.1);
        let model = parent * rest;
        let normalized = Quat::from_euler(EulerRot::YXZ, 0.15, 0.06, -0.01);
        for basis in [Quat::IDENTITY, Quat::from_rotation_y(std::f32::consts::PI)] {
            let actual = parent * rest * raw_overlay(model, basis, normalized);
            let expected = basis * normalized * basis.inverse() * model;
            assert!(actual.abs_diff_eq(expected, 1e-5));
        }
        let position = |negative: bool, name| match name {
            HumanBoneName::Hips => Some(Vec3::Y),
            HumanBoneName::Head => Some(Vec3::Y * 1.7),
            HumanBoneName::LeftUpperLeg => {
                Some(Vec3::new(if negative { -0.1 } else { 0.1 }, 1., 0.))
            }
            HumanBoneName::RightUpperLeg => {
                Some(Vec3::new(if negative { 0.1 } else { -0.1 }, 1., 0.))
            }
            _ => None,
        };
        assert!(
            (anatomical_basis(|name| position(false, name)).unwrap() * Vec3::Z)
                .abs_diff_eq(Vec3::Z, 1e-5)
        );
        assert!(
            (anatomical_basis(|name| position(true, name)).unwrap() * Vec3::Z)
                .abs_diff_eq(Vec3::NEG_Z, 1e-5)
        );
    }

    #[test]
    fn many_frames_do_not_accumulate_overlay_and_disable_restores_only_selected_avatar() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, GazePlugin))
            .init_resource::<AvatarLoadState>();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1. / 60.),
        ));
        let root = app
            .world_mut()
            .spawn(Transform::from_rotation(Quat::from_rotation_y(1.2)))
            .id();
        let other_root = app.world_mut().spawn(Transform::default()).id();
        let mut head = None;
        for (name, position) in [
            (HumanBoneName::Hips, Vec3::Y),
            (HumanBoneName::Head, Vec3::Y * 1.7),
            (HumanBoneName::LeftUpperLeg, Vec3::new(0.1, 1., 0.)),
            (HumanBoneName::RightUpperLeg, Vec3::new(-0.1, 1., 0.)),
        ] {
            let rest = Transform::from_translation(position);
            let entity = app
                .world_mut()
                .spawn((name, rest, HumanoidRestTransform(rest), ChildOf(root)))
                .id();
            if name == HumanBoneName::Head {
                head = Some(entity);
            }
        }
        let untouched = Transform::from_rotation(Quat::from_rotation_z(0.4));
        let other = app
            .world_mut()
            .spawn((
                HumanBoneName::Head,
                untouched,
                HumanoidRestTransform(untouched),
                ChildOf(other_root),
            ))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.world_mut().resource_mut::<GazeState>().config.enabled = true;
        let head = head.unwrap();
        for _ in 0..500 {
            app.update();
            let rotation = app.world().get::<Transform>(head).unwrap().rotation;
            assert!(rotation.angle_between(Quat::IDENTITY) < 0.35);
        }
        assert_eq!(*app.world().get::<Transform>(other).unwrap(), untouched);
        app.world_mut().resource_mut::<GazeState>().config.enabled = false;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(head).unwrap().rotation,
            Quat::IDENTITY
        );
    }

    #[test]
    fn switching_avatars_preserves_new_animation_and_restores_the_new_avatar_on_disable() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, GazePlugin))
            .init_resource::<AvatarLoadState>();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1. / 60.),
        ));
        let spawn_avatar = |world: &mut World| {
            let root = world.spawn(Transform::default()).id();
            let mut head = None;
            for (name, position) in [
                (HumanBoneName::Hips, Vec3::Y),
                (HumanBoneName::Head, Vec3::Y * 1.7),
                (HumanBoneName::LeftUpperLeg, Vec3::new(0.1, 1., 0.)),
                (HumanBoneName::RightUpperLeg, Vec3::new(-0.1, 1., 0.)),
            ] {
                let rest = Transform::from_translation(position);
                let entity = world
                    .spawn((name, rest, HumanoidRestTransform(rest), ChildOf(root)))
                    .id();
                if name == HumanBoneName::Head {
                    head = Some(entity);
                }
            }
            (root, head.unwrap())
        };
        let (first_root, first_head) = spawn_avatar(app.world_mut());
        let (second_root, second_head) = spawn_avatar(app.world_mut());
        app.world_mut().resource_mut::<GazeState>().config.enabled = true;
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(first_root);
        for _ in 0..30 {
            app.update();
        }
        assert_ne!(
            app.world().get::<Transform>(first_head).unwrap().rotation,
            Quat::IDENTITY
        );
        // A new controller may write after gaze applied its previous frame.
        let animated = Quat::from_rotation_z(0.23);
        app.world_mut()
            .get_mut::<Transform>(first_head)
            .unwrap()
            .rotation = animated;
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(second_root);
        for _ in 0..30 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Transform>(first_head).unwrap().rotation,
            animated
        );
        assert_ne!(
            app.world().get::<Transform>(second_head).unwrap().rotation,
            Quat::IDENTITY
        );
        app.world_mut().resource_mut::<GazeState>().config.enabled = false;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(second_head).unwrap().rotation,
            Quat::IDENTITY
        );
        assert_eq!(
            app.world().get::<Transform>(first_head).unwrap().rotation,
            animated
        );
    }
}
