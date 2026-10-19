//! Shares identical immutable CPU morph buffers, keeping all values available
//! for rendering, picking, late derived meshes and device re-extraction.
use bevy::{asset::AssetEvent, mesh::morph::MorphAttributes, prelude::*};
use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::{Arc, Weak},
};

pub(crate) struct MorphSharingPlugin;

impl Plugin for MorphSharingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MorphSources>()
            .add_systems(Last, share_morphs);
    }
}

#[derive(Resource, Default)]
struct MorphSources {
    buckets: HashMap<(usize, u64), Vec<Weak<Vec<MorphAttributes>>>>,
}

/// Allocated CPU morph bytes (logical total, unique shared backing). Intended
/// for opt-in measurements, not the render frame's normal hot path.
pub(crate) fn cpu_bytes(meshes: &Assets<Mesh>) -> (usize, usize) {
    let mut seen = HashSet::new();
    let (mut logical, mut unique) = (0, 0);
    for (_, mesh) in meshes.iter() {
        if let Ok(source) = mesh.try_shared_morph_targets() {
            let bytes = source.capacity() * size_of::<MorphAttributes>();
            logical += bytes;
            if seen.insert(Arc::as_ptr(&source)) {
                unique += bytes;
            }
        }
    }
    (logical, unique)
}

fn key(values: &[MorphAttributes]) -> (usize, u64) {
    // A cheap candidate filter only. Every candidate is confirmed byte-for-byte.
    let mut hash = DefaultHasher::new();
    if !values.is_empty() {
        for index in [0, values.len() / 2, values.len() - 1] {
            bytemuck::bytes_of(&values[index]).hash(&mut hash);
        }
    }
    (values.len(), hash.finish())
}

impl MorphSources {
    fn canonical(&mut self, source: &Arc<Vec<MorphAttributes>>) -> Arc<Vec<MorphAttributes>> {
        let bucket = self.buckets.entry(key(source)).or_default();
        for existing in bucket.iter().filter_map(Weak::upgrade) {
            if Arc::ptr_eq(source, &existing)
                || bytemuck::cast_slice::<_, u8>(source) == bytemuck::cast_slice::<_, u8>(&existing)
            {
                return existing;
            }
        }
        bucket.push(Arc::downgrade(source));
        Arc::clone(source)
    }

    fn prune(&mut self) {
        self.buckets.retain(|_, bucket| {
            bucket.retain(|source| source.strong_count() != 0);
            !bucket.is_empty()
        });
    }
}

