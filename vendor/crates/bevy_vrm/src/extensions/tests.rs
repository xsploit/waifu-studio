//! Unit tests for avatar construction and application.

use std::{
    collections::{HashMap, HashSet},
    f32::consts::{FRAC_PI_2, PI},
};

use bevy::prelude::*;
use serde_vrm::{
    vrm0,
    vrm1::{
        vrmc_spring_bone::VrmcSpringBone,
        vrmc_vrm::{VrmcVrm, humanoid::HumanBoneName},
    },
};

use super::{
    avatar::{Avatar, Facing, JointDesc, SpringDesc, Tail},
    vrm0::spring_from_group,
    vrm1::springs_from_vrm1,
};
use crate::spring_bones::{SpringBones, SpringColliderShape, SpringSettings};

fn load_gltf(file: &str) -> gltf::Gltf {
    let path = format!("{}/../../assets/{file}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).expect("asset must be readable");
    gltf::Gltf::from_slice_without_validation(&bytes).expect("asset must be valid glTF")
}

fn settings(stiffness: f32) -> SpringSettings {
    SpringSettings {
        stiffness,
        drag_force: 0.5,
        gravity_power: 0.0,
        gravity_dir: Vec3::NEG_Y,
        hit_radius: 0.0,
    }
}

#[test]
fn vrm0_bone_groups_expand_to_descendants_with_tails() {
    let gltf = load_gltf("alicia.vrm");
    let value = gltf
        .extension_value(vrm0::Vrm::EXTENSION_NAME)
        .expect("alicia.vrm must have a VRM extension");
    let vrm = serde_json::from_value::<vrm0::Vrm>(value.clone()).expect("VRM extension must parse");

    let avatar = Avatar::from_vrm0(&gltf, &vrm);

    assert_eq!(avatar.facing, Facing::NegativeZ);
    assert!(
        avatar
            .human_bones
            .iter()
            .any(|(bone, _)| *bone == HumanBoneName::Head),
        "Head must be in the humanoid"
    );
    assert!(!avatar.springs.is_empty(), "alicia.vrm must have springs");

    let nodes = gltf.nodes().collect::<Vec<_>>();
    for spring in &avatar.springs {
        let chain = spring.joints.iter().map(|j| j.node).collect::<Vec<_>>();

        for (position, joint) in spring.joints.iter().enumerate() {
            let node = &nodes[joint.node];
            let first_child = node.children().next().map(|c| c.index());
            match (joint.tail, first_child) {
                (Tail::Node(tail), Some(child)) => {
                    assert_eq!(tail, child, "tail must be the first child");
                }
                (Tail::Extrapolated, None) => {}
                (tail, child) => panic!("tail {tail:?} does not match first child {child:?}"),
            }

            let parent = nodes
                .iter()
                .find(|n| n.children().any(|c| c.index() == joint.node))
                .map(gltf::Node::index);
            if let Some(parent_position) = parent.and_then(|p| chain.iter().position(|n| *n == p)) {
                assert!(
                    parent_position < position,
                    "node {} must come after its parent",
                    joint.node
                );
            }
        }
    }
}

#[test]
fn vrm1_spring_end_joint_is_only_a_tail() {
    let spring_bone = serde_json::from_value::<VrmcSpringBone>(serde_json::json!({
        "specVersion": "1.0",
        "springs": [
            {
                "joints": [
                    { "node": 1, "stiffness": 0.1, "dragForce": 0.2, "hitRadius": 0.3 },
                    { "node": 2, "stiffness": 0.4, "dragForce": 0.5, "hitRadius": 0.6 },
                    { "node": 3, "stiffness": 0.7, "dragForce": 0.8, "hitRadius": 0.9 }
                ]
            },
            { "joints": [{ "node": 4 }] }
        ]
    }))
    .expect("spring bone JSON must parse");

    let springs = springs_from_vrm1(&spring_bone);

    assert_eq!(
        springs.len(),
        1,
        "a one-joint spring has nothing to simulate"
    );
    let joints = &springs[0].joints;
    assert_eq!(joints.len(), 2);
    assert_eq!((joints[0].node, joints[0].tail), (1, Tail::Node(2)));
    assert_eq!((joints[1].node, joints[1].tail), (2, Tail::Node(3)));
    assert_eq!(joints[0].settings.stiffness, 0.1);
    assert_eq!(joints[0].settings.hit_radius, 0.3);
    assert_eq!(joints[1].settings.stiffness, 0.4);
    assert_eq!(joints[1].settings.drag_force, 0.5);
}

#[test]
fn vrm1_avatar_faces_positive_z_and_maps_humanoid() {
    let gltf = load_gltf("vrm1_mtoon_uv_test.vrm");
    let value = gltf
        .extension_value(VrmcVrm::EXTENSION_NAME)
        .expect("sample must have a VRMC_vrm extension");
    let vrm = serde_json::from_value::<VrmcVrm>(value.clone()).expect("VRMC_vrm must parse");

    let avatar = Avatar::from_vrm1(&gltf, &vrm, None);

    assert_eq!(avatar.facing, Facing::PositiveZ);
    assert!(
        avatar
            .human_bones
            .iter()
            .any(|(bone, _)| *bone == HumanBoneName::Head),
        "Head must be in the humanoid"
    );

    let mesh_nodes = gltf
        .nodes()
        .filter(|n| n.mesh().is_some())
        .map(|n| n.index())
        .collect::<HashSet<_>>();
    let covered = avatar
        .first_person
        .iter()
        .map(|(node, _)| *node)
        .collect::<HashSet<_>>();
    assert!(!mesh_nodes.is_empty());
    assert_eq!(
        covered, mesh_nodes,
        "every mesh node needs a first-person type"
    );
}

#[test]
fn apply_rotates_vrm1_root_and_spawns_extrapolated_tails() {
    let mut world = World::new();
    let root = world.spawn(Transform::default()).id();
    let bone_rotation = Quat::from_rotation_z(FRAC_PI_2);
    let bone = world
        .spawn((
            Transform::from_xyz(0.0, 0.1, 0.0).with_rotation(bone_rotation),
            ChildOf(root),
        ))
        .id();

    let avatar = Avatar {
        facing: Facing::PositiveZ,
        human_bones: Vec::new(),
        first_person: Vec::new(),
        springs: vec![SpringDesc {
            colliders: Vec::new(),
            joints: vec![JointDesc {
                node: 0,
                tail: Tail::Extrapolated,
                settings: settings(1.0),
            }],
        }],
    };

    avatar.apply(&mut world, root, &HashMap::from([(0, bone)]));

    let rotation = world
        .get::<Transform>(root)
        .expect("root has a transform")
        .rotation;
    assert!(
        rotation.angle_between(Quat::from_rotation_y(PI)) < 1.0e-5,
        "root must turn 180 degrees about Y"
    );

    let spring_bones = world
        .get::<SpringBones>(root)
        .expect("root gets SpringBones");
    assert_eq!(spring_bones.0.len(), 1);
    let joint = &spring_bones.0[0].joints[0];
    assert_eq!(joint.bone, bone);

    let tail = world
        .get::<Transform>(joint.tail)
        .expect("tail has a transform");
    assert!(
        tail.translation
            .distance(bone_rotation.inverse() * Vec3::new(0.0, 0.07, 0.0))
            < 1.0e-5,
        "tail extends along the parent-to-bone direction"
    );
    assert_eq!(
        world.get::<ChildOf>(joint.tail).map(ChildOf::parent),
        Some(bone),
        "tail must be a child of the bone"
    );
}

#[test]
fn apply_leaves_vrm0_root_rotation_unchanged() {
    let mut world = World::new();
    let rotation = Quat::from_rotation_x(0.3);
    let root = world.spawn(Transform::from_rotation(rotation)).id();

    let avatar = Avatar {
        facing: Facing::NegativeZ,
        human_bones: Vec::new(),
        first_person: Vec::new(),
        springs: Vec::new(),
    };
    avatar.apply(&mut world, root, &HashMap::new());

    let after = world
        .get::<Transform>(root)
        .expect("root has a transform")
        .rotation;
    assert_eq!(after, rotation, "a -Z avatar must not be rotated");
}

#[test]
fn vrm0_descendant_listed_before_ancestor_still_follows_it() {
    let gltf = gltf::Gltf::from_slice_without_validation(
        br#"{"asset":{"version":"2.0"},"nodes":[{"children":[1]},{"children":[2]},{}]}"#,
    )
    .expect("glTF JSON must parse");
    let group = vrm0::BoneGroup {
        bones: Some(vec![1, 0]),
        ..default()
    };

    let spring = spring_from_group(&gltf, &group);

    let order = spring.joints.iter().map(|j| j.node).collect::<Vec<_>>();
    assert_eq!(order, [0, 1, 2], "parents must precede children");
}

#[test]
fn collider_vrm0_references_are_scoped_and_offsets_use_gltf_z() {
    let gltf = gltf::Gltf::from_slice_without_validation(
        br#"{"asset":{"version":"2.0"},"nodes":[{},{},{"children":[3]},{}]}"#,
    )
    .expect("fixture glTF");
    let vrm = serde_json::from_value::<vrm0::Vrm>(serde_json::json!({
        "secondaryAnimation": {
            "colliderGroups": [
                { "node": 0, "colliders": [{"offset":{"x":0.1,"y":0.2,"z":0.3},"radius":0.25}] },
                { "node": 999, "colliders": [{"radius":0.4}] },
                { "node": 1, "colliders": [{"radius":10.0}] }
            ],
            "boneGroups": [
                {"bones":[2],"colliderGroups":[0,0,1,99]},
                {"bones":[2]}
            ]
        }
    }))
    .expect("fixture VRM0");
    let avatar = Avatar::from_vrm0(&gltf, &vrm);
    assert_eq!(avatar.springs[0].colliders.len(), 1);
    assert!(avatar.springs[1].colliders.is_empty());
    assert_eq!(avatar.springs[0].colliders[0].node, 0);
    assert_eq!(
        avatar.springs[0].colliders[0].shape,
        SpringColliderShape::Sphere {
            offset: Vec3::new(0.1, 0.2, -0.3),
            radius: 0.25
        }
    );
    let mut world = World::new();
    let root = world.spawn(Transform::default()).id();
    let collider = world.spawn(Transform::default()).id();
    let bone = world.spawn((Transform::default(), ChildOf(root))).id();
    let tail = world
        .spawn((Transform::from_xyz(0., -0.1, 0.), ChildOf(bone)))
        .id();
    avatar.apply(
        &mut world,
        root,
        &HashMap::from([(0, collider), (2, bone), (3, tail)]),
    );
    let springs = world
        .get::<SpringBones>(root)
        .expect("runtime spring chains");
    assert_eq!(springs.0[0].colliders[0].entity, collider);
    assert_eq!(
        springs.0[0].colliders[0].shape,
        avatar.springs[0].colliders[0].shape
    );
    assert!(springs.0[1].colliders.is_empty());
}

#[test]
fn collider_vrm1_resolves_only_referenced_spheres_and_capsules_and_skips_bad_references() {
    let spring_bone = serde_json::from_value::<VrmcSpringBone>(serde_json::json!({
        "specVersion":"1.0",
        "colliders":[
            {"node":0,"shape":{"sphere":{"offset":[0.1,0.2,0.3],"radius":0.25}}},
            {"node":1,"shape":{"capsule":{"offset":[0.,0.1,0.],"tail":[0.,0.6,0.],"radius":0.2}}},
            {"node":2,"shape":{"sphere":{"radius":10.}}},
            {"node":0,"shape":{"sphere":{"radius":-1.}}}
        ],
        "colliderGroups":[{"colliders":[0,1,0,3,99]},{"colliders":[2]}],
        "springs":[
            {"joints":[{"node":3},{"node":4}],"colliderGroups":[0,0,99]},
            {"joints":[{"node":3},{"node":4}]}
        ]
    }))
    .expect("fixture spring bone");
    let springs = springs_from_vrm1(&spring_bone);
    assert_eq!(springs[0].colliders.len(), 2);
    assert!(springs[1].colliders.is_empty());
    assert_eq!(
        springs[0].colliders[0].shape,
        SpringColliderShape::Sphere {
            offset: Vec3::new(0.1, 0.2, 0.3),
            radius: 0.25
        }
    );
    assert_eq!(
        springs[0].colliders[1].shape,
        SpringColliderShape::Capsule {
            offset: Vec3::new(0., 0.1, 0.),
            tail: Vec3::new(0., 0.6, 0.),
            radius: 0.2
        }
    );
    let avatar = Avatar {
        facing: Facing::PositiveZ,
        human_bones: Vec::new(),
        first_person: Vec::new(),
        springs,
    };
    let mut world = World::new();
    let root = world.spawn(Transform::default()).id();
    let attachment = world.spawn(Transform::default()).id();
    let bone = world.spawn((Transform::default(), ChildOf(root))).id();
    let tail = world
        .spawn((Transform::from_xyz(0., -0.1, 0.), ChildOf(bone)))
        .id();
    // Capsule's node is deliberately absent: it is not bound to some other node.
    avatar.apply(
        &mut world,
        root,
        &HashMap::from([(0, attachment), (3, bone), (4, tail)]),
    );
    let chains = world
        .get::<SpringBones>(root)
        .expect("runtime spring chains");
    assert_eq!(chains.0[0].colliders.len(), 1);
    assert_eq!(chains.0[0].colliders[0].entity, attachment);
}

#[test]
fn collider_bundled_neuro_keeps_hair_and_skirt_references_when_available() {
    let path = std::env::var_os("WAIFU_VRM_COLLIDER_FIXTURE").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../assets/characters/models/4e5c5188c39e-neuro-sama.vrm")
        },
        std::path::PathBuf::from,
    );
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let gltf = gltf::Gltf::from_slice_without_validation(&bytes).expect("bundled Neuro glTF");
    let vrm: vrm0::Vrm =
        serde_json::from_value(gltf.extension_value("VRM").expect("VRM0 extension").clone())
            .expect("Neuro VRM0");
    let avatar = Avatar::from_vrm0(&gltf, &vrm);
    let secondary = vrm
        .secondary_animation
        .as_ref()
        .expect("secondary animation");
    let groups = secondary.collider_groups.as_ref().expect("collider groups");
    assert_eq!(groups.len(), 12);
    assert_eq!(
        groups
            .iter()
            .map(|group| group.colliders.as_ref().map_or(0, Vec::len))
            .sum::<usize>(),
        28
    );
    let mut world = World::new();
    let root = world.spawn(Transform::default()).id();
    let nodes: HashMap<_, _> = gltf
        .nodes()
        .map(|node| {
            let matrix = Mat4::from_cols_array_2d(&node.transform().matrix());
            (
                node.index(),
                world
                    .spawn((Transform::from_matrix(matrix), ChildOf(root)))
                    .id(),
            )
        })
        .collect();
    for node in gltf.nodes() {
        for child in node.children() {
            world
                .entity_mut(nodes[&child.index()])
                .insert(ChildOf(nodes[&node.index()]));
        }
    }
    avatar.apply(&mut world, root, &nodes);
    let runtime = world
        .get::<SpringBones>(root)
        .expect("applied Neuro springs");
    assert_eq!(runtime.0.len(), avatar.springs.len());
    for (index, (source, runtime)) in avatar.springs.iter().zip(&runtime.0).enumerate() {
        let description = &secondary.bone_groups.as_ref().expect("bone groups")[index];
        let references: HashSet<_> = description
            .collider_groups
            .iter()
            .flatten()
            .copied()
            .collect();
        let expected = references
            .iter()
            .map(|index| {
                groups[*index as usize]
                    .colliders
                    .as_ref()
                    .map_or(0, Vec::len)
            })
            .sum::<usize>();
        assert_eq!(source.colliders.len(), expected, "spring {index}");
        assert_eq!(runtime.colliders.len(), expected, "spring {index}");
        for (source, runtime) in source.colliders.iter().zip(&runtime.colliders) {
            assert_eq!(runtime.entity, nodes[&source.node]);
            assert_eq!(runtime.shape, source.shape);
        }
    }
}
