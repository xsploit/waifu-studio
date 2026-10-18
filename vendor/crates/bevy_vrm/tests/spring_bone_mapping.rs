use bevy::{
    ecs::{component::Component, entity::EntityHashMap},
    prelude::{Entity, Vec3, World},
};
use bevy_vrm::spring_bones::{
    SpringBones, SpringChain, SpringCollider, SpringColliderShape, SpringJoint, SpringSettings,
};

#[test]
fn spring_bones_remap_bone_tail_and_collider_on_component_map_entities() {
    let mut world = World::new();
    let bone = world.spawn_empty().id();
    let tail = world.spawn_empty().id();
    let new_bone = world.spawn_empty().id();
    let new_tail = world.spawn_empty().id();
    let collider = world.spawn_empty().id();
    let new_collider = world.spawn_empty().id();

    let mut spring_bones = SpringBones(vec![SpringChain {
        colliders: vec![SpringCollider {
            entity: collider,
            shape: SpringColliderShape::Sphere {
                offset: Vec3::X,
                radius: 0.2,
            },
        }],
        joints: vec![SpringJoint {
            bone,
            tail,
            settings: SpringSettings {
                stiffness: 0.0,
                drag_force: 0.0,
                gravity_power: 0.0,
                gravity_dir: Vec3::ZERO,
                hit_radius: 0.0,
            },
        }],
    }]);

    let mut mapper = EntityHashMap::<Entity>::default();
    mapper.insert(bone, new_bone);
    mapper.insert(tail, new_tail);
    mapper.insert(collider, new_collider);
    Component::map_entities(&mut spring_bones, &mut mapper);

    let joint = &spring_bones.0[0].joints[0];
    assert_eq!(joint.bone, new_bone, "bone must be remapped");
    assert_eq!(joint.tail, new_tail, "tail must be remapped");
    assert_eq!(
        spring_bones.0[0].colliders[0].entity, new_collider,
        "collider attachment must be remapped"
    );
}
