//! Bevy plugin implementing the [MToon](https://vrm.dev/en/univrm/shaders/shader_mtoon.html) shader.

// Bevy`s `AsBindGroup` needs higher limit
#![recursion_limit = "256"]

use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use bevy::{
    camera::visibility::RenderLayers,
    mesh::{VertexAttributeValues, morph::MeshMorphWeights, skinning::SkinnedMesh},
    prelude::*,
    shader::load_shader_library,
};

mod outline;
mod shader;

pub use outline::OutlineMaterial;
pub use shader::{MtoonMaterial, OutlineMode as VrmOutlineMode};

/// Importable from other WESL shaders as `bevy_shader_mtoon::mtoon`.
const SHADER_PATH: &str = "embedded://bevy_shader_mtoon/mtoon.wgsl";
const OUTLINE_SHADER_PATH: &str = "embedded://bevy_shader_mtoon/outline.wgsl";

#[derive(Default)]
pub struct MtoonPlugin;

impl Plugin for MtoonPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "mtoon.wgsl");
        load_shader_library!(app, "outline.wgsl");

        app.init_resource::<OutlineMeshCache>()
            .register_type::<OutlineSync>()
            .add_plugins((
                MaterialPlugin::<MtoonMaterial>::default(),
                MaterialPlugin::<OutlineMaterial>::default(),
            ))
            .add_systems(
                Update,
                (update_mtoon_shader, prune_outline_mesh_cache, sync_outline).chain(),
            )
            .add_systems(
                PostUpdate,
                sync_outline_morph.after(bevy::app::AnimationSystems),
            );
    }
}

#[derive(Bundle, Clone, Default)]
pub struct MtoonBundle {
    pub mtoon: MeshMaterial3d<MtoonMaterial>,
    pub outline_sync: OutlineSync,
}

/// Marks a [`DirectionalLight`] to be used for shading within the `MToon`
/// shader. Only a single [`MtoonSun`] is allowed.
#[derive(Component)]
pub struct MtoonSun;

fn update_mtoon_shader(
    mut materials: ResMut<Assets<MtoonMaterial>>,
    mut events: MessageReader<AssetEvent<MtoonMaterial>>,
    sun: Query<(&GlobalTransform, &DirectionalLight), With<MtoonSun>>,
    changed_sun: Query<
        (),
        (
            With<MtoonSun>,
            Or<(Changed<GlobalTransform>, Changed<DirectionalLight>)>,
        ),
    >,
) {
    let Ok((transform, light)) = sun.single() else {
        events.clear();
        return;
    };

    let light_dir = transform.back().as_vec3();
    let light_color = light.color;

    if !changed_sun.is_empty() {
        events.clear();
        // Assets::iter_mut marks every visited asset modified, even when its
        // values are identical. Inspect immutably before requesting mutation.
        let dirty: Vec<_> = materials
            .iter()
            .filter_map(|(id, material)| {
                (material.light_dir != light_dir || material.light_color != light_color)
                    .then_some(id)
            })
            .collect();
        for id in dirty {
            if let Some(mut material) = materials.get_mut(id) {
                material.light_dir = light_dir;
                material.light_color = light_color;
            }
        }
        return;
    }

    let added = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Added { id } => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();

    for id in added {
        let dirty = materials.get(id).is_some_and(|material| {
            material.light_dir != light_dir || material.light_color != light_color
        });
        if dirty && let Some(mut material) = materials.get_mut(id) {
            material.light_dir = light_dir;
            material.light_color = light_color;
        }
    }
}

/// Syncs an outline with the outline properties of an entity's
/// [`MtoonMaterial`]. The outline renders on a child entity so it coexists with
/// the base material.
#[derive(Component, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct OutlineSync;

/// Points a mesh at its outline child spawned by [`sync_outline`].
#[derive(Component)]
struct OutlineChild(Entity);

/// Mesh handles live with outline children, never in a process-lifetime cache.
struct OutlineMeshHandle(Handle<Mesh>);
#[derive(Component)]
struct SharedOutlineMesh {
    _handle: Arc<OutlineMeshHandle>,
}
#[derive(Resource, Default)]
struct OutlineMeshCache(HashMap<AssetId<Mesh>, Weak<OutlineMeshHandle>>);

