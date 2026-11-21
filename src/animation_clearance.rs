//! Experimental post-retarget arm clearance, in avatar-model space.
//! Torso volume comes from skinned mesh support points; arm capsules include
//! the upper arms. The proximal fifth allows attachment at the shoulder.
//! Runs after crossfading so both clips and their blended poses are considered.
use super::PoseBone;
use bevy::{math::Affine3A, prelude::*};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName as Bone;

const SHOULDER_ATTACHMENT: f32 = 0.2;

/// VRM 1.0 rigs can omit the neck. Use the center of the shoulder line as
/// the upper torso landmark on those rigs.
pub(super) fn torso_top(neck: Option<Vec3>, left_upper_arm: Vec3, right_upper_arm: Vec3) -> Vec3 {
    neck.unwrap_or_else(|| left_upper_arm.midpoint(right_upper_arm))
}

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
    /// Overlapping height bands retain a narrow waist instead of filling the
    /// whole jacket/skirt convex envelope. Each band encloses actual mesh points.
    pub(super) fn sections(points: &[Vec3]) -> Vec<Self> {
        let lo = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let hi = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        let step = (hi - lo) / 6.;
        if !step.is_finite() || step < 1e-5 {
            return Vec::new();
        }
        (0..6)
            .filter_map(|i| {
                let lower = lo + i as f32 * step - step * 0.35;
                let upper = lower + step * 1.7;
                Self::from_points(
                    &points
                        .iter()
                        .copied()
                        .filter(|p| p.y >= lower && p.y <= upper)
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    }
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
        let mut best = (f32::INFINITY, Vec3::ZERO);
        for &(normal, offset) in &self.planes {
            let depth = offset + margin - normal.dot(point);
            if depth <= 0. {
                return Vec3::ZERO;
            }
            if depth < best.0 {
                best = (depth, normal);
            }
        }
        best.1 * best.0
    }
    fn segment_contact(&self, start: Vec3, end: Vec3, margin: f32, begin: f32) -> (Vec3, f32) {
        // Clip the whole axis against the inflated convex volume first. Most
        // band/capsule pairs do not overlap and can return after a few planes.
        let mut lo = begin;
        let mut hi: f32 = 1.;
        let delta = end - start;
        for &(normal, offset) in &self.planes {
            let slack = offset + margin - normal.dot(start);
            let slope = -normal.dot(delta);
            if slope.abs() < 1e-10 {
                if slack <= 0. {
                    return (Vec3::ZERO, begin);
                }
            } else if slope > 0. {
                lo = lo.max(-slack / slope);
            } else {
                hi = hi.min(-slack / slope);
            }
            if lo >= hi {
                return (Vec3::ZERO, begin);
            }
        }
        // The minimum plane slack is concave along the segment. Search inside
        // the actual overlap rather than missing intersections between samples.
        for _ in 0..8 {
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
        for t in [lo, (lo + hi) * 0.5, hi] {
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

fn limit_correction(
    shoulder: Vec3,
    elbow: Vec3,
    wrist: Vec3,
    previous: (Vec3, Vec3),
    best: (Vec3, Vec3),
    max_step: f32,
) -> (Vec3, Vec3) {
    // Reproject onto this frame's limb lengths. This limits guard-induced
    // motion, not source animation, and does not guarantee swept clearance.
    let base = two_bone(shoulder, elbow, wrist, previous.1, previous.0);
    let distance = |pose: (Vec3, Vec3)| pose.0.distance(base.0).max(pose.1.distance(base.1));
    if distance(best) <= max_step {
        return best;
    }
    let (mut low, mut high) = (0., 1.);
    let mut limited = base;
    for _ in 0..18 {
        let t = (low + high) * 0.5;
        let pose = two_bone(
            shoulder,
            elbow,
            wrist,
            base.1.lerp(best.1, t),
            base.0.lerp(best.0, t),
        );
        if distance(pose) <= max_step {
            low = t;
            limited = pose;
        } else {
            high = t;
        }
    }
    limited
}

fn push_all(mut point: Vec3, shapes: &[impl BodyShape], margin: f32) -> Vec3 {
    let original = point;
    for shape in shapes {
        point += shape.push(point, margin);
    }
    point - original
}

fn upper_contact(
    shape: &impl BodyShape,
    shoulder: Vec3,
    elbow: Vec3,
    radius: f32,
    attachment: f32,
) -> (Vec3, f32) {
    if attachment <= SHOULDER_ATTACHMENT {
        return shape.segment_contact(shoulder, elbow, radius, attachment);
    }
    // The sleeve emerges gradually from its armhole. Taper the protection
    // near the attachment instead of treating the joint as a full capsule.
    let mut best = (Vec3::ZERO, attachment);
    for i in 0..4 {
        let a = attachment + (1. - attachment) * (i as f32 / 4.);
        let b = attachment + (1. - attachment) * ((i + 1) as f32 / 4.);
        let margin = radius * ((a - attachment) / 0.20).min(1.);
        let (push, t) =
            shape.segment_contact(shoulder.lerp(elbow, a), shoulder.lerp(elbow, b), margin, 0.);
        if push.length_squared() > best.0.length_squared() {
            best = (push, a + (b - a) * t);
        }
    }
    best
}

#[cfg(test)]
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
    solve_guided(
        shoulder,
        elbow,
        wrist,
        fingers,
        shapes,
        size,
        radii,
        None,
        None,
        None,
        f32::INFINITY,
    )
}

#[allow(clippy::too_many_arguments)]
fn solve_guided(
    shoulder: Vec3,
    elbow: Vec3,
    wrist: Vec3,
    fingers: &[Vec3],
    shapes: &[impl BodyShape],
    size: f32,
    radii: (f32, f32),
    axes: Option<(Vec3, Vec3)>,
    previous: Option<(Vec3, Vec3)>,
    hand_sphere: Option<(Vec3, f32)>,
    max_step: f32,
) -> (Vec3, Vec3) {
    let mut e = elbow;
    let mut w = wrist;
    // A shoulder joint inside the clothed torso is an attachment, not a loose
    // capsule that can be expelled. Protect the distal arm; do not lift the arm
    // overhead to clear its own armhole. Outside shoulders retain full coverage.
    let attachment = if shapes
        .iter()
        .any(|s| s.push(shoulder, radii.0).length_squared() > 1e-10)
    {
        0.35
    } else {
        SHOULDER_ATTACHMENT
    };
    let score = |e: Vec3, w: Vec3| {
        let mut score = 0.;
        for shape in shapes {
            score += shape.push(e, radii.1).length_squared();
            score += shape.push(w, size * 0.020).length_squared();
            if let Some((center, radius)) = hand_sphere {
                score += shape.push(w + center, radius).length_squared();
            }
            for offset in fingers {
                score += shape.push(w + *offset, size * 0.005).length_squared();
            }
            score += upper_contact(shape, shoulder, e, radii.0, attachment)
                .0
                .length_squared();
            score += shape.segment_contact(e, w, radii.1, 0.).0.length_squared();
        }
        score
    };
    let mut best = (e, w);
    let raw_score = score;
    let score = |e: Vec3, w: Vec3| {
        let penetration = (raw_score(e, w).sqrt() - size * 0.00025).max(0.);
        penetration.powi(2) * 1_000_000.
            + e.distance_squared(elbow)
            + w.distance_squared(wrist) * 4.
            + axes.map_or(0., |(_, up)| {
                let raised = (e - elbow).dot(up).max(0.);
                raised * raised * 16.
            })
            + previous.map_or(0., |(a, b)| {
                e.distance_squared(a) + w.distance_squared(b) * 4.
            })
    };
    let mut best_score = score(e, w);
    if raw_score(e, w) < 1e-12
        && previous.is_none_or(|(a, b)| a.distance_squared(e) + b.distance_squared(w) < 1e-12)
    {
        return best;
    }
    let axis = (wrist - shoulder).try_normalize().unwrap_or(Vec3::Y);
    let mut wrist_seeds = vec![wrist];
    for shape in shapes {
        if previous.is_some() {
            wrist_seeds.push(wrist + shape.push(wrist, size * 0.020));
        } else {
            wrist_seeds.extend(shape.exit_candidates(wrist, size * 0.020));
        }
    }
    if let Some((outward, up)) = axes {
        let reach = shoulder.distance(elbow) + elbow.distance(wrist);
        for amount in if previous.is_some() {
            &[0.08][..]
        } else {
            &[0.05, 0.1, 0.2][..]
        } {
            wrist_seeds.push(wrist + outward * size * *amount);
        }
        // Preserve a relaxed, nearly extended arm as a candidate. Without
        // this seed the solver can favor an elbows-out, hands-on-hips pose.
        wrist_seeds.push(shoulder + outward * reach * 0.30 - up * reach * 0.94);
        wrist_seeds.push(shoulder + outward * reach * 0.38 - up * reach * 0.90);
        wrist_seeds.push(shoulder + outward * reach * 0.45 - up * reach * 0.85);
        wrist_seeds.push(shoulder + outward * reach * 0.52 - up * reach * 0.85);
        wrist_seeds.push(shoulder + outward * reach * 0.55 - up * reach * 0.83);
        wrist_seeds.push(shoulder + outward * reach * 0.60 - up * reach * 0.80);
        wrist_seeds.push(shoulder + outward * reach * 0.7 - up * reach * 0.65);
    }
    if let Some((_, w)) = previous {
        wrist_seeds.insert(0, w);
    }
    wrist_seeds.dedup_by(|a, b| a.distance_squared(*b) < 1e-8);
    for wrist_seed in wrist_seeds {
        // Search elbow bend planes before moving the hand far from its gesture.
        let angles: &[f32] = if previous.is_some() {
            &[0., 0.5, -0.5]
        } else {
            &[0., 0.5, -0.5, 1., -1., 1.5, -1.5, std::f32::consts::PI]
        };
        for &angle in angles {
            (e, w) = two_bone(
                shoulder,
                elbow,
                wrist,
                wrist_seed,
                if angle == 0. {
                    previous.map_or(elbow, |p| p.0)
                } else {
                    shoulder + Quat::from_axis_angle(axis, angle) * (elbow - shoulder)
                },
            );
            for _ in 0..if previous.is_some() { 8 } else { 24 } {
                let elbow_push = push_all(e, shapes, radii.1);
                let mut pole = e + elbow_push;
                // A pole alone cannot move an elbow along its current bend plane.
                // Let the wrist move too when necessary to make elbow clearance feasible.
                let mut target = w + push_all(w, shapes, size * 0.020) + elbow_push;
                if let Some((center, radius)) = hand_sphere {
                    target += push_all(target + center, shapes, radius);
                }
                // Preserve the original hand's model orientation while protecting its
                // palm/finger envelope as well as the wrist joint itself.
                for offset in fingers {
                    target += push_all(target + *offset, shapes, size * 0.005);
                }
                for shape in shapes {
                    let (correction, t) = upper_contact(shape, shoulder, pole, radii.0, attachment);
                    pole += correction / t.max(attachment);
                    target += correction * 0.5 / t.max(attachment);
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
    previous.map_or(best, |prior| {
        limit_correction(shoulder, elbow, wrist, prior, best, max_step)
    })
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
    shapes: &[Hull],
    radii: (f32, f32),
    hands: &[Vec<Vec3>; 2],
    history: &mut [Option<(Vec3, Vec3)>; 2],
    delta_seconds: f32,
) {
    let find = |name| bones.iter().find(|bone| bone.bone == name);
    let Some((hips, (left, right))) =
        find(Bone::Hips).zip(find(Bone::LeftUpperArm).zip(find(Bone::RightUpperArm)))
    else {
        return;
    };
    let neck = find(Bone::Neck);
    let rest_top = torso_top(
        neck.map(|bone| bone.rest_position),
        left.rest_position,
        right.rest_position,
    );
    let size = hips.rest_position.distance(rest_top) * 4.;
    if !size.is_finite() || size < 1e-5 {
        return;
    }
    let current = |bone: &PoseBone| {
        model_transform(bone.entity, root, parents, transforms).map(|m| Vec3::from(m.translation))
    };
    let Some((hips_position, (left_position, right_position))) =
        current(hips).zip(current(left).zip(current(right)))
    else {
        return;
    };
    let neck_position = match neck {
        Some(neck) => {
            let Some(position) = current(neck) else {
                return;
            };
            Some(position)
        }
        None => None,
    };
    let top = torso_top(neck_position, left_position, right_position);
    let Some(up) = (top - hips_position).try_normalize() else {
        return;
    };
    let center = left_position.midpoint(right_position);
    for (side, (upper, lower, hand, tips)) in [
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
    ]
    .into_iter()
    .enumerate()
    {
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
        let mut fingers: Vec<_> = tips
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
        fingers.extend(hands[side].iter().map(|point| *point - wrist));
        let Some(outward) = (shoulder - center).try_normalize() else {
            continue;
        };
        // Persist correction offsets, not absolute joint positions. Ordinary
        // authored motion follows immediately; only the guard is smoothed.
        let previous = history[side].map(|(a, b)| (elbow + a, wrist + b));
        let (new_elbow, new_wrist) = solve_guided(
            shoulder,
            elbow,
            wrist,
            &fingers,
            shapes,
            size,
            radii,
            Some((outward, up)),
            previous,
            None,
            delta_seconds.clamp(0., 0.1) * 1.2,
        );
        history[side] = Some((new_elbow - elbow, new_wrist - wrist));
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
            history[side] = None;
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

    #[test]
    fn correction_limit_preserves_arm_lengths_and_bounds_guard_step() {
        let shoulder = Vec3::ZERO;
        let elbow = Vec3::new(0.25, -0.20, 0.0);
        let wrist = Vec3::new(0.45, -0.35, 0.0);
        let previous = two_bone(shoulder, elbow, wrist, wrist, elbow);
        let desired = two_bone(
            shoulder,
            elbow,
            wrist,
            Vec3::new(0.1, -0.35, 0.3),
            Vec3::new(0.1, -0.1, 0.3),
        );
        let limited = limit_correction(shoulder, elbow, wrist, previous, desired, 0.02);
        assert!(limited.0.distance(previous.0) <= 0.02001);
        assert!(limited.1.distance(previous.1) <= 0.02001);
        assert!((shoulder.distance(limited.0) - shoulder.distance(elbow)).abs() < 1e-5);
        assert!((limited.0.distance(limited.1) - elbow.distance(wrist)).abs() < 1e-5);
    }

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
        assert_ecs_ik(true);
    }

    #[test]
    fn ecs_ik_runs_without_an_optional_neck_bone() {
        assert_ecs_ik(false);
    }

    fn assert_ecs_ik(has_neck: bool) {
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
        .filter(|&(_, bone, _)| has_neck || bone != Bone::Neck)
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
            &[hull],
            (0.012, 0.012),
            &[Vec::new(), Vec::new()],
            &mut [None; 2],
            1. / 60.,
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
