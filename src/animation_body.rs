//! CPU skinning samples for the active avatar's torso and clothed arm radii.
//! Requires retained CPU mesh attributes; deliberately has no skeleton body proxy.
//! Morph targets and cloth/spring simulation are not applied to these samples.
use super::{PoseBone, animation_clearance::model_transform};
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
// Soft clearance fits its proxy per frame, so its sample set stays small.
const MAX_STRICT_POINTS: usize = 6_000;

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
    /// Vertices skinned directly to humanoid torso/upper-leg bones. Garment and
    /// hair spring chains are non-humanoid joints, so they never count as body.
    strict: Vec<SkinPoint>,
    radii: (f32, f32),
    /// Loaded skinned meshes at the last scan; a consumer whose set is still empty
    /// rescans only when more mesh data has arrived.
    scanned_meshes: usize,
}

impl MeshBodyCache {
    /// Capture until the requesting consumer's set exists: `strict` for soft
    /// clearance, `points` for the experimental guard. A consumer whose set is
    /// empty rescans whenever more skinned mesh data has loaded since the last scan.
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
        strict_needed: bool,
    ) -> bool {
        if self.root != Some(root) {
            *self = Self {
                root: Some(root),
                ..Self::default()
            };
        }
        let ready = |cache: &Self| {
            if strict_needed {
                !cache.strict.is_empty()
            } else {
                !cache.points.is_empty()
            }
        };
        if ready(self) {
            return true;
        }
        let loaded = mesh_entities
            .iter()
            .filter(|(entity, mesh, skin)| {
                descends_from(*entity, root, parents)
                    && meshes.contains(&mesh.0)
                    && bindposes.contains(&skin.inverse_bindposes)
            })
            .count();
        if loaded == 0 || loaded == self.scanned_meshes {
            return false;
        }
        self.scanned_meshes = loaded;
        self.points.clear();
        self.strict.clear();
        // Even supplied humanoid mappings must belong to this avatar.
        let mapped: HashMap<_, _> = bones
            .iter()
            .filter(|bone| descends_from(bone.entity, root, parents))
            .map(|bone| (bone.entity, bone))
            .collect();
        let find = |name| mapped.values().find(|bone| bone.bone == name).copied();
        let Some((hips, neck, left, right)) = find(Bone::Hips)
            .zip(find(Bone::Neck))
            .zip(find(Bone::LeftUpperArm))
            .zip(find(Bone::RightUpperArm))
            .map(|(((hips, neck), left), right)| (hips, neck, left, right))
        else {
            return false;
        };
        let hips = hips.rest_position;
        let neck = neck.rest_position;
        let width = left.rest_position.distance(right.rest_position);
        let length = hips.distance(neck);
        if !hips.is_finite()
            || !neck.is_finite()
            || !width.is_finite()
            || width < 1e-5
            || !length.is_finite()
            || length < 1e-5
            || neck.y <= hips.y
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
        let legs: Vec<(Vec3, Vec3)> = [
            (Bone::LeftUpperLeg, Bone::LeftLowerLeg),
            (Bone::RightUpperLeg, Bone::RightLowerLeg),
        ]
        .into_iter()
        .filter_map(|(upper, lower)| {
            find(upper)
                .zip(find(lower))
                .map(|(a, b)| (a.rest_position, b.rest_position))
        })
        .collect();
        let mut radius_samples = [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]];
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
                    && self.strict.len() == MAX_STRICT_POINTS
                    && radius_samples
                        .iter()
                        .flatten()
                        .all(|samples| samples.len() == MAX_RADIUS_SAMPLES)
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
                let mut direct_weight = 0.;
                let mut direct_leg_weight = 0.;
                let mut arm_weights = [0.; 2];
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
                        // Some(is_upper_leg) for joints that ARE humanoid torso/leg bones.
                        let direct = mapped.get(&entity).and_then(|bone| match bone.bone {
                            Bone::Hips | Bone::Spine | Bone::Chest | Bone::UpperChest => {
                                Some(false)
                            }
                            Bone::LeftUpperLeg | Bone::RightUpperLeg => Some(true),
                            _ => None,
                        });
                        inverse_bind.is_finite().then_some((
                            entity,
                            inverse_bind,
                            rest,
                            classify(entity, root, &mapped, parents),
                            direct,
                        ))
                    });
                    let Some((entity, inverse_bind, rest, class, direct)) = joint.as_ref() else {
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
                    if let Some(leg) = direct {
                        direct_weight += weight;
                        if *leg {
                            direct_leg_weight += weight;
                        }
                    }
                    match class {
                        JointClass::Torso => torso_weight += weight,
                        JointClass::LeftArm => arm_weights[0] += weight,
                        JointClass::RightArm => arm_weights[1] += weight,
                        JointClass::Other => {}
                    }
                }
                if !valid || !rest_point.is_finite() {
                    continue;
                }
                let body = self.points.len() < MAX_BODY_POINTS
                    && torso_weight > 0.75
                    && inside_torso(rest_point, hips, neck, length, width);
                // Upper thighs matter (hanging hands), knees and below do not.
                let strict = self.strict.len() < MAX_STRICT_POINTS
                    && strict_candidate(
                        rest_point,
                        direct_weight,
                        direct_leg_weight,
                        (hips, neck, length),
                        &legs,
                        (width * 0.75, width * 0.35),
                    );
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
                if (!body && !strict && measurements.iter().flatten().all(Option::is_none))
                    || !seen.insert(point.key(position))
                {
                    continue;
                }
                if body {
                    self.points.push(point);
                }
                if strict {
                    self.strict.push(point);
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
        if !self.points.is_empty() {
            info!(
                points = self.points.len(),
                upper_radius = self.radii.0,
                forearm_radius = self.radii.1,
                "Prepared experimental mesh arm clearance"
            );
        }
        ready(self)
    }

    /// Model-space positions skinned from actual current joint affines, with
    /// root placement excluded. Missing/unsupported joint hierarchies fail closed.
    pub(super) fn posed_points(
        &self,
        root: Entity,
        parents: &Query<&ChildOf>,
        transforms: &Query<&mut Transform>,
    ) -> Option<Vec<Vec3>> {
        if self.root != Some(root) || self.points.is_empty() {
            return None;
        }
        skin_points(&self.points, root, parents, transforms)
    }

    /// Posed strict body points (see `strict`), or None when none were captured.
    pub(super) fn posed_strict(
        &self,
        root: Entity,
        parents: &Query<&ChildOf>,
        transforms: &Query<&mut Transform>,
    ) -> Option<Vec<Vec3>> {
        if self.root != Some(root) || self.strict.is_empty() {
            return None;
        }
        skin_points(&self.strict, root, parents, transforms)
    }

    /// (Upper arm, forearm) sleeve-aware 95th percentile radii across both sides.
    /// Missing measurements fall back to shoulder span * (0.15, 0.12). Returns
    /// (0, 0) until capture has received valid hips/neck/shoulder rest geometry.
    pub(super) fn arm_radii(&self) -> (f32, f32) {
        self.radii
    }
}