fn prune_outline_mesh_cache(
    mut removed: RemovedComponents<SharedOutlineMesh>,
    mut cache: ResMut<OutlineMeshCache>,
) {
    if removed.read().count() > 0 {
        cache.0.retain(|_, handle| handle.strong_count() > 0);
    }
}

fn shared_outline_mesh(
    source: AssetId<Mesh>,
    meshes: &mut Assets<Mesh>,
    cache: &mut OutlineMeshCache,
) -> Option<Arc<OutlineMeshHandle>> {
    if let Some(shared) = cache.0.get(&source).and_then(Weak::upgrade) {
        return Some(shared);
    }
    let outline = smooth_outline_mesh(meshes.get(source)?)?;
    let shared = Arc::new(OutlineMeshHandle(meshes.add(outline)));
    cache.0.insert(source, Arc::downgrade(&shared));
    Some(shared)
}

/// Reference weights already share their animated source. Direct weights need
/// copying after animation; compare first to avoid marking unchanged children.
fn sync_outline_morph(
    parents: Query<
        (&OutlineChild, &MeshMorphWeights),
        (
            Changed<MeshMorphWeights>,
            Without<MeshMaterial3d<OutlineMaterial>>,
        ),
    >,
    mut children: Query<&mut MeshMorphWeights, With<MeshMaterial3d<OutlineMaterial>>>,
) {
    for (child, parent_weights) in &parents {
        if let Ok(mut child_weights) = children.get_mut(child.0) {
            let equal = match (&*child_weights, parent_weights) {
                (
                    MeshMorphWeights::Value { weights: a },
                    MeshMorphWeights::Value { weights: b },
                ) => a == b,
                (MeshMorphWeights::Reference(a), MeshMorphWeights::Reference(b)) => a == b,
                _ => false,
            };
            if !equal {
                *child_weights = parent_weights.clone();
            }
        }
    }
}

fn sync_outline(
    mut commands: Commands,
    mut outlines: ResMut<Assets<OutlineMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mtoons: Res<Assets<MtoonMaterial>>,
    mut mesh_cache: ResMut<OutlineMeshCache>,
    query: Query<
        (
            Entity,
            &MeshMaterial3d<MtoonMaterial>,
            &Mesh3d,
            Option<&OutlineChild>,
            Option<&SkinnedMesh>,
            Option<&MeshMorphWeights>,
            Option<&RenderLayers>,
        ),
        With<OutlineSync>,
    >,
    outline_children: Query<(&MeshMaterial3d<OutlineMaterial>, Option<&RenderLayers>)>,
) {
    for (entity, mtoon_handle, mesh, child, skin, morph, layers) in &query {
        let Some(mtoon) = mtoons.get(mtoon_handle.id()) else {
            continue;
        };

        if mtoon.outline_mode == VrmOutlineMode::None {
            if let Some(child) = child {
                commands.entity(child.0).try_despawn();
                commands.entity(entity).remove::<OutlineChild>();
            }
            continue;
        }

        let params = OutlineMaterial::from_mtoon(mtoon);

        if let Some(child) = child {
            let Ok((material, child_layers)) = outline_children.get(child.0) else {
                continue;
            };
            if outlines
                .get(material.id())
                .is_some_and(|existing| *existing != params)
                && let Some(mut existing) = outlines.get_mut(material.id())
            {
                *existing = params;
            }
            // The child dies with its parent, which may be despawned later this
            // frame.
            if layers != child_layers {
                match layers {
                    Some(layers) => {
                        commands.entity(child.0).try_insert(layers.clone());
                    }
                    None => {
                        commands.entity(child.0).try_remove::<RenderLayers>();
                    }
                }
            }
            continue;
        }

        let Some(shared_mesh) = shared_outline_mesh(mesh.id(), &mut meshes, &mut mesh_cache) else {
            continue;
        };

        let mesh_handle = shared_mesh.0.clone();
        let handle = outlines.add(params);
        let skin = skin.cloned();
        let morph = morph.cloned();
        let layers = layers.cloned();

        // Spawned only if the mesh still exists when the command applies, so a
        // mesh despawned this frame leaves no orphaned outline behind.
        commands
            .entity(entity)
            .queue_silenced(move |mut parent: EntityWorldMut| {
                let child = parent.world_scope(|world| {
                    let mut child = world.spawn((
                        Mesh3d(mesh_handle),
                        SharedOutlineMesh {
                            _handle: shared_mesh,
                        },
                        MeshMaterial3d(handle),
                        Transform::default(),
                    ));
                    if let Some(skin) = skin {
                        child.insert(skin);
                    }
                    if let Some(morph) = morph {
                        child.insert(morph);
                    }
                    if let Some(layers) = layers {
                        child.insert(layers);
                    }
                    child.id()
                });
                parent.insert(OutlineChild(child)).add_child(child);
            });
    }
}

