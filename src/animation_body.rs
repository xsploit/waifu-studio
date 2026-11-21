//! CPU skinning samples for the active avatar's torso, skirt and clothed arms.
//! Requires retained CPU mesh attributes; deliberately has no skeleton body proxy.
//! Morph targets and cloth/spring simulation are not applied to these samples.
use super::{
    PoseBone,
    animation_clearance::{model_transform, torso_top},
};
use bevy::{
    math::Affine3A,
    mesh::{
        VertexAttributeValues,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName as Bone;
use std::collections::{HashMap, HashSet};

const MAX_BODY_POINTS: usize = 20_000;
// Per side and segment: a dense left sleeve cannot crowd out the right.
const MAX_RADIUS_SAMPLES: usize = 2_048;
const MAX_HAND_CANDIDATES: usize = 4_096;

#[derive(Clone, Copy)]
struct Influence {
    entity: Entity,
    /// Mesh position already transformed by this joint's inverse bind matrix.
    position: Vec3,
    weight: f32,
}

#[derive(Clone, Copy)]
struct SkinPoint([Option<Influence>; 4]);

// Include actual joint entities, weights, and inverse-bind position bits, rather
// than mesh handles or palette indices: material primitives can duplicate arrays
// under different handles, and different skins can reuse the same palette index.
#[derive(Hash, PartialEq, Eq)]
struct VertexKey {
    position: [u32; 3],
    influences: [Option<(u64, [u32; 3], u32)>; 4],
}

impl SkinPoint {
    fn key(&self, position: Vec3) -> VertexKey {
        let mut influences = self.0.map(|influence| {
            influence.map(|influence| {
                (
                    influence.entity.to_bits(),
                    influence.position.to_array().map(f32::to_bits),
                    influence.weight.to_bits(),
                )
            })
        });
        influences.sort_unstable();
        VertexKey {
            position: position.to_array().map(f32::to_bits),
            influences,
        }
    }
}

#[derive(Default)]
pub(super) struct MeshBodyCache {
    root: Option<Entity>,
    points: Vec<SkinPoint>,
    hands: [Vec<SkinPoint>; 2],
    radii: (f32, f32),
}

impl MeshBodyCache {
    /// Capture once points exist; an empty capture is retried on the next call.
    /// Reset the cache when the same avatar's mesh/skin assets are replaced.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn capture(
        &mut self,
        root: Entity,
        bones: &[PoseBone],
        meshes: &Assets<Mesh>,
        bindposes: &Assets<SkinnedMeshInverseBindposes>,
        mesh_entities: &Query<(Entity, &Mesh3d, &SkinnedMesh)>,
        parents: &Query<&ChildOf>,
        transforms: &Query<&mut Transform>,
    ) -> bool {
        if self.root != Some(root) {
            *self = Self {
                root: Some(root),
                ..Self::default()
            };
        }
        if !self.points.is_empty() {
            return true;
        }
        // Even supplied humanoid mappings must belong to this avatar.
        let mapped: HashMap<_, _> = bones
            .iter()
            .filter(|bone| descends_from(bone.entity, root, parents))
            .map(|bone| (bone.entity, bone))
            .collect();
        let find = |name| mapped.values().find(|bone| bone.bone == name).copied();
        let Some((hips, (left, right))) =
            find(Bone::Hips).zip(find(Bone::LeftUpperArm).zip(find(Bone::RightUpperArm)))
        else {
            return false;
        };
        let hips = hips.rest_position;
        let top = torso_top(
            find(Bone::Neck).map(|bone| bone.rest_position),
            left.rest_position,
            right.rest_position,
        );
        let width = left.rest_position.distance(right.rest_position);
        let length = hips.distance(top);
        if !hips.is_finite()
            || !top.is_finite()
            || !width.is_finite()
            || width < 1e-5
            || !length.is_finite()
            || length < 1e-5
            || top.y <= hips.y
        {
            return false;
        }
        // Conservative skeletal-width fallback only for missing arm samples.
        // Zero radii before any valid skeleton has been captured are intentional.
        self.radii = (width * 0.15, width * 0.12);
        let arms = [
            (Bone::LeftUpperArm, Bone::LeftLowerArm, Bone::LeftHand),
            (Bone::RightUpperArm, Bone::RightLowerArm, Bone::RightHand),
        ]
        .map(|(upper, lower, hand)| {
            find(upper)
                .zip(find(lower))
                .zip(find(hand))
                .map(|((upper, lower), hand)| {
                    [upper.rest_position, lower.rest_position, hand.rest_position]
                })
        });
        let mut radius_samples = [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]];
        let mut hand_candidates: [Vec<(SkinPoint, Vec3)>; 2] = [Vec::new(), Vec::new()];
        let mut seen = HashSet::new();
        // Lazily resolve only referenced, positively weighted joints. This also
        // bounds the temporary joint cache to the u16 mesh palette address space.
        for (mesh_entity, handle, skin) in mesh_entities.iter() {
            if !descends_from(mesh_entity, root, parents) {
                continue;
            }
            let Some(mesh) = meshes.get(&handle.0) else {
                continue;
            };
            let Some(inverse_bindposes) = bindposes.get(&skin.inverse_bindposes) else {
                continue;
            };
            let Ok(VertexAttributeValues::Float32x3(positions)) =
                mesh.try_attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                continue;
            };
            let Ok(VertexAttributeValues::Uint16x4(joints)) =
                mesh.try_attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
            else {
                continue;
            };
            let Ok(VertexAttributeValues::Float32x4(weights)) =
                mesh.try_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT)
            else {
                continue;
            };
            let Ok(indices) = mesh.try_indices_option() else {
                continue;
            };
            let count = positions.len().min(joints.len()).min(weights.len());
            let vertices: Box<dyn Iterator<Item = usize> + '_> = match indices {
                Some(indices) => Box::new(indices.iter()),
                None => Box::new(0..count),
            };
            let mut resolved = HashMap::new();
            for vertex in vertices {
                if self.points.len() == MAX_BODY_POINTS
                    && radius_samples
                        .iter()
                        .flatten()
                        .all(|samples| samples.len() == MAX_RADIUS_SAMPLES)
                    && hand_candidates
                        .iter()
                        .all(|samples| samples.len() == MAX_HAND_CANDIDATES)
                {
                    break;
                }
                // Indexed primitives often retain complete shared attribute
                // arrays: only referenced vertices with all attributes count.
                if vertex >= count {
                    continue;
                }
                let position = Vec3::from_array(positions[vertex]);
                if !position.is_finite() {
                    continue;
                }
                let mut point = SkinPoint([None; 4]);
                let mut rest_point = Vec3::ZERO;
                let mut torso_weight = 0.;
                let mut arm_weights = [0.; 2];
                let mut hand_weights = [0.; 2];
                let mut valid = true;
                for slot in 0..4 {
                    let weight = weights[vertex][slot];
                    if !weight.is_finite() || weight < 0. {
                        valid = false;
                        break;
                    }
                    if weight == 0. {
                        continue;
                    }
                    let index = joints[vertex][slot] as usize;
                    let joint = resolved.entry(index).or_insert_with(|| {
                        let entity = *skin.joints.get(index)?;
                        if !descends_from(entity, root, parents) {
                            return None;
                        }
                        let inverse_bind = *inverse_bindposes.get(index)?;
                        if !inverse_bind.is_finite() || inverse_bind.row(3) != Vec4::W {
                            return None;
                        }
                        let rest =
                            rest_model_transform(entity, root, &mapped, parents, transforms)?;
                        // Invalid current hierarchies cannot be used by the IK
                        // consumer, even if their imported rest pose was valid.
                        model_transform(entity, root, parents, transforms)?;
                        inverse_bind.is_finite().then_some((
                            entity,
                            inverse_bind,
                            rest,
                            classify(entity, root, &mapped, parents),
                        ))
                    });
                    let Some((entity, inverse_bind, rest, class)) = joint.as_ref() else {
                        valid = false;
                        break;
                    };
                    let local = inverse_bind.transform_point3(position);
                    if !local.is_finite() {
                        valid = false;
                        break;
                    }
                    point.0[slot] = Some(Influence {
                        entity: *entity,
                        position: local,
                        weight,
                    });
                    rest_point += rest.transform_point3(local) * weight;
                    match class {
                        JointClass::Torso => torso_weight += weight,
                        JointClass::LeftArm => arm_weights[0] += weight,
                        JointClass::RightArm => arm_weights[1] += weight,
                        JointClass::LeftHand => hand_weights[0] += weight,
                        JointClass::RightHand => hand_weights[1] += weight,
                        JointClass::Other => {}
                    }
                }
                if !valid || !rest_point.is_finite() {
                    continue;
                }
                let body = self.points.len() < MAX_BODY_POINTS
                    && torso_weight > 0.75
                    && inside_torso(rest_point, hips, top, length, width);
                let mut measurements = [[None; 2]; 2];
                for side in 0..2 {
                    if arm_weights[side] < 0.75 {
                        continue;
                    }
                    if let Some([upper, lower, hand]) = arms[side] {
                        for (limb, (start, end)) in
                            [(upper, lower), (lower, hand)].into_iter().enumerate()
                        {
                            if radius_samples[side][limb].len() < MAX_RADIUS_SAMPLES {
                                measurements[side][limb] = segment_radius(rest_point, start, end);
                            }
                        }
                    }
                }
                let hand = hand_weights.map(|w| w > 0.95);
                if (!body
                    && !hand.iter().any(|&x| x)
                    && measurements.iter().flatten().all(Option::is_none))
                    || !seen.insert(point.key(position))
                {
                    continue;
                }
                if body {
                    self.points.push(point);
                }
                for side in 0..2 {
                    if hand[side] && hand_candidates[side].len() < MAX_HAND_CANDIDATES {
                        hand_candidates[side].push((point, rest_point));
                    }
                }
                for (samples, measurement) in radius_samples
                    .iter_mut()
                    .flatten()
                    .zip(measurements.into_iter().flatten())
                {
                    if let Some(radius) = measurement {
                        samples.push(radius);
                    }
                }
            }
        }
        let [[mut upper, mut lower], [right_upper, right_lower]] = radius_samples;
        upper.extend(right_upper);
        lower.extend(right_lower);
        self.radii = (
            percentile_radius(&mut upper).unwrap_or(self.radii.0),
            percentile_radius(&mut lower).unwrap_or(self.radii.1),
        );
        self.hands = hand_candidates.map(|candidates| {
            let mut selected = Vec::new();
            let mut indices = HashSet::new();
            for x in -1..=1 {
                for y in -1..=1 {
                    for z in -1..=1 {
                        let direction = Vec3::new(x as f32, y as f32, z as f32);
                        if direction == Vec3::ZERO {
                            continue;
                        }
                        if let Some((index, _)) = candidates
                            .iter()
                            .enumerate()
                            .max_by(|a, b| direction.dot(a.1.1).total_cmp(&direction.dot(b.1.1)))
                        {
                            if indices.insert(index) {
                                selected.push(candidates[index].0);
                            }
                        }
                    }
                }
            }
            selected
        });
        if !self.points.is_empty() {
            info!(
                points = self.points.len(),
                upper_radius = self.radii.0,
                forearm_radius = self.radii.1,
                left_hand_points = self.hands[0].len(),
                right_hand_points = self.hands[1].len(),
                "Prepared experimental mesh arm clearance"
            );
        }
        !self.points.is_empty()
    }

    /// Model-space positions skinned from actual current joint affines, with
    /// root placement excluded. Missing/unsupported joint hierarchies fail closed.
    pub(super) fn posed_points(
        &self,
        root: Entity,
        parents: &Query<&ChildOf>,
        transforms: &Query<&mut Transform>,
    ) -> Option<(Vec<Vec3>, [Vec<Vec3>; 2])> {
        if self.root != Some(root) || self.points.is_empty() {
            return None;
        }
        let mut joints = HashMap::new();
        let mut skin = |points: &[SkinPoint]| -> Option<Vec<Vec3>> {
            let mut result = Vec::with_capacity(points.len());
            for point in points {
                let mut position = Vec3::ZERO;
                for influence in point.0.iter().flatten() {
                    let affine = match joints.entry(influence.entity) {
                        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::hash_map::Entry::Vacant(entry) => *entry.insert(
                            model_transform(influence.entity, root, parents, transforms)?,
                        ),
                    };
                    position += affine.transform_point3(influence.position) * influence.weight;
                }
                if !position.is_finite() {
                    return None;
                }
                result.push(position);
            }
            Some(result)
        };
        let body = skin(&self.points)?;
        let left = skin(&self.hands[0])?;
        let right = skin(&self.hands[1])?;
        Some((body, [left, right]))
    }

    /// (Upper arm, forearm) sleeve-aware 95th percentile radii across both sides.
    /// Missing measurements fall back to shoulder span * (0.15, 0.12). Returns
    /// (0, 0) until capture has received valid hips/neck/shoulder rest geometry.
    pub(super) fn arm_radii(&self) -> (f32, f32) {
        self.radii
    }
}

