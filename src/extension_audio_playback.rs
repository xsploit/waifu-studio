//! Host authority for one persistent extension incarnation; never a wire payload.
use super::*;
use crate::extension_protocol::AudioOutput;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPlaybackRoute {
    pub installation_id: Uuid,
    pub session: Uuid,
    pub process_generation: u64,
    pub target: Target,
    pub origin: IntegrationOrigin,
}
impl ExtensionRuntime {
    /// Capture only from a live persistent service with the separate audio grant.
    pub fn audio_playback_route(&self, id: Uuid, target: Target) -> Option<AudioPlaybackRoute> {
        target.validate().ok()?;
        let inner = self.inner();
        if inner.quiescing {
            return None;
        }
        let entry = inner.entries.get(&id)?;
        if !playback_allowed(entry) {
            return None;
        }
        Some(AudioPlaybackRoute {
            installation_id: id,
            session: entry.connection.session_id(),
            process_generation: entry.generation,
            target,
            origin: entry.connection.origin().clone(),
        })
    }
    /// Current avatar plus eligible processes in one read-only snapshot.
    pub fn current_audio_playback_routes(&self, character: Uuid) -> Vec<AudioPlaybackRoute> {
        let inner = self.inner();
        if inner.quiescing {
            return Vec::new();
        }
        let Some(target) = inner
            .current_target
            .filter(|target| target.uuid == character && target.validate().is_ok())
        else {
            return Vec::new();
        };
        inner
            .entries
            .iter()
            .filter(|(_, entry)| playback_allowed(entry))
            .map(|(id, entry)| AudioPlaybackRoute {
                installation_id: *id,
                session: entry.connection.session_id(),
                process_generation: entry.generation,
                target,
                origin: entry.connection.origin().clone(),
            })
            .collect()
    }
    /// Current eligible process incarnations; this is authority, not call readiness.
    pub fn audio_playback_routes(&self, target: Target) -> Vec<AudioPlaybackRoute> {
        if target.validate().is_err() {
            return Vec::new();
        }
        let inner = self.inner();
        if inner.quiescing {
            return Vec::new();
        }
        inner
            .entries
            .iter()
            .filter(|(_, entry)| playback_allowed(entry))
            .map(|(id, entry)| AudioPlaybackRoute {
                installation_id: *id,
                session: entry.connection.session_id(),
                process_generation: entry.generation,
                target,
                origin: entry.connection.origin().clone(),
            })
            .collect()
    }
    /// Teardown-only Cancel may refer to the previous avatar target. It never
    /// permits frames for that target or bypasses the live incarnation/grant.
    pub fn cancel_audio_playback(&self, route: &AudioPlaybackRoute, playback_id: Uuid) -> bool {
        self.publish_audio_output(
            route,
            Some(route.target),
            AudioOutput::Cancel {
                playback_id,
                target: route.target,
            },
        )
    }
    /// Fail closed on the captured process only; never disable/stop a replacement.
    pub fn abort_audio_playback(&self, route: &AudioPlaybackRoute) -> bool {
        let mut inner = self.inner();
        let Some(entry) = inner.entries.get_mut(&route.installation_id) else {
            return false;
        };
        if entry.test_parent.is_some()
            || entry.hook_target.is_some()
            || entry.generation != route.process_generation
            || entry.connection.session_id() != route.session
            || entry.connection.origin() != &route.origin
        {
            return false;
        }
        entry.connection.stop("Speech playback stopped");
        true
    }
    /// Queue only to the original live incarnation. Caller supplies the current
    /// authoritative avatar target immediately before each send on the host thread.
    /// True means bounded transport admission, never remote delivery or playback.
    pub fn publish_audio_output(
        &self,
        route: &AudioPlaybackRoute,
        current_target: Option<Target>,
        output: AudioOutput,
    ) -> bool {
        if Some(route.target) != current_target || output.target() != route.target {
            return false;
        }
        let mut inner = self.inner();
        if inner.quiescing {
            return false;
        }
        let Some(entry) = inner.entries.get_mut(&route.installation_id) else {
            return false;
        };
        if !playback_allowed(entry)
            || entry.generation != route.process_generation
            || entry.connection.session_id() != route.session
            || entry.connection.origin() != &route.origin
        {
            return false;
        }
        entry.connection.publish_audio_output(output).is_ok()
    }
}
pub(super) fn playback_allowed(entry: &Managed) -> bool {
    entry.enabled
        && !entry.restart
        && !entry.remove
        && entry.test_parent.is_none()
        && entry.hook_target.is_none()
        && entry.connection.is_active()
        && entry.grants.contains(&Capability::AudioPlayback)
        && !entry.revoked_controls.contains(&Capability::AudioPlayback)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        animation::AnimationState,
        behavior::BehaviorController,
        extension_actions::{ActionCatalog, ActionControllers},
    };
    use std::{fs, path::PathBuf};
    struct Fixture {
        runtime: ExtensionRuntime,
        root: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.runtime.shutdown(Duration::from_secs(2));
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn private_target_control_uses_the_same_persistent_route_eligibility() {
        for case in ["eligible", "hook", "manual", "revoked"] {
            let root =
                std::env::temp_dir().join(format!("private-audio-target-{}", Uuid::new_v4()));
            fs::create_dir_all(root.join("data")).unwrap();
            let fixture = Fixture {
                runtime: ExtensionRuntime::default(),
                root,
            };
            let id = Uuid::new_v4();
            let target = Target {
                uuid: Uuid::new_v4(),
                generation: 0,
            };
            let script = r#"import json,os,sys
from pathlib import Path
root=Path(os.environ['WAIFU_EXTENSION_DATA'])
h=json.loads(sys.stdin.readline()); (root/'ready').write_text('ready'); targets=0
for line in sys.stdin:
 m=json.loads(line)['message']
 if m['type']=='event' and m['event']['type']=='target.changed':
  targets+=1; (root/'targets').write_text(str(targets))
 elif m['type']=='audio.output':
  p=root/'barrier.tmp'; p.write_text(str(targets)); p.replace(root/'barrier')
"#;
            fixture
                .runtime
                .start(
                    id,
                    ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec!["-u".into(), "-c".into(), script.into()],
                        working_directory: fixture.root.clone(),
                        data_directory: fixture.root.join("data"),
                        lifetime: Some(Duration::from_secs(8)),
                    },
                    IntegrationOrigin {
                        integration_id: "org.example.target-fixture".into(),
                        display_name: "Owned target fixture".into(),
                    },
                    [Capability::AudioPlayback],
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(4);
            while !fixture.root.join("data/ready").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            {
                let mut inner = fixture.runtime.inner();
                let entry = inner.entries.get_mut(&id).unwrap();
                if case == "hook" {
                    entry.hook_target = Some(Some(target));
                }
                if case == "manual" {
                    entry.test_parent = Some(Uuid::new_v4());
                }
            }
            if case == "revoked" {
                fixture.runtime.set_grants(id, []);
                fixture.runtime.set_grants(id, [Capability::AudioPlayback]);
            }
            let frame = || {
                fixture.runtime.frame(
                    &[],
                    Some(target),
                    false,
                    &ActionCatalog::from_packs([], &[]),
                    ActionControllers {
                        animation: &mut AnimationState::default(),
                        behavior: &mut BehaviorController::default(),
                        external_messages: None,
                    },
                )
            };
            frame();
            if case == "revoked" {
                loop {
                    frame();
                    if fixture.runtime.inner().entries[&id]
                        .connection
                        .termination()
                        .is_some()
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert!(!fixture.root.join("data/targets").exists());
                continue;
            }
            // The transport barrier follows all frame publications on the same
            // FIFO; observe it instead of treating a sleep as proof of absence.
            fixture
                .runtime
                .inner()
                .entries
                .get_mut(&id)
                .unwrap()
                .connection
                .publish_audio_output(AudioOutput::Cancel {
                    playback_id: Uuid::new_v4(),
                    target,
                })
                .unwrap();
            while !fixture.root.join("data/barrier").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            let count = fs::read_to_string(fixture.root.join("data/barrier")).unwrap();
            assert_eq!(count, if case == "eligible" { "1" } else { "0" }, "{case}");
        }
    }
    #[test]
    fn private_audio_checks_target_session_grant_revocation_and_restart() {
        let root = std::env::temp_dir().join(format!("audio-output-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("data")).unwrap();
        let fixture = Fixture {
            runtime: ExtensionRuntime::default(),
            root,
        };
        let runtime = &fixture.runtime;
        let id = Uuid::new_v4();
        let target = Target {
            uuid: Uuid::new_v4(),
            generation: 7,
        };
        let script = r#"import json,os,sys
from pathlib import Path
root = Path(os.environ['WAIFU_EXTENSION_DATA'])
h = json.loads(sys.stdin.readline())
assert 'audio.playback' in h['message']['capabilities']
(root / 'ready').write_text(h['session_id'])
records = []
for line in sys.stdin:
 r = json.loads(line)
 if r['message']['type'] == 'audio.output':
  records.append(r)
  temporary = root / 'received.tmp'
  temporary.write_text(json.dumps(records))
  temporary.replace(root / 'received.json')
"#;
        runtime
            .start(
                id,
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-u".into(), "-c".into(), script.into()],
                    working_directory: fixture.root.clone(),
                    data_directory: fixture.root.join("data"),
                    lifetime: None,
                },
                IntegrationOrigin {
                    integration_id: "org.example.audio-output".into(),
                    display_name: "Owned output fixture".into(),
                },
                [Capability::AudioPlayback],
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !fixture.root.join("data/ready").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let route = runtime.audio_playback_route(id, target).unwrap();
        let playback_id = Uuid::new_v4();
        let start = AudioOutput::Start {
            playback_id,
            target,
        };
        assert!(!runtime.publish_audio_output(&route, None, start.clone()));
        assert!(!runtime.publish_audio_output(
            &route,
            Some(Target {
                generation: 8,
                ..target
            }),
            start.clone()
        ));
        assert!(!runtime.publish_audio_output(
            &route,
            Some(target),
            AudioOutput::Start {
                playback_id,
                target: Target {
                    uuid: Uuid::new_v4(),
                    ..target
                }
            }
        ));
        for mut changed in [route.clone(), route.clone(), route.clone(), route.clone()]
            .into_iter()
            .enumerate()
        {
            match changed.0 {
                0 => changed.1.session = Uuid::new_v4(),
                1 => changed.1.process_generation += 1,
                2 => changed.1.origin.integration_id = "org.example.other".into(),
                _ => changed.1.installation_id = Uuid::new_v4(),
            }
            assert!(!runtime.publish_audio_output(&changed.1, Some(target), start.clone()));
        }
        assert!(runtime.publish_audio_output(&route, Some(target), start));
        assert!(runtime.publish_audio_output(
            &route,
            Some(target),
            AudioOutput::Frame {
                playback_id,
                target,
                frame_sequence: 0,
                pcm: vec![123; crate::call_pcm::CALL_FRAME_SAMPLES]
            }
        ));
        assert!(runtime.publish_audio_output(
            &route,
            Some(target),
            AudioOutput::Finish {
                playback_id,
                target,
                frame_count: 1
            }
        ));
        let deadline = Instant::now() + Duration::from_secs(4);
        let records = loop {
            let records = fs::read(fixture.root.join("data/received.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok());
            if let Some(records) = records
                && records.len() == 3
            {
                break records;
            }
            assert!(
                Instant::now() < deadline,
                "private records not delivered: {:?}",
                runtime.views()
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(
            records
                .iter()
                .map(|r| r["message"]["output"]["phase"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["start", "frame", "finish"]
        );
        assert!(
            records
                .iter()
                .all(|r| r["session_id"] == route.session.to_string()
                    && r["process_generation"] == route.process_generation
                    && r["message"]["output"]["target"]["uuid"] == target.uuid.to_string())
        );
        assert_eq!(
            records[1]["message"]["output"]["pcm"]
                .as_array()
                .unwrap()
                .len(),
            1920
        );
        runtime.set_grants(id, [Capability::LifecycleObserve, Capability::ReplyRead]);
        assert!(runtime.audio_playback_route(id, target).is_none());
        assert!(!runtime.publish_audio_output(
            &route,
            Some(target),
            AudioOutput::Cancel {
                playback_id,
                target
            }
        ));
        runtime.set_grants(id, [Capability::AudioPlayback]);
        assert!(
            runtime.audio_playback_route(id, target).is_none(),
            "regrant cannot revive stopped incarnation"
        );
        runtime.restart(id);
        let catalog = ActionCatalog::from_packs([], &[]);
        let mut animation = AnimationState::default();
        let mut behavior = BehaviorController::default();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            runtime.frame(
                &[],
                Some(target),
                false,
                &catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut animation,
                    behavior: &mut behavior,
                },
            );
            if let Some(next) = runtime.audio_playback_route(id, target)
                && next.session != route.session
            {
                assert!(next.process_generation > route.process_generation);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "explicit restart not ready: {:?}",
                runtime.views()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!runtime.publish_audio_output(
            &route,
            Some(target),
            AudioOutput::Cancel {
                playback_id,
                target
            }
        ));
    }
}