fn share_morphs(
    mut events: MessageReader<AssetEvent<Mesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sources: ResMut<MorphSources>,
) {
    let mut candidates = HashSet::new();
    let mut changed = false;
    for event in events.read() {
        changed = true;
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = event {
            candidates.insert(*id);
        }
    }
    if !changed {
        return;
    }
    sources.prune();
    for id in candidates {
        let Some(source) = meshes
            .get(id)
            .and_then(|mesh| mesh.try_shared_morph_targets().ok())
        else {
            continue;
        };
        let canonical = sources.canonical(&source);
        if !Arc::ptr_eq(&source, &canonical) {
            // Values are exactly unchanged: replacing ownership must not mark
            // the mesh modified, create an event loop or trigger a GPU upload.
            if let Some(mesh) = meshes.get_mut_untracked(id) {
                mesh.try_set_shared_morph_targets(canonical)
                    .expect("mesh extraction state cannot change within this system");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::{AssetApp, AssetPlugin, RenderAssetUsages},
        mesh::PrimitiveTopology,
    };

    fn mesh(value: f32) -> Mesh {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.set_morph_targets(vec![MorphAttributes {
            position: Vec3::splat(value),
            ..Default::default()
        }]);
        mesh
    }

    #[test]
    fn cloning_shares_backing_and_replacement_is_isolated() {
        let original = mesh(1.0);
        let mut clone = original.clone();
        assert!(Arc::ptr_eq(
            &original.try_shared_morph_targets().unwrap(),
            &clone.try_shared_morph_targets().unwrap()
        ));
        clone.set_morph_targets(mesh(2.0).morph_targets().unwrap().clone());
        assert_eq!(original.morph_targets().unwrap()[0].position, Vec3::ONE);
        assert_eq!(clone.morph_targets().unwrap()[0].position, Vec3::splat(2.0));
        assert_eq!(
            original.get_morph_targets().unwrap(),
            original.morph_targets().unwrap()
        );
    }

    #[test]
    fn exact_dedup_preserves_gpu_bytes_and_weak_cache_releases() {
        let mut sources = MorphSources::default();
        let a = mesh(1.0).try_shared_morph_targets().unwrap();
        let b = mesh(1.0).try_shared_morph_targets().unwrap();
        let different = mesh(2.0).try_shared_morph_targets().unwrap();
        let canonical = sources.canonical(&a);
        let shared = sources.canonical(&b);
        assert!(Arc::ptr_eq(&canonical, &shared));
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&b),
            bytemuck::cast_slice::<_, u8>(&shared)
        );
        assert!(!Arc::ptr_eq(&canonical, &sources.canonical(&different)));
        drop((a, b, different, canonical, shared));
        sources.prune();
        assert!(sources.buckets.is_empty());
    }

    #[test]
    fn candidate_collisions_and_float_bit_patterns_never_change_gpu_bytes() {
        let mut sources = MorphSources::default();
        let values = vec![MorphAttributes::default(); 5];
        let a = Arc::new(values.clone());
        let canonical = sources.canonical(&a);
        let mut collision = values.clone();
        collision[1].position.x = 1.0; // Not sampled by the candidate key.
        let collision = Arc::new(collision);
        assert_eq!(key(&a), key(&collision));
        assert!(!Arc::ptr_eq(&canonical, &sources.canonical(&collision)));
        let mut negative_zero = values;
        negative_zero[1].position.x = -0.0;
        assert!(!Arc::ptr_eq(
            &canonical,
            &sources.canonical(&Arc::new(negative_zero))
        ));
        let mut nan = vec![MorphAttributes::default(); 5];
        nan[1].position.x = f32::from_bits(0x7fc00001);
        let nan_a = Arc::new(nan.clone());
        let nan_canonical = sources.canonical(&nan_a);
        assert!(Arc::ptr_eq(
            &nan_canonical,
            &sources.canonical(&Arc::new(nan.clone()))
        ));
        nan[1].position.x = f32::from_bits(0x7fc00002);
        assert!(!Arc::ptr_eq(
            &nan_canonical,
            &sources.canonical(&Arc::new(nan))
        ));
    }

    #[test]
    fn extracted_mesh_rejects_shared_access_and_reflected_clone_keeps_values() {
        let mut mesh = mesh(1.0);
        let backing = mesh.try_shared_morph_targets().unwrap();
        let reflected = mesh.reflect_clone().unwrap();
        assert_eq!(reflected.downcast_ref::<Mesh>().unwrap(), &mesh);
        let extracted = mesh.take_gpu_data().unwrap();
        assert!(mesh.try_shared_morph_targets().is_err());
        assert!(mesh.try_set_shared_morph_targets(backing).is_err());
        assert_eq!(extracted.morph_targets().unwrap()[0].position, Vec3::ONE);
    }

    #[test]
    fn event_pass_deduplicates_without_modification_events_and_accepts_replacement() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .add_plugins(MorphSharingPlugin);
        let a = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(mesh(1.0));
        let b = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(mesh(1.0));
        app.update();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let aa = meshes.get(&a).unwrap().try_shared_morph_targets().unwrap();
        let bb = meshes.get(&b).unwrap().try_shared_morph_targets().unwrap();
        assert!(Arc::ptr_eq(&aa, &bb));
        let (logical, unique) = cpu_bytes(meshes);
        assert_eq!(logical, unique * 2);
        app.update();
        // The real AssetPlugin publishes any tracked modifications here.
        app.update();
        let messages = app.world().resource::<Messages<AssetEvent<Mesh>>>();
        let mut cursor = messages.get_cursor();
        assert!(
            !cursor
                .read(messages)
                .any(|event| matches!(event, AssetEvent::Modified { .. }))
        );
        app.world_mut()
            .resource_mut::<Assets<Mesh>>()
            .insert(b.id(), mesh(2.0))
            .unwrap();
        app.update();
        let meshes = app.world().resource::<Assets<Mesh>>();
        assert_eq!(
            meshes.get(&a).unwrap().morph_targets().unwrap()[0].position,
            Vec3::ONE
        );
        assert_eq!(
            meshes.get(&b).unwrap().morph_targets().unwrap()[0].position,
            Vec3::splat(2.0)
        );
        app.world_mut()
            .resource_mut::<Assets<Mesh>>()
            .insert(a.id(), mesh(2.0))
            .unwrap();
        app.update();
        let meshes = app.world().resource::<Assets<Mesh>>();
        assert!(Arc::ptr_eq(
            &meshes.get(&a).unwrap().try_shared_morph_targets().unwrap(),
            &meshes.get(&b).unwrap().try_shared_morph_targets().unwrap()
        ));
    }
}