fn descends_from(entity: Entity, root: Entity, parents: &Query<&ChildOf>) -> bool {
    let mut current = entity;
    for _ in 0..128 {
        if current == root {
            return true;
        }
        let Ok(parent) = parents.get(current) else {
            return false;
        };
        current = parent.parent();
    }
    false
}

#[derive(Clone, Copy)]
enum JointClass {
    Torso,
    LeftArm,
    RightArm,
    LeftHand,
    RightHand,
    Other,
}

fn classify(
    entity: Entity,
    root: Entity,
    mapped: &HashMap<Entity, &PoseBone>,
    parents: &Query<&ChildOf>,
) -> JointClass {
    let mut current = entity;
    for _ in 0..128 {
        if let Some(bone) = mapped.get(&current) {
            let class = match bone.bone {
                // Skirt spring chains commonly descend from an upper leg.
                // The spatial bounds below retain only the nearby upper-thigh
                // region, excluding lower legs, shoes and unrelated geometry.
                Bone::Hips
                | Bone::Spine
                | Bone::Chest
                | Bone::UpperChest
                | Bone::LeftUpperLeg
                | Bone::RightUpperLeg => JointClass::Torso,
                Bone::LeftUpperArm | Bone::LeftLowerArm => JointClass::LeftArm,
                Bone::RightUpperArm | Bone::RightLowerArm => JointClass::RightArm,
                Bone::LeftHand => JointClass::LeftHand,
                Bone::RightHand => JointClass::RightHand,
                _ => {
                    if current == root {
                        break;
                    }
                    let Ok(parent) = parents.get(current) else {
                        break;
                    };
                    current = parent.parent();
                    continue;
                }
            };
            return class;
        }
        if current == root {
            break;
        }
        let Ok(parent) = parents.get(current) else {
            break;
        };
        current = parent.parent();
    }
    JointClass::Other
}

