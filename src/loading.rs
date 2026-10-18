use bevy::{
    asset::{AssetPath, LoadState, RecursiveDependencyLoadState, UntypedAssetId},
    gltf::Gltf,
    prelude::*,
    world_serialization::WorldInstanceReady,
};
use bevy_vrm::{VrmPlugins, instance::VrmInstance, loader::Vrm};

pub struct VrmLoaderPlugin;

impl Plugin for VrmLoaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(VrmPlugins)
            .init_resource::<AvatarLoadState>()
            .add_message::<LoadAvatar>()
            .add_message::<UnloadAvatar>()
            .add_observer(scene_ready)
            .add_systems(
                Update,
                (unload_avatar, request_avatar, check_loading).chain(),
            );
    }
}

/// Paths follow Bevy's asset source rules. The newest request wins.
#[derive(Message)]
pub struct LoadAvatar(pub AssetPath<'static>);

/// Release the visible model and any in-flight replacement, including their
/// child entities and asset handles. A LoadAvatar in the same frame wins.
#[derive(Message)]
pub struct UnloadAvatar;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LoadStatus {
    #[default]
    Empty,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Resource, Default)]
pub struct AvatarLoadState {
    pub status: LoadStatus,
    pub requested_path: Option<String>,
    /// The visible avatar. A failed replacement leaves this entity intact.
    pub active: Option<Entity>,
    pending: Option<PendingAvatar>,
}

struct PendingAvatar {
    entity: Entity,
    handle: ModelHandle,
    scene_ready: bool,
    scene_attached: bool,
}

enum ModelHandle {
    Vrm(Handle<Vrm>),
    Gltf(Handle<Gltf>),
}
impl ModelHandle {
    fn id(&self) -> UntypedAssetId {
        match self {
            Self::Vrm(h) => h.id().untyped(),
            Self::Gltf(h) => h.id().untyped(),
        }
    }
}

fn unload_avatar(
    mut requests: MessageReader<UnloadAvatar>,
    mut commands: Commands,
    mut state: ResMut<AvatarLoadState>,
) {
    if requests.read().count() == 0 {
        return;
    }
    if let Some(pending) = state.pending.take() {
        // Children uses linked despawning; Bevy cancels asset loads when their
        // last external handle is dropped and releases unused assets normally.
        commands.entity(pending.entity).try_despawn();
    }
    if let Some(active) = state.active.take() {
        commands.entity(active).try_despawn();
    }
    state.requested_path = None;
    state.status = LoadStatus::Empty;
}

fn request_avatar(
    mut requests: MessageReader<LoadAvatar>,
    mut commands: Commands,
    server: Res<AssetServer>,
    mut state: ResMut<AvatarLoadState>,
) {
    let Some(request) = requests.read().last() else {
        return;
    };
    start_slot(&mut commands, &server, &mut state, request.0.clone());
}

pub(crate) fn start_slot(
    commands: &mut Commands,
    server: &AssetServer,
    state: &mut AvatarLoadState,
    path: AssetPath<'static>,
) {
    if let Some(old) = state.pending.take() {
        commands.entity(old.entity).try_despawn();
    }
    let handle = if matches!(
        path.path().extension().and_then(|e| e.to_str()),
        Some("glb" | "gltf")
    ) {
        ModelHandle::Gltf(server.load(path.clone()))
    } else {
        ModelHandle::Vrm(server.load(path.clone()))
    };
    let entity = commands
        .spawn((
            Name::new("VRM avatar"),
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
            Visibility::Hidden,
        ))
        .id();
    if let ModelHandle::Vrm(handle) = &handle {
        commands.entity(entity).insert(VrmInstance(handle.clone()));
    }
    state.pending = Some(PendingAvatar {
        entity,
        handle,
        scene_ready: false,
        scene_attached: false,
    });
    state.requested_path = Some(path.to_string());
    state.status = LoadStatus::Loading;
}

