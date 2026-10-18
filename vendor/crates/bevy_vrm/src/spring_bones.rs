//! Spring bone components and the simulation driving them.

use bevy::{
    ecs::entity::MapEntities,
    prelude::*,
    transform::{
        TransformSystems,
        systems::{mark_dirty_trees, propagate_parent_transforms, sync_simple_transforms},
    },
};

const SPRING_STEP: f64 = 1.0 / 60.0;
const MAX_FRAME_TIME: f64 = 0.25;
const MAX_STEPS: usize = 15;
const MAX_ANCESTRY: usize = 256;
pub(crate) const MAX_COLLIDERS: usize = 256;
const COLLISION_PASSES: usize = 8;

/// Fixed-step Verlet history must never be interpreted at a different timestep.
#[derive(Resource, Default)]
struct SpringClock {
    remainder: f64,
}

impl SpringClock {
    fn advance(&mut self, elapsed: f64) -> usize {
        if !elapsed.is_finite() || elapsed <= 0.0 {
            return 0;
        }
        self.remainder += elapsed.min(MAX_FRAME_TIME);
        let steps = ((self.remainder / SPRING_STEP + 1e-9).floor() as usize).min(MAX_STEPS);
        self.remainder = (steps as f64)
            .mul_add(-SPRING_STEP, self.remainder)
            .max(0.0);
        steps
    }
}

/// A resolved, degenerate joint is terminal; do not retry it on every frame.
#[derive(Component)]
struct InvalidSpringJoint;

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct SpringBones(#[entities] pub Vec<SpringChain>);

/// Joints ordered so that every parent precedes its children.
#[derive(Reflect)]
pub struct SpringChain {
    pub joints: Vec<SpringJoint>,
    pub colliders: Vec<SpringCollider>,
}

impl MapEntities for SpringChain {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        for joint in &mut self.joints {
            joint.bone = entity_mapper.get_mapped(joint.bone);
            joint.tail = entity_mapper.get_mapped(joint.tail);
        }
        for collider in &mut self.colliders {
            collider.entity = entity_mapper.get_mapped(collider.entity);
        }
    }
}

/// Collider geometry is local to its attached node, not the avatar root.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct SpringCollider {
    pub entity: Entity,
    pub shape: SpringColliderShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum SpringColliderShape {
    Sphere {
        offset: Vec3,
        radius: f32,
    },
    Capsule {
        offset: Vec3,
        tail: Vec3,
        radius: f32,
    },
}

impl SpringColliderShape {
    pub(crate) fn is_valid(self) -> bool {
        let (offset, tail, radius) = match self {
            Self::Sphere { offset, radius } => (offset, offset, radius),
            Self::Capsule {
                offset,
                tail,
                radius,
            } => (offset, tail, radius),
        };
        offset.is_finite()
            && tail.is_finite()
            && offset.abs().max_element() <= 1e6
            && tail.abs().max_element() <= 1e6
            && radius.is_finite()
            && (0.0..=1e6).contains(&radius)
    }

    fn in_world(self, global: GlobalTransform) -> Option<WorldCollider> {
        if !self.is_valid() || !global.to_matrix().is_finite() {
            return None;
        }
        let (offset, tail, radius) = match self {
            Self::Sphere { offset, radius } => (offset, offset, radius),
            Self::Capsule {
                offset,
                tail,
                radius,
            } => (offset, tail, radius),
        };
        // A conservative sphere radius under nonuniform node scaling.
        let radius = radius * global.scale().abs().max_element();
        let head = global.transform_point(offset);
        let tail = global.transform_point(tail);
        (head.is_finite() && tail.is_finite() && radius.is_finite()).then_some(WorldCollider {
            head,
            tail,
            radius,
        })
    }
}

#[derive(Clone, Copy)]
struct WorldCollider {
    head: Vec3,
    tail: Vec3,
    radius: f32,
}

