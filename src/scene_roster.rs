//! Bounded additional scene occupants. Chat and speech remain focused-character only.
use crate::data::Library;
use crate::loading::{AvatarLoadState, advance_slot, start_slot};
use bevy::{gltf::Gltf, prelude::*, world_serialization::WorldInstanceReady};
use bevy_vrm::loader::Vrm;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread,
    time::Duration,
};

const MAX_FILE_BYTES: u64 = 16 * 1024;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMember {
    character: Uuid,
    position: [f32; 3],
    yaw: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedRoster {
    version: u32,
    members: Vec<SavedMember>,
}
impl Default for SavedRoster {
    fn default() -> Self {
        Self {
            version: 1,
            members: Vec::new(),
        }
    }
}
fn decode(bytes: &[u8]) -> Result<SavedRoster, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Scene roster exceeds size limit".into());
    }
    let saved: SavedRoster = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if saved.version != 1 || saved.members.len() > MAX_ADDITIONAL_MEMBERS {
        return Err("Unsupported scene roster version or member count".into());
    }
    let mut seen = std::collections::HashSet::new();
    for member in &saved.members {
        if !seen.insert(member.character)
            || !member.yaw.is_finite()
            || member
                .position
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 10.)
        {
            return Err("Invalid scene roster placement or duplicate member".into());
        }
    }
    Ok(saved)
}
fn read_saved(path: &Path) -> Result<SavedRoster, String> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SavedRoster::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode(&bytes)
}
fn write_saved(path: &Path, saved: &SavedRoster) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
    decode(&bytes)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::durable_file::write(path, &bytes).map_err(|e| e.to_string())
}
struct Persistence {
    sender: Option<mpsc::SyncSender<SavedRoster>>,
    loaded: Mutex<mpsc::Receiver<Result<SavedRoster, String>>>,
    errors: Mutex<mpsc::Receiver<String>>,
    worker: Option<thread::JoinHandle<()>>,
    observed: SavedRoster,
    pending: Option<SavedRoster>,
    loading: bool,
}
impl Drop for Persistence {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            if let Some(saved) = self.pending.take() {
                let _ = sender.send(saved);
            }
            drop(sender);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
use uuid::Uuid;

pub const MAX_ADDITIONAL_MEMBERS: usize = 3;
#[derive(Component)]
pub struct SceneMember(pub Uuid);
pub struct SceneSlot {
    pub character: Uuid,
    pub name: String,
    pub position: Vec3,
    pub yaw: f32,
    pub load: AvatarLoadState,
    path: PathBuf,
    requested: bool,
    remove: bool,
}
#[derive(Resource, Default)]
pub struct SceneRoster {
    pub slots: Vec<SceneSlot>,
    /// Changes only when visible geometry/placement changes, for camera framing.
    pub revision: u64,
    persistence: Option<Persistence>,
    edit_epoch: u64,
    pub persistence_error: Option<String>,
}
impl SceneRoster {
    /// Starts bounded disk loading on a worker. Call `poll` with the current library each update.
    pub fn load(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<SavedRoster>(1);
        let (loaded_tx, loaded) = mpsc::channel();
        let (error_tx, errors) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = loaded_tx.send(read_saved(&path));
            while let Ok(mut saved) = receiver.recv() {
                loop {
                    match receiver.recv_timeout(Duration::from_millis(200)) {
                        Ok(newer) => saved = newer,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            if let Err(error) = write_saved(&path, &saved) {
                                let _ = error_tx.send(error);
                            }
                            return;
                        }
                    }
                }
                if let Err(error) = write_saved(&path, &saved) {
                    let _ = error_tx.send(error);
                }
            }
        });
        Self {
            persistence: Some(Persistence {
                sender: Some(sender),
                loaded: Mutex::new(loaded),
                errors: Mutex::new(errors),
                worker: Some(worker),
                observed: SavedRoster::default(),
                pending: None,
                loading: true,
            }),
            ..Default::default()
        }
    }
    fn snapshot(&self) -> SavedRoster {
        SavedRoster {
            version: 1,
            members: self
                .slots
                .iter()
                .filter(|s| !s.remove)
                .map(|s| SavedMember {
                    character: s.character,
                    position: s.position.to_array(),
                    yaw: s.yaw,
                })
                .collect(),
        }
    }
    /// Nonblocking frame polling. Library resolves persisted UUIDs to current names/models.
    pub fn poll(&mut self, library: &Library) {
        let Some(mut persistence) = self.persistence.take() else {
            return;
        };
        if persistence.loading {
            let result = persistence.loaded.lock().unwrap().try_recv();
            match result {
                Ok(result) => {
                    persistence.loading = false;
                    match result {
                        Ok(saved)
                            if self.edit_epoch == 0
                                && self.snapshot() == SavedRoster::default() =>
                        {
                            for member in &saved.members {
                                if let Some(character) = library.characters.iter().find(|c| {
                                    c.id == member.character && Some(c.id) != library.selected
                                }) {
                                    let _ = self.add(
                                        character.id,
                                        character.name.clone(),
                                        character.model.clone(),
                                    );
                                    if let Some(slot) = self.slots.last_mut() {
                                        slot.position = Vec3::from_array(member.position);
                                        slot.yaw = member.yaw;
                                    }
                                }
                            }
                            persistence.observed = saved;
                        }
                        Ok(_) => {}
                        Err(error) => self.persistence_error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    persistence.loading = false;
                    self.persistence_error = Some("Scene persistence worker stopped".into());
                }
            }
        }
        if let Ok(error) = persistence.errors.lock().unwrap().try_recv() {
            self.persistence_error = Some(error);
        }
        if !persistence.loading {
            let saved = self.snapshot();
            if saved != persistence.observed {
                // Invalid edits never reach disk; retain the previous valid file.
                if serde_json::to_vec(&saved)
                    .ok()
                    .and_then(|bytes| decode(&bytes).ok())
                    .is_some()
                {
                    persistence.observed = saved.clone();
                    persistence.pending = Some(saved);
                }
            }
            if let Some(saved) = persistence.pending.take()
                && let Some(sender) = &persistence.sender
            {
                match sender.try_send(saved) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(saved)) => persistence.pending = Some(saved),
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        self.persistence_error = Some("Scene persistence worker stopped".into())
                    }
                }
            }
        }
        self.persistence = Some(persistence);
    }

    pub fn add(&mut self, character: Uuid, name: String, path: PathBuf) -> Result<(), String> {
        if self
            .slots
            .iter()
            .any(|s| s.character == character && !s.remove)
        {
            return Err("Character is already in this scene".into());
        }
        if self.slots.len() >= MAX_ADDITIONAL_MEMBERS {
            return Err("The scene supports one focused character and three companions".into());
        }
        self.edit_epoch = self.edit_epoch.wrapping_add(1);
        let x = [-1.2, 1.2, 2.4][self.slots.len()];
        self.slots.push(SceneSlot {
            character,
            name,
            position: Vec3::new(x, 0., 0.),
            yaw: std::f32::consts::PI,
            load: Default::default(),
            path,
            requested: true,
            remove: false,
        });
        Ok(())
    }
    pub fn remove(&mut self, character: Uuid) {
        for slot in &mut self.slots {
            if slot.character == character {
                if !slot.remove {
                    self.edit_epoch = self.edit_epoch.wrapping_add(1);
                }
                slot.remove = true;
            }
        }
    }
    pub fn replace(&mut self, character: Uuid, path: PathBuf) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.character == character) {
            slot.path = path;
            slot.requested = true;
        }
    }
    pub fn contains(&self, character: Uuid) -> bool {
        self.slots
            .iter()
            .any(|s| s.character == character && !s.remove)
    }
}
pub struct SceneRosterPlugin;
impl Plugin for SceneRosterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneRoster>()
            .add_observer(ready)
            .add_systems(Update, update_roster);
    }
}
fn ready(event: On<WorldInstanceReady>, mut roster: ResMut<SceneRoster>) {
    for slot in &mut roster.slots {
        slot.load.mark_ready(event.entity);
    }
}
fn update_roster(
    mut commands: Commands,
    server: Res<AssetServer>,
    assets: Res<Assets<Vrm>>,
    gltfs: Option<Res<Assets<Gltf>>>,
    mut roster: ResMut<SceneRoster>,
    mut transforms: Query<&mut Transform, With<SceneMember>>,
) {
    let mut changed = false;
    for slot in &mut roster.slots {
        if slot.remove {
            slot.load.release(&mut commands);
            changed = true;
            continue;
        }
        if slot.requested {
            start_slot(
                &mut commands,
                &server,
                &mut slot.load,
                bevy::asset::AssetPath::from_path(&slot.path).into_owned(),
            );
            if let Some(entity) = slot.load.pending_entity() {
                commands.entity(entity).insert(SceneMember(slot.character));
            }
            slot.requested = false;
        }
        let before = slot.load.active;
        advance_slot(
            &mut commands,
            &server,
            &assets,
            gltfs.as_deref(),
            &mut slot.load,
        );
        changed |= before != slot.load.active;
        let position = if slot.position.is_finite() {
            slot.position.clamp(Vec3::splat(-10.), Vec3::splat(10.))
        } else {
            Vec3::ZERO
        };
        let yaw = if slot.yaw.is_finite() {
            slot.yaw
        } else {
            std::f32::consts::PI
        };
        let transform =
            Transform::from_translation(position).with_rotation(Quat::from_rotation_y(yaw));
        for entity in [slot.load.active, slot.load.pending_entity()]
            .into_iter()
            .flatten()
        {
            if let Ok(mut current) = transforms.get_mut(entity) {
                if *current != transform {
                    *current = transform;
                    changed = true;
                }
            } else {
                commands.entity(entity).insert(transform);
            }
        }
    }
    roster.slots.retain(|slot| !slot.remove);
    if changed {
        roster.revision = roster.revision.wrapping_add(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persisted_schema_rejects_old_malformed_oversized_and_invalid_data() {
        assert!(decode(br#"{"version":0,"members":[]}"#).is_err());
        assert!(decode(br#"{"members":[]}"#).is_err());
        assert!(decode(b"broken").is_err());
        assert!(decode(&vec![b' '; MAX_FILE_BYTES as usize + 1]).is_err());
        let member = SavedMember {
            character: Uuid::new_v4(),
            position: [0.; 3],
            yaw: 0.,
        };
        let mut saved = SavedRoster {
            version: 1,
            members: vec![member.clone(); 4],
        };
        assert!(decode(&serde_json::to_vec(&saved).unwrap()).is_err());
        saved.members.truncate(2);
        assert!(decode(&serde_json::to_vec(&saved).unwrap()).is_err());
        saved.members.truncate(1);
        saved.members[0].position[0] = 11.;
        assert!(decode(&serde_json::to_vec(&saved).unwrap()).is_err());
        saved.members[0].position[0] = f32::NAN;
        assert!(decode(&serde_json::to_vec(&saved).unwrap()).is_err());
    }

    #[test]
    fn disk_roundtrip_and_drop_drains_latest_snapshot() {
        let directory = std::env::temp_dir().join(format!("waifu-roster-{}", Uuid::new_v4()));
        let path = directory.join("scene.json");
        let id = Uuid::new_v4();
        let mut roster = SceneRoster::load(path.clone());
        roster
            .add(id, "name-never-saved".into(), "secret-path.vrm".into())
            .unwrap();
        // Wait only in this test for initial worker IO; edits must win the startup race.
        for _ in 0..100 {
            roster.poll(&Library::default());
            if !roster.persistence.as_ref().unwrap().loading {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(!roster.persistence.as_ref().unwrap().loading);
        roster.slots[0].position = Vec3::new(1., 2., 3.);
        roster.poll(&Library::default());
        let expected = roster.snapshot();
        drop(roster);
        assert_eq!(read_saved(&path).unwrap(), expected);
        let bytes = std::fs::read_to_string(&path).unwrap();
        assert!(!bytes.contains("secret-path"));
        assert!(!bytes.contains("name-never-saved"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_file_is_preserved_until_a_real_edit() {
        let directory = std::env::temp_dir().join(format!("waifu-roster-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("scene.json");
        let original = b"{malformed scene roster";
        std::fs::write(&path, original).unwrap();
        let mut roster = SceneRoster::load(path.clone());
        for _ in 0..100 {
            roster.poll(&Library::default());
            if !roster.persistence.as_ref().unwrap().loading {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(roster.persistence_error.is_some());
        for _ in 0..10 {
            roster.poll(&Library::default());
        }
        assert!(roster.persistence.as_ref().unwrap().pending.is_none());
        drop(roster);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn old_startup_membership_never_overwrites_new_edits() {
        let directory = std::env::temp_dir().join(format!("waifu-roster-{}", Uuid::new_v4()));
        let path = directory.join("scene.json");
        let old = Uuid::new_v4();
        write_saved(
            &path,
            &SavedRoster {
                version: 1,
                members: vec![SavedMember {
                    character: old,
                    position: [0.; 3],
                    yaw: 0.,
                }],
            },
        )
        .unwrap();
        let new = Uuid::new_v4();
        let mut roster = SceneRoster::load(path.clone());
        roster.add(new, "new".into(), "new.vrm".into()).unwrap();
        for _ in 0..100 {
            roster.poll(&Library::default());
            if !roster.persistence.as_ref().unwrap().loading {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(roster.contains(new));
        assert!(!roster.contains(old));
        drop(roster);
        assert_eq!(read_saved(&path).unwrap().members[0].character, new);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn releasing_one_slot_preserves_other_hierarchy() {
        let mut app = App::new();
        app.init_resource::<SceneRoster>();
        let a = app.world_mut().spawn_empty().id();
        let a_child = app.world_mut().spawn(ChildOf(a)).id();
        let b = app.world_mut().spawn_empty().id();
        let b_child = app.world_mut().spawn(ChildOf(b)).id();
        {
            let mut roster = app.world_mut().resource_mut::<SceneRoster>();
            for entity in [a, b] {
                roster
                    .add(Uuid::new_v4(), "fixture".into(), "a.vrm".into())
                    .unwrap();
                roster.slots.last_mut().unwrap().load.active = Some(entity);
            }
        }
        app.add_systems(
            Update,
            |mut commands: Commands, mut roster: ResMut<SceneRoster>| {
                roster.slots[0].load.release(&mut commands);
            },
        );
        app.update();
        assert!(app.world().get_entity(a).is_err());
        assert!(app.world().get_entity(a_child).is_err());
        assert!(app.world().get_entity(b).is_ok());
        assert!(app.world().get_entity(b_child).is_ok());
        assert_eq!(
            app.world().resource::<SceneRoster>().slots[1].load.active,
            Some(b)
        );
    }

    #[test]
    fn roster_bounds_duplicates_and_removal_are_character_scoped() {
        let mut roster = SceneRoster::default();
        let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        for id in ids {
            roster.add(id, "fixture".into(), "a.vrm".into()).unwrap();
        }
        assert!(
            roster
                .add(ids[0], "duplicate".into(), "a.vrm".into())
                .is_err()
        );
        assert!(
            roster
                .add(Uuid::new_v4(), "overflow".into(), "a.vrm".into())
                .is_err()
        );
        roster.remove(ids[1]);
        assert!(roster.contains(ids[0]));
        assert!(!roster.contains(ids[1]));
        assert!(roster.contains(ids[2]));
        roster.replace(ids[0], "b.vrm".into());
        assert_eq!(roster.slots[0].path, PathBuf::from("b.vrm"));
        assert_eq!(roster.slots[2].path, PathBuf::from("a.vrm"));
    }
}
