//! Builds an [`Avatar`] from the VRM 0.0 extension.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use serde_vrm::{
    vrm0::{BoneGroup, Vrm},
    vrm1::vrmc_vrm::{first_person::FirstPersonType, humanoid::HumanBoneName},
};

use super::avatar::{
    Avatar, ColliderDesc, Facing, JointDesc, SpringDesc, Tail, resolve_first_person,
};
use crate::spring_bones::{MAX_COLLIDERS, SpringColliderShape, SpringSettings};

impl Avatar {
    pub(crate) fn from_vrm0(gltf: &gltf::Gltf, vrm: &Vrm) -> Self {
        let mut human_bones = Vec::new();
        if let Some(bones) = vrm.humanoid.as_ref().and_then(|h| h.human_bones.as_ref()) {
            for bone in bones {
                if let (Some(name), Some(node)) = (bone.bone, bone.node) {
                    human_bones.push((HumanBoneName::from(name), node as usize));
                }
            }
        }

        let head = human_bones
            .iter()
            .find(|(bone, _)| *bone == HumanBoneName::Head)
            .map(|(_, node)| *node);

        let annotations = vrm
            .first_person
            .as_ref()
            .and_then(|fp| fp.mesh_annotations.as_ref());
        let mut annotated = HashMap::new();
        if let Some(annotations) = annotations {
            for node in gltf.nodes() {
                let Some(mesh) = node.mesh() else {
                    continue;
                };
                if let Some(annotation) = annotations
                    .iter()
                    .find(|a| a.mesh == Some(mesh.index() as u32))
                {
                    annotated.insert(
                        node.index(),
                        FirstPersonType::from(annotation.first_person_flag),
                    );
                }
            }
        }

        let springs = vrm
            .secondary_animation
            .as_ref()
            .and_then(|sa| sa.bone_groups.as_ref())
            .map_or_else(Default::default, |groups| {
                groups
                    .iter()
                    .map(|group| {
                        let mut spring = spring_from_group(gltf, group);
                        let collider_groups = vrm
                            .secondary_animation
                            .as_ref()
                            .and_then(|secondary| secondary.collider_groups.as_ref());
                        let mut seen = HashSet::new();
                        for index in group.collider_groups.iter().flatten() {
                            if !seen.insert(*index) {
                                continue;
                            }
                            let Some(colliders) =
                                collider_groups.and_then(|groups| groups.get(*index as usize))
                            else {
                                continue;
                            };
                            let Some(node) = colliders.node else {
                                continue;
                            };
                            for collider in colliders.colliders.iter().flatten() {
                                let offset = collider.offset.as_ref().map_or(Vec3::ZERO, |value| {
                                    // VRM 0.x collider offsets retain Unity's
                                    // Z convention rather than glTF node space.
                                    Vec3::new(value.x, value.y, -value.z)
                                });
                                let shape = SpringColliderShape::Sphere {
                                    offset,
                                    radius: collider.radius.unwrap_or(0.),
                                };
                                if node as usize >= gltf.nodes().len()
                                    || !shape.is_valid()
                                    || spring.colliders.len() == MAX_COLLIDERS
                                {
                                    continue;
                                }
                                spring.colliders.push(ColliderDesc {
                                    node: node as usize,
                                    shape,
                                });
                            }
                        }
                        spring
                    })
                    .collect()
            });

        Self {
            facing: Facing::NegativeZ,
            human_bones,
            first_person: resolve_first_person(gltf, head, &annotated),
            springs,
        }
    }
}

/// Defaults follow `UniVRM`; the VRM 0.0 schema declares none.
fn group_settings(group: &BoneGroup) -> SpringSettings {
    SpringSettings {
        stiffness: group.stiffiness.unwrap_or(1.0),
        drag_force: group.drag_force.unwrap_or(0.4),
        gravity_power: group.gravity_power.unwrap_or(0.0),
        gravity_dir: group
            .gravity_dir
            .as_ref()
            .map_or(Vec3::NEG_Y, |d| Vec3::new(d.x, d.y, d.z)),
        hit_radius: group.hit_radius.unwrap_or(0.02),
    }
}

/// Joints are ordered pre-order, so every parent precedes its children.
pub(super) fn spring_from_group(gltf: &gltf::Gltf, group: &BoneGroup) -> SpringDesc {
    let nodes = gltf.nodes().collect::<Vec<_>>();
    let settings = group_settings(group);

    let mut parents = HashMap::new();
    for node in &nodes {
        for child in node.children() {
            parents.insert(child.index(), node.index());
        }
    }

    let listed = group
        .bones
        .iter()
        .flatten()
        .map(|root| *root as usize)
        .collect::<HashSet<_>>();

    let mut joints = Vec::new();
    let mut visited = HashSet::new();

    for root in group.bones.iter().flatten() {
        let root = *root as usize;
        if has_listed_ancestor(root, &listed, &parents) {
            continue;
        }
        let mut stack = vec![root];

        while let Some(index) = stack.pop() {
            let Some(node) = nodes.get(index) else {
                continue;
            };
            if !visited.insert(index) {
                continue;
            }

            let children = node.children().map(|c| c.index()).collect::<Vec<_>>();
            let tail = children
                .first()
                .map_or(Tail::Extrapolated, |c| Tail::Node(*c));
            joints.push(JointDesc {
                node: index,
                tail,
                settings,
            });

            stack.extend(children.into_iter().rev());
        }
    }

    SpringDesc {
        joints,
        colliders: Vec::new(),
    }
}

/// A root below another listed root is reached through that root's pre-order.
fn has_listed_ancestor(
    node: usize,
    listed: &HashSet<usize>,
    parents: &HashMap<usize, usize>,
) -> bool {
    let mut seen = HashSet::new();
    let mut current = node;

    while let Some(&parent) = parents.get(&current) {
        if listed.contains(&parent) {
            return true;
        }
        if !seen.insert(parent) {
            return false;
        }
        current = parent;
    }

    false
}
