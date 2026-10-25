//! Builds an [`Avatar`] from the `VRMC_vrm` and `VRMC_springBone` extensions.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use serde_vrm::vrm1::{
    vrmc_spring_bone::{Shape, VrmcSpringBone},
    vrmc_vrm::{VrmcVrm, humanoid::HumanBoneName},
};

use super::avatar::{
    Avatar, ColliderDesc, Facing, JointDesc, SpringDesc, Tail, resolve_first_person,
};
use crate::spring_bones::{MAX_COLLIDERS, SpringColliderShape, SpringSettings};

impl Avatar {
    pub(crate) fn from_vrm1(
        gltf: &gltf::Gltf,
        vrm: &VrmcVrm,
        springs: Option<&VrmcSpringBone>,
    ) -> Self {
        let human_bones = vrm
            .humanoid
            .human_bones
            .iter()
            .map(|(name, bone)| (*name, bone.node as usize))
            .collect::<Vec<_>>();

        let head = human_bones
            .iter()
            .find(|(bone, _)| *bone == HumanBoneName::Head)
            .map(|(_, node)| *node);

        let annotated = vrm
            .first_person
            .as_ref()
            .map_or_else(Default::default, |fp| {
                fp.mesh_annotations
                    .iter()
                    .map(|a| (a.node as usize, a.kind))
                    .collect::<HashMap<_, _>>()
            });

        Self {
            facing: Facing::PositiveZ,
            human_bones,
            first_person: resolve_first_person(gltf, head, &annotated),
            springs: springs.map_or_else(Default::default, springs_from_vrm1),
        }
    }
}

/// The last joint of a spring only serves as the tail of the one before it.
pub(super) fn springs_from_vrm1(spring_bone: &VrmcSpringBone) -> Vec<SpringDesc> {
    let mut springs = Vec::new();

    for spring in &spring_bone.springs {
        if spring.joints.len() < 2 {
            continue;
        }

        let joints = spring
            .joints
            .windows(2)
            .map(|pair| JointDesc {
                node: pair[0].node as usize,
                tail: Tail::Node(pair[1].node as usize),
                settings: SpringSettings {
                    stiffness: pair[0].stiffness,
                    drag_force: pair[0].drag_force,
                    gravity_power: pair[0].gravity_power,
                    gravity_dir: Vec3::from_array(pair[0].gravity_dir),
                    hit_radius: pair[0].hit_radius,
                },
            })
            .collect();

        let mut colliders = Vec::new();
        let mut seen = HashSet::new();
        for group in &spring.collider_groups {
            let Some(group) = spring_bone.collider_groups.get(*group as usize) else {
                continue;
            };
            for index in &group.colliders {
                if !seen.insert(*index) || colliders.len() == MAX_COLLIDERS {
                    continue;
                }
                let Some(collider) = spring_bone.colliders.get(*index as usize) else {
                    continue;
                };
                let shape = match collider.shape {
                    Shape::Sphere(sphere) => SpringColliderShape::Sphere {
                        offset: Vec3::from_array(sphere.offset),
                        radius: sphere.radius,
                    },
                    Shape::Capsule(capsule) => SpringColliderShape::Capsule {
                        offset: Vec3::from_array(capsule.offset),
                        tail: Vec3::from_array(capsule.tail),
                        radius: capsule.radius,
                    },
                };
                if shape.is_valid() {
                    colliders.push(ColliderDesc {
                        node: collider.node as usize,
                        shape,
                    });
                }
            }
        }
        springs.push(SpringDesc { joints, colliders });
    }

    springs
}