fn rest_model_transform(
    entity: Entity,
    root: Entity,
    mapped: &HashMap<Entity, &PoseBone>,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
) -> Option<Affine3A> {
    let mut current = entity;
    let mut result = Affine3A::IDENTITY;
    for _ in 0..128 {
        if current == root {
            return result.is_finite().then_some(result);
        }
        if let Some(bone) = mapped.get(&current) {
            if let Some(model_to_parent) = bone.model_to_parent {
                // Imported parent affine retains scale and excludes application
                // placement, unlike rebuilding from a rest quaternion alone.
                result = model_to_parent.inverse() * bone.rest.compute_affine() * result;
                return result.is_finite().then_some(result);
            }
            result = bone.rest.compute_affine() * result;
        } else {
            // The API has rest snapshots only for mapped humanoid bones.
            // Unmapped helper locals are sampled at capture time.
            result = transforms.get(current).ok()?.compute_affine() * result;
        }
        current = parents.get(current).ok()?.parent();
    }
    None
}

fn inside_torso(point: Vec3, hips: Vec3, neck: Vec3, length: f32, width: f32) -> bool {
    let fraction = ((point.y - hips.y) / (neck.y - hips.y)).clamp(0., 1.);
    let center = hips.lerp(neck, fraction);
    let horizontal = Vec2::new(point.x - center.x, point.z - center.z);
    // Hands hang below the hips. The former 15% cutoff omitted the skirt and
    // lower jacket completely. Allow the hip-bound clothing envelope down to
    // the upper thighs, widening towards the hem rather than cropping it to
    // the shoulder joint span. Leg/foot vertices are excluded by joint class.
    let below = ((hips.y - point.y) / length).clamp(0., 0.7);
    let width_factor = 1.1 + (below * 4.).min(1.0);
    point.y >= hips.y - length * 0.7
        && point.y <= neck.y
        && horizontal.length_squared() <= (width * width_factor).powi(2)
}