/// Model-space positions skinned from current joint affines, root placement excluded.
fn skin_points(
    points: &[SkinPoint],
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
) -> Option<Vec<Vec3>> {
    let mut joints = HashMap::new();
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
            return match bone.bone {
                Bone::Hips | Bone::Spine | Bone::Chest | Bone::UpperChest => JointClass::Torso,
                Bone::LeftUpperArm | Bone::LeftLowerArm => JointClass::LeftArm,
                Bone::RightUpperArm | Bone::RightLowerArm => JointClass::RightArm,
                _ => JointClass::Other,
            };
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
    point.y >= hips.y - length * 0.15
        && point.y <= neck.y
        && horizontal.length_squared() <= (width * 0.8).powi(2)
}


/// Strict body membership for a rest-pose vertex with `direct` weight on humanoid
/// torso/leg bones (`leg` of it on upper legs).
///
/// Guarantee: arms are kept outside the body and outside clothing that is skinned
/// straight to the torso and lies close to it (a fitted jacket or waistband is a
/// visible surface the arm should not pass through). Flared or loose parts are
/// excluded: above the hips, anything farther than `torso_limit` from the spine axis
/// (wide hems, bows, capes); below the hip joint, anything not mostly on an upper leg
/// and within `thigh_limit` of its bone (a skirt or coat tail skinned to Hips).
fn strict_candidate(
    rest: Vec3,
    direct: f32,
    leg: f32,
    (hips, neck, length): (Vec3, Vec3, f32),
    legs: &[(Vec3, Vec3)],
    (torso_limit, thigh_limit): (f32, f32),
) -> bool {
    if direct <= 0.9 || rest.y < hips.y - length * 0.7 || rest.y > neck.y {
        return false;
    }
    if rest.y >= hips.y {
        return segment_distance(rest, hips, neck) <= torso_limit;
    }
    leg >= direct * 0.5
        && legs
            .iter()
            .any(|(start, end)| segment_distance(rest, *start, *end) <= thigh_limit)
}

