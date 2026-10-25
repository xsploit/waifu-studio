//! Opt-in diagnostic only: restored before Update, applied before propagation/springs.
use super::Capture;
use bevy::{math::Affine3A, prelude::*};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName as Bone;
use waifu_studio::loading::AvatarLoadState;

#[derive(Resource, Default)]
pub(super) struct Probe {
    saved: Vec<(Entity, Quat)>,
    pub applied: bool,
}

pub(super) fn restore(mut probe: ResMut<Probe>, mut transforms: Query<&mut Transform>) {
    for (entity, rotation) in probe.saved.drain(..) {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.rotation = rotation;
        }
    }
    probe.applied = false;
}

fn model(
    mut entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
) -> Option<Affine3A> {
    let mut result = Affine3A::IDENTITY;
    for _ in 0..128 {
        if entity == root {
            return Some(result);
        }
        result = transforms.get(entity).ok()?.compute_affine() * result;
        entity = parents.get(entity).ok()?.parent();
    }
    None
}

fn rotation_space(value: Affine3A) -> Result<Quat, &'static str> {
    let (scale, rotation, _) = value.to_scale_rotation_translation();
    let axes = [
        value.matrix3.x_axis,
        value.matrix3.y_axis,
        value.matrix3.z_axis,
    ];
    let orthogonal = [(0, 1), (0, 2), (1, 2)]
        .into_iter()
        .all(|(a, b)| axes[a].dot(axes[b]).abs() < 1e-5 * axes[a].length() * axes[b].length());
    if !value.is_finite()
        || scale.min_element() <= 1e-6
        || value.matrix3.determinant() <= 0.
        || !orthogonal
        || scale.max_element() - scale.min_element() > 1e-4 * scale.max_element()
    {
        return Err("Arm pose probe requires positive uniform scale without shear");
    }
    Ok(rotation.normalize())
}

fn adjusted(parent: Quat, original: Quat, axis: Vec3) -> Result<Quat, &'static str> {
    let axis = axis
        .try_normalize()
        .ok_or("Arm pose probe has a degenerate upper arm")?;
    if axis.x.abs() < 1e-4 {
        return Err("Arm pose probe has an ambiguous left/right axis");
    }
    let sign = axis.x.signum();
    let change = Quat::from_rotation_z(sign * 10_f32.to_radians())
        * Quat::from_axis_angle(axis, sign * (-20_f32).to_radians());
    Ok((parent.inverse() * change * parent * original).normalize())
}

pub(super) fn apply(
    state: Res<Capture>,
    avatar: Res<AvatarLoadState>,
    bones: Query<(Entity, &Bone)>,
    parents: Query<&ChildOf>,
    mut transforms: Query<&mut Transform>,
    mut probe: ResMut<Probe>,
    mut exit: MessageWriter<AppExit>,
) {
    if !state.options.arm_pose_probe {
        return;
    }
    let Some(root) = avatar.active else { return };
    let mut changes = Vec::new();
    for (upper, lower) in [
        (Bone::LeftUpperArm, Bone::LeftLowerArm),
        (Bone::RightUpperArm, Bone::RightLowerArm),
    ] {
        let find = |bone| {
            bones
                .iter()
                .filter(|(_, b)| **b == bone)
                .find_map(|(entity, _)| {
                    model(entity, root, &parents, &transforms).map(|m| (entity, m))
                })
        };
        let (Some((entity, upper_model)), Some((_, lower_model))) = (find(upper), find(lower))
        else {
            return;
        };
        let Ok(parent) = parents.get(entity) else {
            return;
        };
        let Some(parent_model) = model(parent.parent(), root, &parents, &transforms) else {
            return;
        };
        let result = (|| {
            rotation_space(upper_model)?;
            rotation_space(lower_model)?;
            let parent_rotation = rotation_space(parent_model)?;
            let old = transforms
                .get(entity)
                .map_err(|_| "Missing probe arm transform")?
                .rotation;
            let axis = Vec3::from(lower_model.translation - upper_model.translation);
            Ok::<_, &str>((old, adjusted(parent_rotation, old, axis)?))
        })();
        match result {
            Ok((old, new)) => changes.push((entity, old, new)),
            Err(error) => {
                eprintln!("{error}");
                exit.write(AppExit::error());
                return;
            }
        }
    }
    // Compute both sides from the same input before changing either local pose.
    for (entity, old, new) in changes {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.rotation = new;
            probe.saved.push((entity, old));
        }
    }
    probe.applied = probe.saved.len() == 2;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_adjustment_matches_parented_rotation_and_preserves_lengths() {
        let parent = Quat::from_euler(EulerRot::XYZ, 0.2, -0.4, 0.3);
        let original = Quat::from_rotation_y(0.6);
        for axis in [Vec3::new(-0.1, -0.17, 0.02), Vec3::new(0.13, -0.15, 0.01)] {
            let result = adjusted(parent, original, axis).unwrap();
            let model_change = parent * result * original.inverse() * parent.inverse();
            let expected = Quat::from_rotation_z(axis.x.signum() * 10_f32.to_radians()) * axis;
            assert!((model_change * axis - expected).length() < 1e-6);
            assert!(((model_change * axis).length() - axis.length()).abs() < 1e-6);
        }
        assert!(adjusted(parent, original, Vec3::ZERO).is_err());
        assert!(adjusted(parent, original, Vec3::Y).is_err());
        assert!(rotation_space(Affine3A::from_scale(Vec3::new(1., 2., 1.))).is_err());
    }
    #[test]
    fn first_schedule_restores_input_instead_of_accumulating_the_probe() {
        let mut app = App::new();
        let original = Quat::from_rotation_y(0.4);
        let entity = app
            .world_mut()
            .spawn(Transform::from_rotation(original))
            .id();
        app.init_resource::<Probe>().add_systems(First, restore);
        for _ in 0..3 {
            app.world_mut()
                .get_mut::<Transform>(entity)
                .unwrap()
                .rotation = Quat::from_rotation_x(1.);
            app.world_mut()
                .resource_mut::<Probe>()
                .saved
                .push((entity, original));
            app.update();
            assert!(
                app.world()
                    .get::<Transform>(entity)
                    .unwrap()
                    .rotation
                    .angle_between(original)
                    < 1e-6
            );
            assert!(app.world().resource::<Probe>().saved.is_empty());
        }
    }
}