impl WorldCollider {
    fn closest(self, point: Vec3) -> Vec3 {
        let segment = self.tail - self.head;
        let length_squared = segment.length_squared();
        if length_squared <= 1e-12 {
            return self.head;
        }
        self.head + segment * ((point - self.head).dot(segment) / length_squared).clamp(0., 1.)
    }
}

/// Resolve a sphere while keeping the tail on the bone's fixed-length sphere.
/// A simple push followed by normalization fails for collinear head colliders.
fn project_sphere(origin: Vec3, length: f32, point: Vec3, center: Vec3, radius: f32) -> Vec3 {
    if point.distance_squared(center) >= radius * radius {
        return point;
    }
    let relative = center - origin;
    let distance = relative.length();
    if distance <= 1e-6 {
        return point;
    } // Fully enclosed or already valid: no angular solution.
    let axis = relative / distance;
    let cosine = (radius.mul_add(-radius, distance.mul_add(distance, length * length))
        / (2. * length * distance))
        .clamp(-1., 1.);
    let toward = (point - origin) / length;
    let tangent = (toward - axis * toward.dot(axis))
        .try_normalize()
        .unwrap_or_else(|| axis.any_orthonormal_vector());
    origin + (axis * cosine + tangent * (1. - cosine * cosine).max(0.).sqrt()) * length
}