fn segment_distance(point: Vec3, start: Vec3, end: Vec3) -> f32 {
    let axis = end - start;
    let t = if axis.length_squared() > 1e-10 {
        ((point - start).dot(axis) / axis.length_squared()).clamp(0., 1.)
    } else {
        0.
    };
    point.distance(start + axis * t)
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
    #[test]
    fn soft_clearance_strict_points_exclude_hip_weighted_garments() {
        let hips = Vec3::new(0., 1.0, 0.);
        let neck = Vec3::new(0., 1.5, 0.);
        let legs = [
            (Vec3::new(0.09, 0.95, 0.), Vec3::new(0.09, 0.55, 0.)),
            (Vec3::new(-0.09, 0.95, 0.), Vec3::new(-0.09, 0.55, 0.)),
        ];
        let body = (hips, neck, 0.5);
        let limits = (0.26, 0.12);
        // Chest skin (or a fitted jacket) on Chest: body.
        assert!(strict_candidate(Vec3::new(0.12, 1.3, 0.05), 1.0, 0., body, &legs, limits));
        // A wide jacket hem or bow skinned to Spine, far from the spine axis: not body.
        assert!(!strict_candidate(Vec3::new(0.30, 1.05, 0.), 1.0, 0., body, &legs, limits));
        // Thigh skin on an upper leg, close to its bone: body.
        assert!(strict_candidate(Vec3::new(0.16, 0.8, 0.02), 1.0, 1.0, body, &legs, limits));
        // Skirt hem skinned straight to Hips below the pelvis: not body.
        assert!(!strict_candidate(Vec3::new(0.25, 0.8, 0.), 1.0, 0., body, &legs, limits));
        // Flared skirt skinned to an upper leg but far from the thigh bone: not body.
        assert!(!strict_candidate(Vec3::new(0.32, 0.75, 0.), 1.0, 1.0, body, &legs, limits));
        // Spring-bone garments (no direct humanoid weight): never body.
        assert!(!strict_candidate(Vec3::new(0.1, 1.2, 0.), 0.2, 0., body, &legs, limits));
    }

    use super::*;

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
    fn rest_bounds_exclude_skirts_and_wide_ornaments() {
        let hips = Vec3::Y;
        let neck = Vec3::Y * 1.5;
        assert!(inside_torso(Vec3::new(0.1, 1.2, 0.1), hips, neck, 0.5, 0.4));
        assert!(!inside_torso(Vec3::new(0., 0.8, 0.), hips, neck, 0.5, 0.4));
        assert!(!inside_torso(Vec3::new(0.4, 1.2, 0.), hips, neck, 0.5, 0.4));
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