fn segment_radius(point: Vec3, start: Vec3, end: Vec3) -> Option<f32> {
    let axis = end - start;
    let length_squared = axis.length_squared();
    if !length_squared.is_finite() || length_squared < 1e-10 {
        return None;
    }
    let fraction = (point - start).dot(axis) / length_squared;
    if !(0.2..=0.8).contains(&fraction) {
        return None;
    }
    let radius = point.distance(start + axis * fraction);
    (radius.is_finite() && radius > 0.).then_some(radius)
}

fn percentile_radius(samples: &mut [f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable_by(f32::total_cmp);
    // Nearest-rank percentile, including the sole measurement in a small mesh.
    Some(samples[(samples.len() * 95).div_ceil(100) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skirt_helpers_and_finger_bones_follow_their_mapped_ancestors() {
        use bevy::ecs::system::SystemState;
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let leg = world.spawn(ChildOf(root)).id();
        let skirt = world.spawn(ChildOf(leg)).id();
        let hand = world.spawn(ChildOf(root)).id();
        let finger = world.spawn(ChildOf(hand)).id();
        let bones: Vec<_> = [
            (leg, Bone::LeftUpperLeg),
            (hand, Bone::LeftHand),
            (finger, Bone::LeftIndexProximal),
        ]
        .into_iter()
        .map(|(entity, bone)| PoseBone {
            entity,
            bone,
            rest: Transform::IDENTITY,
            idle: Transform::IDENTITY,
            rest_rotation: Quat::IDENTITY,
            rest_position: Vec3::ZERO,
            model_to_parent: None,
            joint: None,
            depth: 0,
        })
        .collect();
        let mapped = bones.iter().map(|bone| (bone.entity, bone)).collect();
        let mut system: SystemState<Query<&ChildOf>> = SystemState::new(&mut world);
        let parents = system.get(&world).unwrap();
        assert!(matches!(
            classify(skirt, root, &mapped, &parents),
            JointClass::Torso
        ));
        assert!(matches!(
            classify(finger, root, &mapped, &parents),
            JointClass::LeftHand
        ));
    }

    #[test]
    fn sleeve_measurements_exclude_segment_ends_and_use_percentile() {
        assert_eq!(
            segment_radius(Vec3::new(0.1, 0.2, 0.), Vec3::ZERO, Vec3::X),
            None
        );
        assert_eq!(
            segment_radius(Vec3::new(0.9, 0.2, 0.), Vec3::ZERO, Vec3::X),
            None
        );
        assert_eq!(
            segment_radius(Vec3::new(0.5, 0.2, 0.), Vec3::ZERO, Vec3::X),
            Some(0.2)
        );
        let mut radii = vec![0.04; 95];
        radii.extend([0.5; 5]);
        assert_eq!(percentile_radius(&mut radii), Some(0.04));
        assert_eq!(percentile_radius(&mut []), None);
    }

    #[test]
    fn rest_bounds_include_hip_bound_skirts_without_foot_or_wide_ornament_geometry() {
        let hips = Vec3::Y;
        let neck = Vec3::Y * 1.5;
        assert!(inside_torso(Vec3::new(0.1, 1.2, 0.1), hips, neck, 0.5, 0.4));
        assert!(inside_torso(Vec3::new(0.25, 0.8, 0.), hips, neck, 0.5, 0.4));
        assert!(!inside_torso(Vec3::new(0., 0.5, 0.), hips, neck, 0.5, 0.4));
        assert!(!inside_torso(Vec3::new(0.5, 1.2, 0.), hips, neck, 0.5, 0.4));
        assert!(!inside_torso(Vec3::new(0., 1.6, 0.), hips, neck, 0.5, 0.4));
    }

    #[test]
    fn material_duplicate_keys_preserve_influences_and_ignore_slot_order() {
        let mut world = World::new();
        let a = Influence {
            entity: world.spawn_empty().id(),
            position: Vec3::Y,
            weight: 0.8,
        };
        let b = Influence {
            entity: world.spawn_empty().id(),
            position: Vec3::X,
            weight: 0.2,
        };
        let original = SkinPoint([Some(a), Some(b), None, None]);
        let reordered = SkinPoint([Some(b), None, Some(a), None]);
        assert!(original.key(Vec3::Y) == reordered.key(Vec3::Y));
        let changed = SkinPoint([Some(Influence { weight: 0.9, ..a }), Some(b), None, None]);
        assert!(original.key(Vec3::Y) != changed.key(Vec3::Y));
        assert!(original.key(Vec3::Y) != original.key(Vec3::X));
    }
}
