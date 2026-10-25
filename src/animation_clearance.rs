//! Experimental post-retarget arm clearance, in avatar-model space.
//! Torso volume comes from skinned mesh support points; arm capsules include
//! the upper arms. The proximal fifth allows attachment at the shoulder.
//! Runs after crossfading so both clips and their blended poses are considered.
use super::PoseBone;
use bevy::{math::Affine3A, prelude::*};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName as Bone;

const SHOULDER_ATTACHMENT: f32 = 0.2;

trait BodyShape {
    fn push(&self, point: Vec3, margin: f32) -> Vec3;
    fn segment_contact(&self, start: Vec3, end: Vec3, margin: f32, begin: f32) -> (Vec3, f32);
    fn exit_candidates(&self, point: Vec3, margin: f32) -> Vec<Vec3> {
        vec![point + self.push(point, margin)]
    }
}

pub(super) struct Hull {
    planes: Vec<(Vec3, f32)>,
}
impl Hull {
    /// Intersection of mesh support planes. It encloses the selected torso
    /// vertices conservatively, including front/back and diagonal silhouettes.
    pub(super) fn from_points(points: &[Vec3]) -> Option<Self> {
        if points.len() < 4 || points.iter().any(|point| !point.is_finite()) {
            return None;
        }
        let mut planes = Vec::with_capacity(26);
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let direction = Vec3::new(x as f32, y as f32, z as f32);
                    if let Some(normal) = direction.try_normalize() {
                        let support = points
                            .iter()
                            .map(|point| normal.dot(*point))
                            .fold(f32::NEG_INFINITY, f32::max);
                        planes.push((normal, support));
                    }
                }
            }
        }
        Some(Self { planes })
    }
    fn clearance(&self, point: Vec3, margin: f32) -> (f32, Vec3) {
        self.planes
            .iter()
            .map(|(normal, offset)| (offset + margin - normal.dot(point), *normal))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((f32::NEG_INFINITY, Vec3::ZERO))
    }
}
impl BodyShape for Hull {
    fn exit_candidates(&self, point: Vec3, margin: f32) -> Vec<Vec3> {
        let mut result = vec![point + self.push(point, margin)];
        if self.clearance(point, margin).0 > 0. {
            for &(normal, offset) in &self.planes {
                if normal.abs().max_element() > 0.99 {
                    result.push(point + normal * (offset + margin - normal.dot(point)));
                }
            }
        }
        result
    }
    fn push(&self, point: Vec3, margin: f32) -> Vec3 {
        let (depth, normal) = self.clearance(point, margin);
        normal * depth.max(0.)
    }
    fn segment_contact(&self, start: Vec3, end: Vec3, margin: f32, begin: f32) -> (Vec3, f32) {
        // Minimum plane slack along a segment is concave and piecewise linear.
        // Find its maximum, including cases where both endpoints are outside.
        // This tests the whole capsule axis rather than three sample locations.
        let mut lo = begin;
        let mut hi = 1.;
        for _ in 0..18 {
            let a = lo + (hi - lo) / 3.;
            let b = hi - (hi - lo) / 3.;
            if self.clearance(start.lerp(end, a), margin).0
                < self.clearance(start.lerp(end, b), margin).0
            {
                lo = a;
            } else {
                hi = b;
            }
        }
        let mut best = (Vec3::ZERO, begin);
        for t in [begin, (lo + hi) * 0.5, 1.] {
            let push = self.push(start.lerp(end, t), margin);
            if push.length_squared() > best.0.length_squared() {
                best = (push, t);
            }
        }
        best
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct Ellipsoid {
    center: Vec3,
    rotation: Quat,
    radii: Vec3,
}
#[cfg(test)]
impl BodyShape for Ellipsoid {
    fn push(&self, point: Vec3, margin: f32) -> Vec3 {
        let radii = self.radii + Vec3::splat(margin);
        let local = self.rotation.inverse() * (point - self.center);
        let scaled = local / radii;
        let distance = scaled.length();
        if !distance.is_finite() || distance >= 1. {
            return Vec3::ZERO;
        }
        let direction = scaled.try_normalize().unwrap_or(Vec3::X);
        self.rotation * (direction * radii - local)
    }
    fn segment_contact(&self, start: Vec3, end: Vec3, margin: f32, begin: f32) -> (Vec3, f32) {
        // Analytic point nearest the ellipsoid center in scaled coordinates.
        let a =
            self.rotation.inverse() * (start - self.center) / (self.radii + Vec3::splat(margin));
        let delta = self.rotation.inverse() * (end - start) / (self.radii + Vec3::splat(margin));
        let t = (-a.dot(delta) / delta.length_squared().max(1e-12)).clamp(begin, 1.);
        (self.push(start.lerp(end, t), margin), t)
    }
}

pub(super) fn model_transform(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
) -> Option<Affine3A> {
    let mut current = entity;
    let mut result = Affine3A::IDENTITY;
    for _ in 0..128 {
        if current == root {
            let (scale, _, _) = result.to_scale_rotation_translation();
            let axes = [
                result.matrix3.x_axis,
                result.matrix3.y_axis,
                result.matrix3.z_axis,
            ];
            let orthogonal = [(0, 1), (0, 2), (1, 2)].into_iter().all(|(a, b)| {
                axes[a].dot(axes[b]).abs() < 1e-5 * axes[a].length() * axes[b].length()
            });
            // Quaternion-only IK cannot preserve lengths under shear/nonuniform scale.
            return (result.is_finite()
                && scale.min_element() > 1e-6
                && result.matrix3.determinant() > 0.
                && orthogonal
                && scale.max_element() - scale.min_element() < 1e-4 * scale.max_element())
            .then_some(result);
        }
        result = transforms.get(current).ok()?.compute_affine() * result;
        current = parents.get(current).ok()?.parent();
    }
    None
}

fn two_bone(shoulder: Vec3, elbow: Vec3, wrist: Vec3, target: Vec3, pole: Vec3) -> (Vec3, Vec3) {
    let upper = shoulder.distance(elbow);
    let lower = elbow.distance(wrist);
    if upper.min(lower) < 1e-6 || !target.is_finite() || !pole.is_finite() {
        return (elbow, wrist);
    }
    let axis = (target - shoulder)
        .try_normalize()
        .or_else(|| (wrist - shoulder).try_normalize())
        .unwrap_or(Vec3::Y);
    let reach = shoulder
        .distance(target)
        .clamp((upper - lower).abs() + 1e-6, upper + lower - 1e-6);
    let along = (upper * upper - lower * lower + reach * reach) / (2. * reach);
    let radius = (upper * upper - along * along).max(0.).sqrt();
    let preferred = pole - shoulder;
    let bend = (preferred - axis * preferred.dot(axis))
        .try_normalize()
        .unwrap_or_else(|| axis.any_orthonormal_vector());
    (
        shoulder + axis * along + bend * radius,
        shoulder + axis * reach,
    )
}

fn push_all(mut point: Vec3, shapes: &[impl BodyShape], margin: f32) -> Vec3 {
    let original = point;
    for shape in shapes {
        point += shape.push(point, margin);
    }
    point - original
}

#[allow(clippy::too_many_arguments)]
fn solve(
    shoulder: Vec3,
    elbow: Vec3,
    wrist: Vec3,
    fingers: &[Vec3],
    shapes: &[impl BodyShape],
    size: f32,
    radii: (f32, f32),
) -> (Vec3, Vec3) {
    let mut e = elbow;
    let mut w = wrist;
    let score = |e: Vec3, w: Vec3| {
        let mut score = 0.;
        for shape in shapes {
            score += shape.push(e, radii.1).length_squared();
            score += shape.push(w, size * 0.020).length_squared();
            for offset in fingers {
                score += shape.push(w + *offset, size * 0.005).length_squared();
            }
            score += shape
                .segment_contact(shoulder, e, radii.0, SHOULDER_ATTACHMENT)
                .0
                .length_squared();
            score += shape.segment_contact(e, w, radii.1, 0.).0.length_squared();
        }
        score
    };
    let mut best = (e, w);
    let raw_score = score;
    let score = |e: Vec3, w: Vec3| (raw_score(e, w).sqrt() - size * 0.0005).max(0.).powi(2);
    let mut best_score = score(e, w);
    if best_score == 0. {
        return best;
    }
    let axis = (wrist - shoulder).try_normalize().unwrap_or(Vec3::Y);
    let mut wrist_seeds = vec![wrist];
    for shape in shapes {
        wrist_seeds.extend(shape.exit_candidates(wrist, size * 0.020));
    }
    for wrist_seed in wrist_seeds {
        // Search elbow bend planes before moving the hand far from its gesture.
        for angle in [0_f32, 0.5, -0.5, 1., -1., 1.5, -1.5, std::f32::consts::PI] {
            (e, w) = two_bone(
                shoulder,
                elbow,
                wrist,
                wrist_seed,
                shoulder + Quat::from_axis_angle(axis, angle) * (elbow - shoulder),
            );
            for _ in 0..24 {
                let elbow_push = push_all(e, shapes, radii.1);
                let mut pole = e + elbow_push;
                // A pole alone cannot move an elbow along its current bend plane.
                // Let the wrist move too when necessary to make elbow clearance feasible.
                let mut target = w + push_all(w, shapes, size * 0.020) + elbow_push;
                // Preserve the original hand's model orientation while protecting its
                // palm/finger envelope as well as the wrist joint itself.
                for offset in fingers {
                    target += push_all(target + *offset, shapes, size * 0.005);
                }
                for shape in shapes {
                    let (correction, t) =
                        shape.segment_contact(shoulder, pole, radii.0, SHOULDER_ATTACHMENT);
                    pole += correction / t.max(SHOULDER_ATTACHMENT);
                    target += correction * 0.5 / t.max(SHOULDER_ATTACHMENT);
                    let (correction, t) = shape.segment_contact(pole, target, radii.1, 0.);
                    let denominator = (1. - t).powi(2) + t.powi(2);
                    pole += correction * (1. - t) / denominator;
                    target += correction * t / denominator;
                }
                let candidate = score(e, w);
                let movement =
                    |e: Vec3, w: Vec3| e.distance_squared(elbow) + w.distance_squared(wrist) * 12.;
                if candidate < best_score - 1e-10
                    || ((candidate - best_score).abs() < 1e-10
                        && movement(e, w) < movement(best.0, best.1))
                {
                    best_score = candidate;
                    best = (e, w);
                }
                if pole.distance_squared(e) + target.distance_squared(w) < 1e-12 {
                    break;
                }
                (e, w) = two_bone(shoulder, elbow, wrist, target, pole);
            }
            let candidate = score(e, w);
            let movement =
                |e: Vec3, w: Vec3| e.distance_squared(elbow) + w.distance_squared(wrist) * 12.;
            if candidate < best_score - 1e-10
                || ((candidate - best_score).abs() < 1e-10
                    && movement(e, w) < movement(best.0, best.1))
            {
                best_score = candidate;
                best = (e, w);
            }
        }
    }
    best
}

fn set_model_rotation(
    entity: Entity,
    desired: Quat,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &mut Query<&mut Transform>,
) -> Option<()> {
    let parent = parents.get(entity).ok()?.parent();
    let (_, parent_rotation, _) =
        model_transform(parent, root, parents, transforms)?.to_scale_rotation_translation();
    let rotation = (parent_rotation.inverse() * desired).normalize();
    if !rotation.is_finite() {
        return None;
    }
    transforms.get_mut(entity).ok()?.rotation = rotation;
    Some(())
}

pub(super) fn correct_arms(
    root: Entity,
    bones: &[PoseBone],
    parents: &Query<&ChildOf>,
    transforms: &mut Query<&mut Transform>,
    hull: &Hull,
    radii: (f32, f32),
) {
    let find = |name| bones.iter().find(|bone| bone.bone == name);
    let Some((hips, neck)) = find(Bone::Hips).zip(find(Bone::Neck)) else {
        return;
    };
    let size = hips.rest_position.distance(neck.rest_position) * 4.;
    if !size.is_finite() || size < 1e-5 {
        return;
    }
    let shapes = std::slice::from_ref(hull);
    for (upper, lower, hand, tips) in [
        (
            Bone::LeftUpperArm,
            Bone::LeftLowerArm,
            Bone::LeftHand,
            [
                Bone::LeftIndexDistal,
                Bone::LeftMiddleDistal,
                Bone::LeftRingDistal,
                Bone::LeftLittleDistal,
                Bone::LeftThumbDistal,
            ],
        ),
        (
            Bone::RightUpperArm,
            Bone::RightLowerArm,
            Bone::RightHand,
            [
                Bone::RightIndexDistal,
                Bone::RightMiddleDistal,
                Bone::RightRingDistal,
                Bone::RightLittleDistal,
                Bone::RightThumbDistal,
            ],
        ),
    ] {
        let Some(((upper, lower), hand)) = find(upper).zip(find(lower)).zip(find(hand)) else {
            continue;
        };
        let Some(((s, e), w)) = model_transform(upper.entity, root, parents, transforms)
            .zip(model_transform(lower.entity, root, parents, transforms))
            .zip(model_transform(hand.entity, root, parents, transforms))
        else {
            continue;
        };
        let (_, upper_rotation, shoulder) = s.to_scale_rotation_translation();
        let (_, _, elbow) = e.to_scale_rotation_translation();
        let (_, hand_rotation, wrist) = w.to_scale_rotation_translation();
        let fingers: Vec<_> = tips
            .iter()
            .filter_map(|tip| {
                let bone = find(*tip)?;
                Some(
                    Vec3::from(
                        model_transform(bone.entity, root, parents, transforms)?.translation,
                    ) - wrist,
                )
            })
            .collect();
        let (new_elbow, new_wrist) = solve(shoulder, elbow, wrist, &fingers, shapes, size, radii);
        if new_elbow.distance_squared(elbow) + new_wrist.distance_squared(wrist) < 1e-12 {
            continue;
        }
        let Some(a) = (elbow - shoulder).try_normalize() else {
            continue;
        };
        let Some(b) = (new_elbow - shoulder).try_normalize() else {
            continue;
        };
        let originals = [upper.entity, lower.entity, hand.entity]
            .map(|entity| transforms.get(entity).copied().ok());
        let applied = (|| -> Option<()> {
            set_model_rotation(
                upper.entity,
                Quat::from_rotation_arc(a, b) * upper_rotation,
                root,
                parents,
                transforms,
            )?;
            let (e, w) = model_transform(lower.entity, root, parents, transforms)
                .zip(model_transform(hand.entity, root, parents, transforms))?;
            let (_, lower_rotation, elbow) = e.to_scale_rotation_translation();
            let (_, _, wrist) = w.to_scale_rotation_translation();
            let a = (wrist - elbow).try_normalize()?;
            let b = (new_wrist - elbow).try_normalize()?;
            set_model_rotation(
                lower.entity,
                Quat::from_rotation_arc(a, b) * lower_rotation,
                root,
                parents,
                transforms,
            )?;
            set_model_rotation(hand.entity, hand_rotation, root, parents, transforms)
        })();
        if applied.is_none() {
            for (entity, original) in [upper.entity, lower.entity, hand.entity]
                .into_iter()
                .zip(originals)
            {
                if let Some(original) = original
                    && let Ok(mut transform) = transforms.get_mut(entity)
                {
                    *transform = original;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_shape(center: Vec3, radii: Vec3) -> Hull {
        let points: Vec<_> = [-1., 1.]
            .into_iter()
            .flat_map(|x| {
                [-1., 1.].into_iter().flat_map(move |y| {
                    [-1., 1.]
                        .into_iter()
                        .map(move |z| center + radii * Vec3::new(x, y, z))
                })
            })
            .collect();
        Hull::from_points(&points).unwrap()
    }

    #[test]
    fn capsule_crossing_is_detected_even_with_clear_endpoints_and_old_samples() {
        let body = box_shape(Vec3::new(0.2, 0., 0.), Vec3::new(0.02, 0.1, 0.1));
        let a = Vec3::NEG_X;
        let b = Vec3::X;
        for fraction in [0., 0.25, 0.5, 0.75, 1.] {
            assert_eq!(body.push(a.lerp(b, fraction), 0.), Vec3::ZERO);
        }
        let (correction, t) = body.segment_contact(a, b, 0., 0.);
        assert!(correction.length() > 0.019);
        assert!((t - 0.6).abs() < 0.001);
    }

    #[test]
    fn upper_arm_crossing_is_corrected_when_wrist_and_forearm_are_already_clear() {
        let body = box_shape(Vec3::ZERO, Vec3::splat(0.12));
        let s = Vec3::new(-0.3, 0.3, 0.);
        let e = Vec3::new(0.3, -0.3, 0.);
        let w = Vec3::new(0.5, -0.4, 0.1);
        assert_eq!(body.push(w, 0.015), Vec3::ZERO);
        assert_eq!(body.segment_contact(e, w, 0.015, 0.).0, Vec3::ZERO);
        let original = body.segment_contact(s, e, 0.02, 0.2).0.length();
        assert!(original > 0.1);
        let (a, b) = solve(s, e, w, &[], &[body], 1., (0.02, 0.015));
        let body = box_shape(Vec3::ZERO, Vec3::splat(0.12));
        assert!(body.segment_contact(s, a, 0.02, 0.2).0.length() < 0.001);
        assert!((s.distance(a) - s.distance(e)).abs() < 1e-5);
        assert!((a.distance(b) - e.distance(w)).abs() < 1e-5);
    }

    #[test]
    fn ecs_ik_preserves_hand_orientation_and_other_avatar() {
        use bevy::ecs::system::SystemState;
        let mut world = World::new();
        let root = world
            .spawn(
                Transform::from_xyz(4., 2., -3.)
                    .with_rotation(Quat::from_rotation_y(1.2))
                    .with_scale(Vec3::splat(3.)),
            )
            .id();
        let hips = world
            .spawn((Transform::from_xyz(0., 0.8, 0.), ChildOf(root)))
            .id();
        let chest = world
            .spawn((Transform::from_xyz(0., 0.2, 0.), ChildOf(hips)))
            .id();
        let neck = world
            .spawn((Transform::from_xyz(0., 0.15, 0.), ChildOf(chest)))
            .id();
        let upper = world
            .spawn((Transform::from_xyz(0.12, 0.1, 0.), ChildOf(chest)))
            .id();
        let lower = world
            .spawn((Transform::from_xyz(0.25, 0., 0.), ChildOf(upper)))
            .id();
        let hand = world
            .spawn((Transform::from_xyz(0.25, 0., 0.), ChildOf(lower)))
            .id();
        let right = world
            .spawn((Transform::from_xyz(-0.12, 0.1, 0.), ChildOf(chest)))
            .id();
        let untouched = world
            .spawn(Transform::from_rotation(Quat::from_rotation_x(0.7)))
            .id();
        let bones: Vec<_> = [
            (hips, Bone::Hips, Vec3::new(0., 0.8, 0.)),
            (chest, Bone::UpperChest, Vec3::new(0., 1., 0.)),
            (neck, Bone::Neck, Vec3::new(0., 1.15, 0.)),
            (upper, Bone::LeftUpperArm, Vec3::new(0.12, 1.1, 0.)),
            (lower, Bone::LeftLowerArm, Vec3::new(0.37, 1.1, 0.)),
            (hand, Bone::LeftHand, Vec3::new(0.62, 1.1, 0.)),
            (right, Bone::RightUpperArm, Vec3::new(-0.12, 1.1, 0.)),
        ]
        .into_iter()
        .map(|(entity, bone, rest_position)| PoseBone {
            entity,
            bone,
            rest: *world.get::<Transform>(entity).unwrap(),
            idle: *world.get::<Transform>(entity).unwrap(),
            rest_rotation: Quat::IDENTITY,
            rest_position,
            model_to_parent: None,
            joint: None,
            depth: 0,
        })
        .collect();
        world.get_mut::<Transform>(upper).unwrap().rotation =
            Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
        world.get_mut::<Transform>(lower).unwrap().rotation = Quat::from_rotation_z(-2.4);
        let mut system: SystemState<(Query<&ChildOf>, Query<&mut Transform>)> =
            SystemState::new(&mut world);
        let (parents, mut transforms) = system.get_mut(&mut world).unwrap();
        let (_, original_rotation, original_position) =
            model_transform(hand, root, &parents, &transforms)
                .unwrap()
                .to_scale_rotation_translation();
        let hull = Hull::from_points(&[
            Vec3::new(-0.13, 0.75, -0.1),
            Vec3::new(0.13, 0.75, -0.1),
            Vec3::new(-0.13, 1.15, -0.1),
            Vec3::new(0.13, 1.15, -0.1),
            Vec3::new(-0.13, 0.75, 0.1),
            Vec3::new(0.13, 0.75, 0.1),
            Vec3::new(-0.13, 1.15, 0.1),
            Vec3::new(0.13, 1.15, 0.1),
        ])
        .unwrap();
        correct_arms(
            root,
            &bones,
            &parents,
            &mut transforms,
            &hull,
            (0.012, 0.012),
        );
        let (_, rotation, position) = model_transform(hand, root, &parents, &transforms)
            .unwrap()
            .to_scale_rotation_translation();
        let elbow = Vec3::from(
            model_transform(lower, root, &parents, &transforms)
                .unwrap()
                .translation,
        );
        let shoulder = Vec3::from(
            model_transform(upper, root, &parents, &transforms)
                .unwrap()
                .translation,
        );
        assert!(position.distance(original_position) > 0.02);
        assert!(rotation.angle_between(original_rotation) < 0.001);
        assert!((shoulder.distance(elbow) - 0.25).abs() < 1e-5);
        assert!((elbow.distance(position) - 0.25).abs() < 1e-5);
        assert_eq!(transforms.get(right).unwrap().rotation, Quat::IDENTITY);
        assert_eq!(
            transforms.get(untouched).unwrap().rotation,
            Quat::from_rotation_x(0.7)
        );
    }

    #[test]
    fn clear_poses_are_unchanged() {
        let s = Vec3::new(0.3, 1., 0.);
        let e = Vec3::new(0.4, 0.7, 0.);
        let w = Vec3::new(0.5, 0.5, 0.);
        let shape = Ellipsoid {
            center: Vec3::Y,
            rotation: Quat::IDENTITY,
            radii: Vec3::splat(0.15),
        };
        assert_eq!(solve(s, e, w, &[], &[shape], 1., (0.012, 0.012)), (e, w));
    }

    #[test]
    fn equal_axis_lengths_do_not_hide_sheared_hierarchies() {
        use bevy::ecs::system::SystemState;
        let mut world = World::new();
        let root = world.spawn(Transform::default()).id();
        let parent = world
            .spawn((Transform::from_scale(Vec3::new(2., 1., 1.)), ChildOf(root)))
            .id();
        let joint = world
            .spawn((
                Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4))
                    .with_scale(Vec3::new(1., 1., 2.5_f32.sqrt())),
                ChildOf(parent),
            ))
            .id();
        let mut system: SystemState<(Query<&ChildOf>, Query<&mut Transform>)> =
            SystemState::new(&mut world);
        let (parents, transforms) = system.get_mut(&mut world).unwrap();
        assert!(model_transform(joint, root, &parents, &transforms).is_none());
    }

    #[test]
    fn elbow_clearance_can_move_wrist_when_pole_alone_cannot_help() {
        let s = Vec3::ZERO;
        let e = Vec3::X;
        let w = Vec3::new(1., 1., 0.);
        let shape = Ellipsoid {
            center: Vec3::new(0.99, 0., 0.),
            rotation: Quat::IDENTITY,
            radii: Vec3::splat(0.03),
        };
        let (a, b) = solve(s, e, w, &[], &[shape], 1., (0.012, 0.012));
        assert!(shape.push(a, 0.012).length() < shape.push(e, 0.012).length() * 0.1);
        assert!((s.distance(a) - 1.).abs() < 1e-5);
        assert!((a.distance(b) - 1.).abs() < 1e-5);
    }

    #[test]
    fn penetrating_hand_is_moved_outside_without_stretching() {
        let s = Vec3::new(0.22, 0.3, 0.);
        let e = Vec3::new(0.32, 0.05, 0.);
        let w = Vec3::new(0.03, 0., 0.03);
        let shape = Ellipsoid {
            center: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            radii: Vec3::new(0.15, 0.2, 0.12),
        };
        let (a, b) = solve(s, e, w, &[], &[shape], 1., (0.012, 0.012));
        assert!(shape.push(b, 0.019).length() < 1e-4, "wrist {b:?}");
        assert!((s.distance(a) - s.distance(e)).abs() < 1e-5);
        assert!((a.distance(b) - e.distance(w)).abs() < 1e-5);
    }

    #[test]
    fn ik_handles_unreachable_and_folded_targets() {
        let s = Vec3::ZERO;
        let e = Vec3::X;
        let w = Vec3::new(1., 1., 0.);
        for target in [s, Vec3::splat(100.), w] {
            let (a, b) = two_bone(s, e, w, target, e);
            assert!(a.is_finite() && b.is_finite());
            assert!((s.distance(a) - 1.).abs() < 1e-4);
            assert!((a.distance(b) - 1.).abs() < 1e-4);
        }
    }

    #[test]
    fn correction_is_invariant_under_rigid_placement() {
        let shape = Ellipsoid {
            center: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            radii: Vec3::new(0.15, 0.2, 0.12),
        };
        let s = Vec3::new(0.22, 0.3, 0.);
        let e = Vec3::new(0.32, 0.05, 0.);
        let w = Vec3::new(0.03, 0., 0.03);
        let rotation = Quat::from_euler(EulerRot::XYZ, 0.4, 1.2, -0.3);
        let shift = Vec3::new(2., -1., 3.);
        let (a, b) = solve(s, e, w, &[], &[shape], 1., (0.012, 0.012));
        let placed = Ellipsoid {
            center: shift,
            rotation,
            ..shape
        };
        let (c, d) = solve(
            rotation * s + shift,
            rotation * e + shift,
            rotation * w + shift,
            &[],
            &[placed],
            1.,
            (0.012, 0.012),
        );
        assert!(
            c.distance(rotation * a + shift) < 1e-4,
            "elbow difference {}",
            c.distance(rotation * a + shift)
        );
        assert!(
            d.distance(rotation * b + shift) < 1e-4,
            "wrist difference {}",
            d.distance(rotation * b + shift)
        );
    }
}