/// Clones a mesh, replacing its normals with position-welded averaged normals
/// so an inverted-hull outline extrudes without splitting at hard edges.
fn smooth_outline_mesh(mesh: &Mesh) -> Option<Mesh> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    let Some(VertexAttributeValues::Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
    else {
        return None;
    };
    if positions.len() != normals.len() {
        return None;
    }

    let quantize = |p: &[f32; 3]| {
        [
            (p[0] * 1024.0).round() as i32,
            (p[1] * 1024.0).round() as i32,
            (p[2] * 1024.0).round() as i32,
        ]
    };

    let mut sums: HashMap<[i32; 3], Vec3> = HashMap::new();
    for (p, n) in positions.iter().zip(normals) {
        *sums.entry(quantize(p)).or_default() += Vec3::from_array(*n);
    }

    let smoothed = positions
        .iter()
        .zip(normals)
        .map(|(p, n)| {
            let sum = sums[&quantize(p)];
            if sum.length_squared() > 1.0e-8 {
                sum.normalize().to_array()
            } else {
                *n
            }
        })
        .collect::<Vec<_>>();

    let mut outline = mesh.clone();
    outline.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    outline.insert_attribute(Mesh::ATTRIBUTE_NORMAL, smoothed);
    Some(outline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_share_one_generated_mesh_and_release_cache_ownership() {
        let mut meshes = Assets::<Mesh>::default();
        let source = meshes.add(Mesh::from(Cuboid::default()));
        let mut cache = OutlineMeshCache::default();
        let first = shared_outline_mesh(source.id(), &mut meshes, &mut cache).unwrap();
        let second = shared_outline_mesh(source.id(), &mut meshes, &mut cache).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            meshes.len(),
            2,
            "a shared source gets only one generated outline"
        );
        let weak = Arc::downgrade(&first);
        drop(first);
        drop(second);
        assert!(
            weak.upgrade().is_none(),
            "cache must not retain mesh handles"
        );
        cache.0.retain(|_, handle| handle.strong_count() > 0);
        assert!(cache.0.is_empty());
    }

    #[test]
    fn direct_outline_morph_weights_follow_animated_parent() {
        let mut app = App::new();
        app.add_systems(PostUpdate, sync_outline_morph);
        let child = app
            .world_mut()
            .spawn((
                MeshMaterial3d::<OutlineMaterial>(Handle::default()),
                MeshMorphWeights::Value {
                    weights: vec![0.0, 0.0],
                },
            ))
            .id();
        let parent = app
            .world_mut()
            .spawn((
                OutlineChild(child),
                MeshMorphWeights::Value {
                    weights: vec![0.3, 0.8],
                },
            ))
            .id();
        app.update();
        match app.world().get::<MeshMorphWeights>(child).unwrap() {
            MeshMorphWeights::Value { weights } => assert_eq!(weights, &[0.3, 0.8]),
            _ => panic!("expected direct weights"),
        }
        app.world_mut()
            .entity_mut(parent)
            .insert(MeshMorphWeights::Value {
                weights: vec![0.7, 0.1],
            });
        app.update();
        match app.world().get::<MeshMorphWeights>(child).unwrap() {
            MeshMorphWeights::Value { weights } => assert_eq!(weights, &[0.7, 0.1]),
            _ => panic!("expected direct weights"),
        }
    }
}