fn scene_ready(event: On<WorldInstanceReady>, mut state: ResMut<AvatarLoadState>) {
    if let Some(pending) = &mut state.pending
        && pending.entity == event.entity
    {
        pending.scene_ready = true;
    }
}

fn check_loading(
    mut commands: Commands,
    server: Res<AssetServer>,
    assets: Res<Assets<Vrm>>,
    gltfs: Option<Res<Assets<Gltf>>>,
    mut state: ResMut<AvatarLoadState>,
) {
    advance_slot(
        &mut commands,
        &server,
        &assets,
        gltfs.as_deref(),
        &mut state,
    );
}

pub(crate) fn advance_slot(
    commands: &mut Commands,
    server: &AssetServer,
    assets: &Assets<Vrm>,
    gltfs: Option<&Assets<Gltf>>,
    state: &mut AvatarLoadState,
) {
    if state.pending.is_none() {
        return;
    }
    let pending = state.pending.as_mut().unwrap();
    let gltf = match &pending.handle {
        ModelHandle::Vrm(handle) => assets.get(handle).map(|a| &a.gltf),
        ModelHandle::Gltf(handle) => gltfs.and_then(|assets| assets.get(handle)),
    };
    if matches!(pending.handle, ModelHandle::Gltf(_))
        && !pending.scene_attached
        && let Some(scene) =
            gltf.and_then(|gltf| gltf.default_scene.as_ref().or(gltf.scenes.first()))
    {
        commands
            .entity(pending.entity)
            .insert(WorldAssetRoot(scene.clone()));
        pending.scene_attached = true;
    }
    let failure = match server.get_load_state(pending.handle.id()) {
        Some(LoadState::Failed(error)) => Some(error.to_string()),
        _ => match server.get_recursive_dependency_load_state(pending.handle.id()) {
            Some(RecursiveDependencyLoadState::Failed(error)) => Some(error.to_string()),
            _ => gltf.and_then(|asset| {
                (asset.default_scene.is_none() && asset.scenes.is_empty())
                    .then(|| "Model contains no scene".to_owned())
            }),
        },
    };
    if let Some(error) = failure {
        let pending = state.pending.take().unwrap();
        commands.entity(pending.entity).try_despawn();
        error!("VRM load failed: {error}");
        state.status = LoadStatus::Failed(error);
    } else if pending.scene_ready && server.is_loaded_with_dependencies(pending.handle.id()) {
        let pending = state.pending.take().unwrap();
        if let Some(previous) = state.active.replace(pending.entity) {
            commands.entity(previous).try_despawn();
        }
        commands.entity(pending.entity).insert(Visibility::Visible);
        state.status = LoadStatus::Ready;
        info!(
            "VRM_READY {}",
            state.requested_path.as_deref().unwrap_or_default()
        );
    }
}

