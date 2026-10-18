//! Portable rest-axis-independent desktop sitting pose for the focused avatar.
use crate::{animation::AnimationState, loading::AvatarLoadState};
use bevy::prelude::*;
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;

#[derive(Resource, Default)]
pub struct DesktopPoseSettings {
    pub sitting: bool,
}
pub struct DesktopPosePlugin;
impl Plugin for DesktopPosePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DesktopPoseSettings>()
            .init_resource::<SitCache>()
            .add_systems(PreUpdate, relinquish_pose)
            .add_systems(
                PostUpdate,
                apply_pose
                    .after(bevy::app::AnimationSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}
#[derive(Clone, Copy)]
struct SitBone {
    entity: Entity,
    rest: Transform,
    rotation: Quat,
}
#[derive(Resource, Default)]
struct SitCache {
    root: Option<Entity>,
    bones: Vec<SitBone>,
    waiting: bool,
}
fn restore(cache: &mut SitCache, transforms: &mut Query<&mut Transform>) {
    for bone in cache.bones.drain(..) {
        if let Ok(mut transform) = transforms.get_mut(bone.entity) {
            *transform = bone.rest;
        }
    }
    cache.root = None;
    cache.waiting = false;
}
fn relinquish_pose(
    settings: Res<DesktopPoseSettings>,
    avatar: Option<Res<AvatarLoadState>>,
    animation: Option<Res<AnimationState>>,
    mut cache: ResMut<SitCache>,
    mut transforms: Query<&mut Transform>,
) {
    let active = avatar.as_ref().and_then(|state| state.active);
    // Keep Update's humanoid rest capture free of our pose even when playback
    // is started by the UI later in that same Update schedule.
    for bone in &cache.bones {
        if let Ok(mut transform) = transforms.get_mut(bone.entity) {
            *transform = bone.rest;
        }
    }
    if !settings.sitting
        || cache.root != active
        || animation.as_ref().is_some_and(|state| state.is_posing())
    {
        // This runs before humanoid animation caches its own rest pose in Update.
        restore(&mut cache, &mut transforms);
    }
}
fn valid_quat(q: Quat) -> bool {
    q.is_finite() && q.length_squared() > 0.5
}
fn aim(rest: Quat, parent: Quat, source: Vec3, desired: Vec3) -> Option<(Quat, Quat)> {
    if !valid_quat(rest) || !valid_quat(parent) || !source.is_finite() || !desired.is_finite() {
        return None;
    }
    let source = source.try_normalize()?;
    let desired = desired.try_normalize()?;
    let delta = Quat::from_rotation_arc(source, desired);
    let rotation = (parent.inverse() * delta * parent * rest).normalize();
    valid_quat(rotation).then_some((rotation, delta))
}
#[allow(clippy::too_many_arguments)]
fn apply_pose(
    settings: Res<DesktopPoseSettings>,
    avatar: Option<Res<AvatarLoadState>>,
    animation: Option<Res<AnimationState>>,
    mut cache: ResMut<SitCache>,
    bones: Query<(Entity, &HumanBoneName, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut transforms: Query<&mut Transform>,
) {
    let root = avatar.as_ref().and_then(|state| state.active);
    let posing = animation.as_ref().is_some_and(|state| state.is_posing());
    if posing {
        // PreUpdate normally restored before animation. If animation starts in
        // Update, only restore bones still carrying our exact previous rotation;
        // never overwrite a transform already owned by the animation system.
        for bone in cache.bones.drain(..) {
            if let Ok(mut transform) = transforms.get_mut(bone.entity)
                && transform.rotation.abs_diff_eq(bone.rotation, 1e-6)
            {
                *transform = bone.rest;
            }
        }
        cache.root = None;
        cache.waiting = false;
        return;
    }
    if !settings.sitting || root.is_none() || cache.root != root {
        restore(&mut cache, &mut transforms);
        if !settings.sitting || root.is_none() {
            return;
        }
        cache.root = root;
        // Globals must first reflect the restored/current idle local transforms.
        cache.waiting = true;
        return;
    }
    let root = root.unwrap();
    if cache.waiting {
        cache.waiting = false;
    }
    if cache.bones.is_empty() {
        let Ok(root_global) = globals.get(root) else {
            return;
        };
        let root_rotation = root_global.to_scale_rotation_translation().1;
        if !valid_quat(root_rotation) {
            return;
        }
        let find = |wanted| {
            bones.iter().find(|(entity, name, _)| {
                **name == wanted
                    && parents
                        .iter_ancestors(*entity)
                        .any(|ancestor| ancestor == root)
            })
        };
        // Root transforms include loader/import coordinate corrections and are
        // not necessarily anatomical. In a right-handed humanoid, anatomical
        // left crossed with up is forward. Derive both axes from rest globals.
        let up = find(HumanBoneName::Head)
            .zip(find(HumanBoneName::Hips))
            .and_then(|((_, _, head), (_, _, hips))| {
                (head.translation() - hips.translation()).try_normalize()
            })
            .filter(|up| up.is_finite())
            .unwrap_or(root_rotation * Vec3::Y);
        let forward = find(HumanBoneName::LeftUpperLeg)
            .zip(find(HumanBoneName::RightUpperLeg))
            .and_then(|((_, _, left), (_, _, right))| {
                (left.translation() - right.translation())
                    .cross(up)
                    .try_normalize()
            })
            .filter(|forward| forward.is_finite())
            .unwrap_or(root_rotation * Vec3::NEG_Z);
        for (upper_name, lower_name, foot_name) in [
            (
                HumanBoneName::LeftUpperLeg,
                HumanBoneName::LeftLowerLeg,
                HumanBoneName::LeftFoot,
            ),
            (
                HumanBoneName::RightUpperLeg,
                HumanBoneName::RightLowerLeg,
                HumanBoneName::RightFoot,
            ),
        ] {
            let (
                Some((upper_entity, _, upper)),
                Some((lower_entity, _, lower)),
                Some((foot_entity, _, foot)),
            ) = (find(upper_name), find(lower_name), find(foot_name))
            else {
                continue;
            };
            if !parents
                .iter_ancestors(lower_entity)
                .any(|ancestor| ancestor == upper_entity)
            {
                continue;
            }
            if !parents
                .iter_ancestors(foot_entity)
                .any(|ancestor| ancestor == lower_entity)
            {
                continue;
            }
            let parent_rotation = |entity| {
                parents
                    .get(entity)
                    .ok()
                    .and_then(|parent| globals.get(parent.parent()).ok())
                    .map(|global| global.to_scale_rotation_translation().1)
            };
            let (Some(upper_parent), Some(lower_parent)) =
                (parent_rotation(upper_entity), parent_rotation(lower_entity))
            else {
                continue;
            };
            let (Ok(upper_rest), Ok(lower_rest)) = (
                transforms.get(upper_entity).copied(),
                transforms.get(lower_entity).copied(),
            ) else {
                continue;
            };
            let Some((upper_rotation, upper_delta)) = aim(
                upper_rest.rotation,
                upper_parent,
                lower.translation() - upper.translation(),
                forward,
            ) else {
                continue;
            };
            let Some((lower_rotation, _)) = aim(
                lower_rest.rotation,
                upper_delta * lower_parent,
                upper_delta * (foot.translation() - lower.translation()),
                -up,
            ) else {
                continue;
            };
            cache.bones.push(SitBone {
                entity: upper_entity,
                rest: upper_rest,
                rotation: upper_rotation,
            });
            cache.bones.push(SitBone {
                entity: lower_entity,
                rest: lower_rest,
                rotation: lower_rotation,
            });
        }
    }
    for bone in &cache.bones {
        if let Ok(mut transform) = transforms.get_mut(bone.entity) {
            transform.rotation = bone.rotation;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_local_axes_aim_in_world_space() {
        let parent = Quat::from_euler(EulerRot::XYZ, 0.4, -0.7, 0.2);
        let rest = Quat::from_euler(EulerRot::XYZ, -0.2, 0.9, 0.6);
        let local_axis = Vec3::new(0.3, -0.8, 0.2).normalize();
        let source = parent * rest * local_axis;
        let desired = Vec3::Z;
        let (rotation, _) = aim(rest, parent, source, desired).unwrap();
        assert!((parent * rotation * local_axis).abs_diff_eq(desired, 1e-5));
        assert!(aim(rest, parent, Vec3::ZERO, desired).is_none());
        assert!(aim(rest, parent, Vec3::splat(f32::NAN), desired).is_none());
    }
    #[test]
    fn anatomical_forward_survives_loader_rotation_and_restores_exact_rest() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin, DesktopPosePlugin))
            .init_resource::<AvatarLoadState>();
        let root = app
            .world_mut()
            .spawn((
                Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
                GlobalTransform::default(),
            ))
            .id();
        // Typical import correction cancels the loader root half-turn. The
        // skeleton faces world +Z although root-local +Z points world -Z.
        let skeleton = app
            .world_mut()
            .spawn((
                Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
                GlobalTransform::default(),
                ChildOf(root),
            ))
            .id();
        for (bone, position) in [
            (HumanBoneName::RightUpperLeg, Vec3::new(-0.1, 1.0, 0.0)),
            (HumanBoneName::Hips, Vec3::Y),
            (HumanBoneName::Head, Vec3::Y * 1.8),
        ] {
            app.world_mut().spawn((
                bone,
                Transform::from_translation(position),
                GlobalTransform::default(),
                ChildOf(skeleton),
            ));
        }
        let upper_rest =
            Transform::from_xyz(0.1, 1.0, 0.0).with_rotation(Quat::from_rotation_y(0.37));
        let upper = app
            .world_mut()
            .spawn((
                HumanBoneName::LeftUpperLeg,
                upper_rest,
                GlobalTransform::default(),
                ChildOf(skeleton),
            ))
            .id();
        let lower_rest =
            Transform::from_xyz(0.0, -0.45, 0.0).with_rotation(Quat::from_rotation_y(-0.24));
        let lower = app
            .world_mut()
            .spawn((
                HumanBoneName::LeftLowerLeg,
                lower_rest,
                GlobalTransform::default(),
                ChildOf(upper),
            ))
            .id();
        let foot = app
            .world_mut()
            .spawn((
                HumanBoneName::LeftFoot,
                Transform::from_xyz(0.0, -0.45, 0.0),
                GlobalTransform::default(),
                ChildOf(lower),
            ))
            .id();
        let extra_rest = Transform::from_rotation(Quat::from_rotation_x(0.2));
        let extra = app
            .world_mut()
            .spawn((
                HumanBoneName::RightUpperLeg,
                extra_rest,
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        app.world_mut()
            .resource_mut::<DesktopPoseSettings>()
            .sitting = true;
        app.update();
        app.update();
        let upper_global = app
            .world()
            .get::<GlobalTransform>(upper)
            .unwrap()
            .translation();
        let lower_global = app
            .world()
            .get::<GlobalTransform>(lower)
            .unwrap()
            .translation();
        let foot_global = app
            .world()
            .get::<GlobalTransform>(foot)
            .unwrap()
            .translation();
        assert!(
            (lower_global - upper_global)
                .normalize()
                .abs_diff_eq(Vec3::Z, 1e-4)
        );
        assert!(
            (foot_global - lower_global)
                .normalize()
                .abs_diff_eq(Vec3::NEG_Y, 1e-4)
        );
        assert_eq!(*app.world().get::<Transform>(extra).unwrap(), extra_rest);
        app.world_mut()
            .resource_mut::<DesktopPoseSettings>()
            .sitting = false;
        app.update();
        assert_eq!(*app.world().get::<Transform>(upper).unwrap(), upper_rest);
        assert_eq!(*app.world().get::<Transform>(lower).unwrap(), lower_rest);
    }
}