fn resolve_collisions(
    origin: Vec3,
    length: f32,
    point: Vec3,
    hit_radius: f32,
    colliders: &[WorldCollider],
) -> Vec3 {
    let mut point = point;
    for _ in 0..COLLISION_PASSES {
        let mut changed = false;
        for collider in colliders {
            let center = collider.closest(point);
            let radius = collider.radius + hit_radius;
            if point.distance_squared(center) < radius * radius {
                let next = project_sphere(
                    origin,
                    length,
                    point,
                    center,
                    1e-5_f32.mul_add(length.max(1.), radius),
                );
                if next.is_finite() {
                    point = next;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    point
}

/// Simulates `bone` swinging toward `tail`, a descendant of `bone`.
#[derive(Reflect)]
pub struct SpringJoint {
    pub bone: Entity,
    pub tail: Entity,
    pub settings: SpringSettings,
}

#[derive(Clone, Copy, Debug, Reflect, PartialEq)]
pub struct SpringSettings {
    pub stiffness: f32,
    pub drag_force: f32,
    pub gravity_power: f32,
    pub gravity_dir: Vec3,
    pub hit_radius: f32,
}

impl SpringSettings {
    fn is_finite(&self) -> bool {
        self.stiffness.is_finite()
            && self.drag_force.is_finite()
            && self.gravity_power.is_finite()
            && self.gravity_dir.is_finite()
            && self.hit_radius.is_finite()
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct SpringBoneLogicState {
    pub prev_tail: Vec3,
    pub current_tail: Vec3,
    pub bone_axis: Vec3,
    pub bone_length: f32,
    pub initial_local_matrix: Mat4,
    pub initial_local_rotation: Quat,
}

pub struct SpringBonePlugin;

impl Plugin for SpringBonePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpringClock>()
            .register_type::<SpringBoneLogicState>()
            .register_type::<SpringBones>()
            .add_systems(
                PostUpdate,
                (
                    initialize_spring_bone_logic,
                    do_springbone_logic,
                    // Springs write local poses after the ordinary propagation pass.
                    // Run the standard pass again so skinning and every descendant
                    // observe the final pose in this same frame.
                    (
                        mark_dirty_trees,
                        propagate_parent_transforms,
                        sync_simple_transforms,
                    )
                        .chain()
                        .run_if(has_spring_joints),
                )
                    .chain()
                    .after(TransformSystems::Propagate),
            );
    }
}

fn has_spring_joints(springs: Query<&SpringBones>) -> bool {
    springs
        .iter()
        .any(|springs| springs.0.iter().any(|chain| !chain.joints.is_empty()))
}

/// Position of `tail` in `bone`'s local space, composed from local
/// transforms. `None` unless `bone` is a strict ancestor of `tail`.
fn tail_offset(
    bone: Entity,
    tail: Entity,
    parents: &Query<&ChildOf>,
    local_transforms: &Query<&Transform>,
) -> Option<Vec3> {
    let mut accumulated = Transform::IDENTITY;
    let mut current = tail;

    let mut depth = 0;
    while current != bone {
        if depth == MAX_ANCESTRY {
            return None;
        }
        depth += 1;
        let local = local_transforms.get(current).ok()?;
        accumulated = local.mul_transform(accumulated);
        current = parents.get(current).ok()?.parent();
    }

    (tail != bone).then_some(accumulated.translation)
}

fn initialize_spring_bone_logic(
    global_transforms: Query<&GlobalTransform>,
    local_transforms: Query<&Transform>,
    logic_states: Query<&SpringBoneLogicState>,
    invalid_states: Query<(), With<InvalidSpringJoint>>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    spring_boness: Query<&SpringBones>,
) {
    for spring_bones in spring_boness.iter() {
        for chain in &spring_bones.0 {
            for joint in &chain.joints {
                if logic_states.contains(joint.bone) || invalid_states.contains(joint.bone) {
                    continue;
                }

                let Some(offset) = tail_offset(joint.bone, joint.tail, &parents, &local_transforms)
                else {
                    continue;
                };
                if !offset.is_finite() || offset.length_squared() <= 1e-12 {
                    commands.entity(joint.bone).insert(InvalidSpringJoint);
                    continue;
                }
                let Ok(global_this_bone) = global_transforms.get(joint.bone) else {
                    continue;
                };
                let Ok(local_this_bone) = local_transforms.get(joint.bone) else {
                    continue;
                };

                let bone_axis = offset.normalize_or_zero();
                let bone_length = offset.length();
                let initial_local_matrix = local_this_bone.to_matrix();
                let initial_local_rotation = local_this_bone.rotation;
                let current_tail = global_this_bone.transform_point(offset);
                if !bone_length.is_finite()
                    || !initial_local_rotation.is_finite()
                    || !current_tail.is_finite()
                    || !initial_local_matrix.is_finite()
                {
                    commands.entity(joint.bone).insert(InvalidSpringJoint);
                    continue;
                }

                commands.entity(joint.bone).insert(SpringBoneLogicState {
                    prev_tail: current_tail,
                    current_tail,
                    bone_axis,
                    bone_length,
                    initial_local_matrix,
                    initial_local_rotation,
                });
            }
        }
    }
}

fn compose_global(
    entity: Entity,
    transforms: &Query<(&mut GlobalTransform, &mut Transform)>,
    parents: &Query<&ChildOf>,
    ancestry: &mut Vec<Transform>,
) -> Option<GlobalTransform> {
    ancestry.clear();
    let mut cursor = entity;
    loop {
        if ancestry.len() == MAX_ANCESTRY {
            return None;
        }
        let (_, local) = transforms.get(cursor).ok()?;
        if !local.to_matrix().is_finite() {
            return None;
        }
        ancestry.push(*local);
        let Ok(parent) = parents.get(cursor) else {
            break;
        };
        cursor = parent.parent();
    }
    let mut global = GlobalTransform::IDENTITY;
    for local in ancestry.iter().rev() {
        global = global.mul_transform(*local);
    }
    global.to_matrix().is_finite().then_some(global)
}

fn do_springbone_logic(
    mut global_transforms: Query<(&mut GlobalTransform, &mut Transform)>,
    mut spring_bone_logic_states: Query<&mut SpringBoneLogicState>,
    parents: Query<&ChildOf>,
    spring_boness: Query<&SpringBones>,
    time: Res<Time>,
    mut clock: ResMut<SpringClock>,
    mut ancestry: Local<Vec<Transform>>,
    mut colliders: Local<Vec<WorldCollider>>,
) {
    for _ in 0..clock.advance(time.delta_secs_f64()) {
        for spring_bones in spring_boness.iter() {
            for chain in &spring_bones.0 {
                colliders.clear();
                for collider in chain.colliders.iter().take(MAX_COLLIDERS) {
                    if let Some(global) =
                        compose_global(collider.entity, &global_transforms, &parents, &mut ancestry)
                        && let Some(collider) = collider.shape.in_world(global)
                    {
                        colliders.push(collider);
                    }
                }
                for joint in &chain.joints {
                    let bone = joint.bone;
                    let settings = &joint.settings;

                    let Ok((_, local_bone)) = global_transforms.get(bone) else {
                        continue;
                    };
                    let local_bone = *local_bone;
                    let Ok(mut spring_bone_logic_state) = spring_bone_logic_states.get_mut(bone)
                    else {
                        continue;
                    };
                    let Ok(parent) = parents.get(bone) else {
                        continue;
                    };
                    let parent_entity = parent.parent();

                    // Include earlier simulated ancestors, even through non-spring nodes.
                    let Some(parent_global_transform) =
                        compose_global(parent_entity, &global_transforms, &parents, &mut ancestry)
                    else {
                        continue;
                    };
                    let parent_matrix = parent_global_transform.to_matrix();
                    let origin = parent_global_transform
                        .mul_transform(local_bone)
                        .translation();
                    let rest_matrix = parent_matrix * spring_bone_logic_state.initial_local_matrix;
                    let rest_direction =
                        rest_matrix.transform_vector3(spring_bone_logic_state.bone_axis);
                    let length = (rest_direction * spring_bone_logic_state.bone_length).length();
                    let rest_determinant = rest_matrix.determinant();
                    if !origin.is_finite()
                        || !length.is_finite()
                        || length <= 1e-6
                        || !rest_matrix.is_finite()
                        || !rest_determinant.is_finite()
                        || rest_determinant.abs() <= 1e-12
                        || !settings.is_finite()
                    {
                        continue;
                    }

                    let inertia = (spring_bone_logic_state.current_tail
                        - spring_bone_logic_state.prev_tail)
                        * (1.0 - settings.drag_force.clamp(0.0, 1.0));
                    let stiffness = SPRING_STEP as f32
                        * rest_direction.normalize_or_zero()
                        * settings.stiffness;
                    let external =
                        SPRING_STEP as f32 * settings.gravity_dir * settings.gravity_power;

                    let mut next_tail =
                        spring_bone_logic_state.current_tail + inertia + stiffness + external;
                    let direction = (next_tail - origin)
                        .try_normalize()
                        .unwrap_or_else(|| rest_direction.normalize_or_zero());
                    next_tail = origin + direction * length;
                    let hit_radius = settings.hit_radius.max(0.)
                        * parent_global_transform
                            .mul_transform(local_bone)
                            .scale()
                            .abs()
                            .max_element();
                    if !next_tail.is_finite() || !hit_radius.is_finite() {
                        continue;
                    }
                    next_tail =
                        resolve_collisions(origin, length, next_tail, hit_radius, &colliders);
                    let Some(to) = rest_matrix
                        .inverse()
                        .transform_point3(next_tail)
                        .try_normalize()
                    else {
                        continue;
                    };
                    let rotation = spring_bone_logic_state.initial_local_rotation
                        * Quat::from_rotation_arc(spring_bone_logic_state.bone_axis, to);
                    if !next_tail.is_finite() || !rotation.is_finite() {
                        continue;
                    }
                    spring_bone_logic_state.prev_tail = spring_bone_logic_state.current_tail;
                    spring_bone_logic_state.current_tail = next_tail;

                    let Ok((mut global, mut local)) = global_transforms.get_mut(bone) else {
                        continue;
                    };
                    local.rotation = rotation.normalize();
                    *global = parent_global_transform.mul_transform(*local);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    #[test]
    fn collider_sphere_projection_escapes_collinear_overlap_without_stretching() {
        let collider = WorldCollider {
            head: Vec3::Y * 0.85,
            tail: Vec3::Y * 0.85,
            radius: 0.2,
        };
        let point = resolve_collisions(Vec3::ZERO, 1., Vec3::Y, 0.1, &[collider]);
        assert!((point.length() - 1.).abs() < 1e-5);
        assert!(point.distance(collider.head) >= 0.3);
        assert!(point.is_finite());
        // The forbidden sphere contains the entire bone sphere. Keep a finite,
        // fixed-length answer even when no collision-free pose exists.
        let point = resolve_collisions(
            Vec3::ZERO,
            1.,
            Vec3::Y,
            0.1,
            &[WorldCollider {
                radius: 3.,
                ..collider
            }],
        );
        assert!((point.length() - 1.).abs() < 1e-5);
        assert!(point.is_finite());
    }

    #[test]
    fn collider_capsule_uses_segment_interior_and_degenerate_sphere() {
        let capsule = WorldCollider {
            head: Vec3::new(-0.5, 0.85, 0.),
            tail: Vec3::new(0.5, 0.85, 0.),
            radius: 0.2,
        };
        let point = resolve_collisions(Vec3::ZERO, 1., Vec3::Y, 0.1, &[capsule]);
        assert!((point.length() - 1.).abs() < 1e-5);
        assert!(
            point.distance(capsule.closest(point)) >= 0.3 - 1e-5,
            "{point:?}"
        );
        let sphere = WorldCollider {
            tail: capsule.head,
            ..capsule
        };
        assert_eq!(sphere.closest(Vec3::Y), capsule.head);
    }

    #[test]
    fn collider_world_geometry_follows_rotated_scaled_attachment_and_rejects_invalid_shapes() {
        let transform = Transform::from_xyz(2., 3., 4.)
            .with_rotation(Quat::from_rotation_z(0.7))
            .with_scale(Vec3::splat(2.));
        let shape = SpringColliderShape::Capsule {
            offset: Vec3::X,
            tail: Vec3::Y,
            radius: 0.2,
        };
        let world = shape
            .in_world(GlobalTransform::from(transform))
            .expect("valid collider");
        assert!(world.head.distance(transform.transform_point(Vec3::X)) < 1e-5);
        assert!(world.tail.distance(transform.transform_point(Vec3::Y)) < 1e-5);
        assert!((world.radius - 0.4).abs() < 1e-5);
        for radius in [-0.1, f32::NAN, f32::INFINITY] {
            assert!(
                SpringColliderShape::Sphere {
                    offset: Vec3::ZERO,
                    radius
                }
                .in_world(GlobalTransform::IDENTITY)
                .is_none()
            );
        }
        assert!(
            !SpringColliderShape::Capsule {
                offset: Vec3::ZERO,
                tail: Vec3::NAN,
                radius: 0.1
            }
            .is_valid()
        );
    }

    #[test]
    fn collider_solver_keeps_real_tail_finite_outside_sphere_and_capsule_at_multiple_frame_rates() {
        for capsule in [false, true] {
            for fps in [30, 60, 144] {
                let mut app = App::new();
                app.add_plugins((MinimalPlugins, TransformPlugin, SpringBonePlugin))
                    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                        1. / f64::from(fps),
                    )));
                let root_transform = Transform::from_xyz(2., 3., 4.)
                    .with_rotation(Quat::from_rotation_y(0.7))
                    .with_scale(Vec3::splat(2.));
                let root = app.world_mut().spawn(root_transform).id();
                let bone = app
                    .world_mut()
                    .spawn((Transform::default(), ChildOf(root)))
                    .id();
                let tail = app
                    .world_mut()
                    .spawn((Transform::from_xyz(0., -1., 0.), ChildOf(bone)))
                    .id();
                let attachment = app
                    .world_mut()
                    .spawn((Transform::from_xyz(0., -0.85, 0.), ChildOf(root)))
                    .id();
                let shape = if capsule {
                    SpringColliderShape::Capsule {
                        offset: Vec3::X * -0.5,
                        tail: Vec3::X * 0.5,
                        radius: 0.2,
                    }
                } else {
                    SpringColliderShape::Sphere {
                        offset: Vec3::ZERO,
                        radius: 0.2,
                    }
                };
                app.world_mut()
                    .entity_mut(root)
                    .insert(SpringBones(vec![SpringChain {
                        colliders: vec![SpringCollider {
                            entity: attachment,
                            shape,
                        }],
                        joints: vec![SpringJoint {
                            bone,
                            tail,
                            settings: SpringSettings {
                                stiffness: 0.,
                                drag_force: 1.,
                                gravity_power: 0.,
                                gravity_dir: Vec3::NEG_Y,
                                hit_radius: 0.1,
                            },
                        }],
                    }]));
                for _ in 0..fps * 3 {
                    app.update();
                }
                let state = app
                    .world()
                    .get::<SpringBoneLogicState>(bone)
                    .expect("initialized joint");
                let actual_tail = app
                    .world()
                    .get::<GlobalTransform>(tail)
                    .expect("propagated tail")
                    .translation();
                let collider = shape
                    .in_world(
                        *app.world()
                            .get::<GlobalTransform>(attachment)
                            .expect("collider transform"),
                    )
                    .expect("valid collider");
                assert!(actual_tail.is_finite());
                assert!(actual_tail.distance(state.current_tail) < 1e-4);
                assert!((actual_tail.distance(root_transform.translation) - 2.).abs() < 1e-4);
                assert!(
                    actual_tail.distance(collider.closest(actual_tail)) >= 0.6 - 1e-4,
                    "capsule={capsule}, fps={fps}, tail={actual_tail:?}"
                );
                if !capsule && fps == 60 {
                    // Move the attached body collider directly into the settled
                    // tail, as an animated head/chest can do between frames.
                    let toward_tail = root_transform
                        .to_matrix()
                        .inverse()
                        .transform_point3(actual_tail);
                    app.world_mut()
                        .get_mut::<Transform>(attachment)
                        .expect("attachment transform")
                        .translation = toward_tail * 0.85;
                    for _ in 0..60 {
                        app.update();
                    }
                    let tail = app
                        .world()
                        .get::<GlobalTransform>(tail)
                        .expect("updated tail")
                        .translation();
                    let collider = shape
                        .in_world(
                            *app.world()
                                .get::<GlobalTransform>(attachment)
                                .expect("moved collider"),
                        )
                        .expect("valid moved collider");
                    assert!(tail.distance(collider.closest(tail)) >= 0.6 - 1e-4);
                    assert!((tail.distance(root_transform.translation) - 2.).abs() < 1e-4);
                }
            }
        }
    }
    #[test]
    fn fixed_steps_match_at_low_and_high_frame_rates() {
        for fps in [5, 30, 60, 144] {
            let mut clock = SpringClock::default();
            let steps: usize = (0..fps * 10).map(|_| clock.advance(1.0 / fps as f64)).sum();
            assert_eq!(steps, 600, "fps={fps}");
            assert!(clock.remainder < SPRING_STEP);
        }
    }
    #[test]
    fn stalls_and_invalid_time_are_bounded() {
        let mut clock = SpringClock::default();
        assert_eq!(clock.advance(30.0), MAX_STEPS);
        assert_eq!(clock.advance(f64::NAN), 0);
        assert_eq!(clock.advance(f64::INFINITY), 0);
        assert_eq!(clock.advance(-1.0), 0);
        assert_eq!(clock.advance(0.0), 0);
        assert_eq!(clock.advance(SPRING_STEP), 1);
    }
}