impl AvatarLoadState {
    pub(crate) fn mark_ready(&mut self, entity: Entity) {
        if let Some(pending) = &mut self.pending
            && pending.entity == entity
        {
            pending.scene_ready = true;
        }
    }
    pub(crate) fn pending_entity(&self) -> Option<Entity> {
        self.pending.as_ref().map(|pending| pending.entity)
    }
    pub(crate) fn release(&mut self, commands: &mut Commands) {
        if let Some(pending) = self.pending.take() {
            commands.entity(pending.entity).try_despawn();
        }
        if let Some(active) = self.active.take() {
            commands.entity(active).try_despawn();
        }
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::{AssetLoader, LoadContext, io::Reader};

    #[derive(TypePath)]
    struct FailingLoader;
    impl AssetLoader for FailingLoader {
        type Asset = Vrm;
        type Settings = ();
        type Error = std::io::Error;
        async fn load(
            &self,
            _: &mut dyn Reader,
            _: &(),
            _: &mut LoadContext<'_>,
        ) -> Result<Vrm, Self::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "deliberate fixture failure",
            ))
        }
        fn extensions(&self) -> &[&str] {
            &["broken"]
        }
    }

    #[test]
    fn failed_replacement_preserves_active_avatar_and_cleans_pending_entity() {
        let dir = std::env::temp_dir().join(format!("vrm-loader-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.broken"), b"bad asset").unwrap();
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: dir.to_string_lossy().into_owned(),
                ..default()
            },
        ))
        .init_asset::<Vrm>()
        .register_asset_loader(FailingLoader)
        .init_resource::<AvatarLoadState>()
        .add_message::<LoadAvatar>()
        .add_systems(Update, (request_avatar, check_loading).chain());
        app.finish();
        app.cleanup();
        let active = app.world_mut().spawn_empty().id();
        let active_child = app.world_mut().spawn(ChildOf(active)).id();
        let other_slot_root = app.world_mut().spawn_empty().id();
        let other_slot_child = app.world_mut().spawn(ChildOf(other_slot_root)).id();
        let superseded = app.world_mut().spawn_empty().id();
        let superseded_child = app.world_mut().spawn(ChildOf(superseded)).id();
        {
            let mut state = app.world_mut().resource_mut::<AvatarLoadState>();
            state.active = Some(active);
            state.pending = Some(PendingAvatar {
                entity: superseded,
                handle: ModelHandle::Vrm(Handle::default()),
                scene_ready: false,
                scene_attached: false,
            });
        }
        // Only the last request in a frame should be spawned.
        app.world_mut()
            .write_message(LoadAvatar("superseded.broken".into()));
        app.world_mut()
            .write_message(LoadAvatar("bad.broken".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            if matches!(
                app.world().resource::<AvatarLoadState>().status,
                LoadStatus::Failed(_)
            ) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "loader did not report failure"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let state = app.world().resource::<AvatarLoadState>();
        assert_eq!(state.active, Some(active));
        assert_eq!(state.requested_path.as_deref(), Some("bad.broken"));
        assert!(state.pending.is_none());
        assert!(app.world().get_entity(active).is_ok());
        assert!(app.world().get_entity(active_child).is_ok());
        assert!(app.world().get_entity(other_slot_root).is_ok());
        assert!(app.world().get_entity(other_slot_child).is_ok());
        assert!(app.world().get_entity(superseded).is_err());
        assert!(app.world().get_entity(superseded_child).is_err());
        assert_eq!(
            app.world_mut()
                .query::<&VrmInstance>()
                .iter(app.world())
                .count(),
            0
        );
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unload_releases_active_and_pending_hierarchies_and_resets_state() {
        let mut app = App::new();
        app.init_resource::<AvatarLoadState>()
            .add_message::<UnloadAvatar>()
            .add_systems(Update, unload_avatar);
        let active = app.world_mut().spawn_empty().id();
        let active_child = app.world_mut().spawn(ChildOf(active)).id();
        let pending = app.world_mut().spawn_empty().id();
        let pending_child = app.world_mut().spawn(ChildOf(pending)).id();
        {
            let mut state = app.world_mut().resource_mut::<AvatarLoadState>();
            state.active = Some(active);
            state.pending = Some(PendingAvatar {
                entity: pending,
                handle: ModelHandle::Vrm(Handle::default()),
                scene_ready: false,
                scene_attached: false,
            });
            state.status = LoadStatus::Loading;
            state.requested_path = Some("loading.vrm".into());
        }
        app.world_mut().write_message(UnloadAvatar);
        app.update();
        for entity in [active, active_child, pending, pending_child] {
            assert!(app.world().get_entity(entity).is_err());
        }
        let state = app.world().resource::<AvatarLoadState>();
        assert!(state.active.is_none());
        assert!(state.pending.is_none());
        assert!(state.requested_path.is_none());
        assert_eq!(state.status, LoadStatus::Empty);
    }
}
