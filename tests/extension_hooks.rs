#![cfg(target_os = "linux")]
//! Real subprocess coverage of the public hook coordinator, without app/GPU setup.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{
    animation::AnimationState,
    behavior::BehaviorController,
    extension_actions::{ActionCatalog, ActionControllers},
    extension_hooks::{HookProcess, HookTrigger},
    extension_lifecycle::{LifecycleEvent, ReplyOutcome, SequencedEvent},
    extension_process::ProcessSpec,
    extension_protocol::{Capability, IntegrationOrigin, SpeechStopReason, Target},
    extension_runtime::ExtensionRuntime,
};

const SERVICE: &str = r#"
import json, os, sys
from pathlib import Path
h = json.loads(sys.stdin.readline())
Path(os.environ['WAIFU_EXTENSION_DATA'], 'ready').write_text(h['session_id'])
for line in sys.stdin:
    json.loads(line)
"#;

const HOOK: &str = r#"
import json, os, sys, time
from pathlib import Path
d = Path(os.environ['WAIFU_EXTENSION_DATA'])
h = json.loads(sys.stdin.readline())
assert h['message']['type'] == 'handshake'
# A second child for this installation must never overlap the first child.
active = d / 'active'
with active.open('x') as out:
    out.write(h['session_id'])
events = []
for line in sys.stdin:
    m = json.loads(line)
    assert m['session_id'] == h['session_id']
    if m['message']['type'] != 'event':
        continue
    e = m['message']['event']
    if e['type'] == 'target.changed':
        continue
    events.append(e)
    if e['type'] in ('reply.finished', 'speech.started', 'speech.stopped', 'emotion.changed'):
        with (d / 'records.jsonl').open('a') as out:
            out.write(json.dumps({'session': h['session_id'], 'generation': h['process_generation'],
                'capabilities': h['message']['capabilities'], 'events': events}) + '\n')
            out.flush()
        break
else:
    raise AssertionError('hook never received its captured trigger')
while not (d / 'auto-release').exists() and not (d / ('release-' + str(h['process_generation']))).exists():
    time.sleep(0.002)
active.unlink()
"#;

struct Fixture {
    runtime: ExtensionRuntime,
    root: PathBuf,
    animation: AnimationState,
    behavior: BehaviorController,
    catalog: ActionCatalog,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("waifu-public-hooks-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self {
            runtime: ExtensionRuntime::default(),
            root,
            animation: AnimationState::default(),
            behavior: BehaviorController::default(),
            catalog: ActionCatalog::from_packs([], &[]),
        }
    }
    fn spec(&self, name: &str, script: &str) -> ProcessSpec {
        ProcessSpec {
            program: "/usr/bin/python3".into(),
            args: vec!["-u".into(), "-c".into(), script.into()],
            working_directory: self.root.clone(),
            data_directory: self.root.join(name),
            lifetime: Some(Duration::from_secs(15)),
        }
    }
    fn register(&self, id: Uuid, triggers: &[HookTrigger], private: bool, auto_release: bool) {
        let data = self.root.join("hook");
        fs::create_dir_all(&data).unwrap();
        if auto_release {
            fs::write(data.join("auto-release"), b"go").unwrap();
        }
        let grants = if private {
            vec![Capability::LifecycleObserve, Capability::ReplyRead]
        } else {
            vec![Capability::LifecycleObserve]
        };
        self.runtime
            .configure_hooks(
                id,
                origin("hook"),
                grants,
                Default::default(),
                triggers
                    .iter()
                    .map(|trigger| HookProcess {
                        trigger: *trigger,
                        process: self.spec("hook", HOOK),
                    })
                    .collect(),
            )
            .unwrap();
    }
    fn frame(&mut self, events: &[SequencedEvent], target: Option<Target>) {
        self.runtime.frame(
            events,
            target,
            false,
            &self.catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut self.animation,
                behavior: &mut self.behavior,
            },
        );
        assert!(
            self.runtime.process_count() <= 4,
            "shared process limit exceeded: {:?}",
            self.runtime.views()
        );
    }
    fn until(&mut self, target: Option<Target>, mut ready: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            self.frame(&[], target);
            if ready(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "hook test timed out: {:?}, records: {:?}",
                self.runtime.views(),
                self.records()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn occupy_slots(&mut self) -> Vec<Uuid> {
        let mut ids = Vec::new();
        for index in 0..4 {
            let id = Uuid::new_v4();
            self.runtime
                .start(
                    id,
                    self.spec(&format!("service-{index}"), SERVICE),
                    origin(&format!("service-{index}")),
                    [],
                )
                .unwrap();
            ids.push(id);
        }
        self.until(None, |f| {
            (0..4).all(|i| f.root.join(format!("service-{i}/ready")).exists())
        });
        assert_eq!(self.runtime.process_count(), 4);
        ids
    }
    fn records(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("hook/records.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
    fn release(&self, generation: u64) {
        fs::write(self.root.join(format!("hook/release-{generation}")), b"go").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Bounded cleanup also runs when a test assertion panics.
        let _ = self.runtime.shutdown(Duration::from_secs(2));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn origin(name: &str) -> IntegrationOrigin {
    IntegrationOrigin {
        integration_id: format!("org.example.{name}"),
        display_name: name.into(),
    }
}
fn reply(sequence: u64, id: Uuid, text: &str) -> SequencedEvent {
    SequencedEvent {
        sequence,
        event: LifecycleEvent::ReplyFinished {
            reply_id: id,
            character: Some(Uuid::from_u128(91)),
            outcome: ReplyOutcome::Completed { text: text.into() },
        },
    }
}

#[test]
fn hooks_share_service_slots_serialize_children_and_cannot_regain_queued_private_text() {
    let mut f = Fixture::new();
    let hook = Uuid::new_v4();
    f.register(hook, &[HookTrigger::ReplyFinished], true, false);
    assert_eq!(
        f.runtime.process_count(),
        0,
        "idle hook registration consumed a slot"
    );
    let services = f.occupy_slots();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let events = [
        reply(1, first, "private original one"),
        reply(2, second, "private original two"),
    ];
    f.frame(&events, None);
    assert_eq!(f.runtime.process_count(), 4);
    assert!(f.records().is_empty());
    assert!(
        f.runtime
            .views()
            .iter()
            .any(|v| v.id == hook && !v.active && v.status.contains("queued"))
    );
    // Revoke and restore before another frame: captured data must stay redacted.
    f.runtime.set_grants(hook, [Capability::LifecycleObserve]);
    f.runtime
        .set_grants(hook, [Capability::LifecycleObserve, Capability::ReplyRead]);
    f.runtime.remove(services[0]);
    assert_eq!(
        f.runtime.process_count(),
        4,
        "slot freed before runtime reaping"
    );
    f.until(None, |f| f.records().len() == 1);
    let first_record = f.records().remove(0);
    assert_eq!(f.runtime.process_count(), 4);
    // Pump several frames while child one is held; child two must stay queued.
    for _ in 0..12 {
        f.frame(&[], None);
    }
    assert_eq!(f.records().len(), 1);
    let generation = first_record["generation"].as_u64().unwrap();
    f.release(generation);
    f.until(None, |f| f.records().len() == 2);
    let records = f.records();
    assert_ne!(records[0]["session"], records[1]["session"]);
    let next_generation = records[1]["generation"].as_u64().unwrap();
    assert!(next_generation > generation);
    for (record, id) in records.iter().zip([first, second]) {
        assert_eq!(
            record["capabilities"],
            serde_json::json!(["lifecycle.observe"])
        );
        let captured = record["events"].as_array().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "revoked text or extra trigger was delivered"
        );
        assert_eq!(captured[0]["type"], "reply.finished");
        assert_eq!(captured[0]["reply_id"], id.to_string());
        assert_eq!(captured[0]["character"], Uuid::from_u128(91).to_string());
    }
    assert!(
        !fs::read_to_string(f.root.join("hook/records.jsonl"))
            .unwrap()
            .contains("private original")
    );
    f.release(next_generation);
    f.until(None, |f| f.runtime.process_count() == 3);
    // Replayed source sequences must not create another invocation.
    f.frame(&events, None);
    assert_eq!(f.runtime.process_count(), 3);
    assert_eq!(f.records().len(), 2);
    assert!(f.runtime.shutdown(Duration::from_secs(2)));
}

#[test]
fn disabling_removing_resetting_and_target_change_discard_queued_triggers() {
    for operation in ["disable", "remove", "reset", "target-change"] {
        let mut f = Fixture::new();
        let services = f.occupy_slots();
        let hook = Uuid::new_v4();
        f.register(hook, &[HookTrigger::ReplyFinished], false, true);
        let discarded = Uuid::new_v4();
        f.frame(&[reply(1, discarded, "old")], None);
        assert!(f.records().is_empty());
        let target = if operation == "target-change" {
            Some(Target {
                uuid: Uuid::new_v4(),
                generation: 3,
            })
        } else {
            None
        };
        match operation {
            "disable" => {
                f.runtime.stop(hook);
                f.runtime.restart(hook);
            }
            "remove" => {
                f.runtime.remove(hook);
                assert!(!f.runtime.views().iter().any(|v| v.id == hook));
                f.register(hook, &[HookTrigger::ReplyFinished], false, true);
            }
            "reset" => f.runtime.restart(hook),
            "target-change" => f.frame(&[], target),
            _ => unreachable!(),
        }
        f.runtime.remove(services[0]);
        f.until(target, |f| f.runtime.process_count() == 3);
        assert!(
            f.records().is_empty(),
            "{operation} replayed a discarded trigger"
        );
        let fresh = Uuid::new_v4();
        f.frame(&[reply(2, fresh, "new")], target);
        f.until(target, |f| !f.records().is_empty());
        f.until(target, |f| f.runtime.process_count() == 3);
        let records = f.records();
        assert_eq!(records.len(), 1, "{operation} created extra invocations");
        assert_eq!(
            records[0]["events"][0]["reply_id"],
            fresh.to_string(),
            "{operation}"
        );
        assert!(f.runtime.shutdown(Duration::from_secs(2)));
    }
}

#[test]
fn speech_hooks_deliver_source_metadata_once_in_separate_sessions() {
    let mut f = Fixture::new();
    let hook = Uuid::new_v4();
    f.register(
        hook,
        &[HookTrigger::SpeechStarted, HookTrigger::SpeechStopped],
        false,
        true,
    );
    let playback_id = Uuid::new_v4();
    let reply_id = Uuid::new_v4();
    let events = [
        SequencedEvent {
            sequence: 1,
            event: LifecycleEvent::SpeechStarted {
                playback_id,
                character: None,
                voice_generation: 37,
                reply_id: Some(reply_id),
            },
        },
        SequencedEvent {
            sequence: 2,
            event: LifecycleEvent::SpeechStopped {
                playback_id,
                character: None,
                voice_generation: 37,
                reply_id: Some(reply_id),
                reason: SpeechStopReason::ObservedTerminal,
            },
        },
    ];
    f.frame(&events, None);
    f.until(None, |f| {
        f.records().len() == 2 && f.runtime.process_count() == 0
    });
    let records = f.records();
    assert_ne!(records[0]["session"], records[1]["session"]);
    assert!(
        records[1]["generation"].as_u64().unwrap() > records[0]["generation"].as_u64().unwrap()
    );
    for (record, kind) in records.iter().zip(["speech.started", "speech.stopped"]) {
        assert_eq!(record["events"].as_array().unwrap().len(), 1);
        let event = &record["events"][0];
        assert_eq!(event["type"], kind);
        assert_eq!(event["playback_id"], playback_id.to_string());
        assert_eq!(event["reply_id"], reply_id.to_string());
        assert_eq!(event["voice_generation"], 37);
        assert!(event["character"].is_null());
        assert!(event.get("target").is_none());
    }
    assert_eq!(records[1]["events"][0]["reason"], "observed_terminal");
    assert!(f.runtime.shutdown(Duration::from_secs(2)));
}

#[test]
fn emotion_hooks_preserve_both_source_owners_without_avatar_or_private_read() {
    use waifu_studio::{extension_protocol::EmotionOwner, reply_affect::AffectEmotion};
    let mut f = Fixture::new();
    let hook = Uuid::new_v4();
    f.register(
        hook,
        &[
            HookTrigger::ReplyFinished,
            HookTrigger::SpeechStarted,
            HookTrigger::SpeechStopped,
            HookTrigger::EmotionChanged,
        ],
        false,
        true,
    );
    assert_eq!(
        f.runtime.process_count(),
        0,
        "registration launched a child"
    );
    let character = Uuid::new_v4();
    let reply_id = Uuid::new_v4();
    let events: Vec<_> = [EmotionOwner::Studio, EmotionOwner::SceneChat]
        .into_iter()
        .enumerate()
        .map(|(index, owner)| SequencedEvent {
            sequence: index as u64 + 1,
            event: LifecycleEvent::EmotionChanged {
                character,
                owner,
                previous: AffectEmotion::Neutral,
                emotion: AffectEmotion::Happy,
                reply_id: Some(reply_id),
            },
        })
        .collect();
    f.frame(&events, None);
    f.until(None, |f| {
        f.records().len() == 2 && f.runtime.process_count() == 0
    });
    let records = f.records();
    assert_ne!(records[0]["session"], records[1]["session"]);
    for (record, owner) in records.iter().zip(["studio", "scene_chat"]) {
        assert_eq!(
            record["capabilities"],
            serde_json::json!(["lifecycle.observe"])
        );
        let delivered = record["events"].as_array().unwrap();
        assert_eq!(delivered.len(), 1);
        let event = &delivered[0];
        assert_eq!(event["type"], "emotion.changed");
        assert_eq!(event["character"], character.to_string());
        assert_eq!(event["owner"], owner);
        assert_eq!(event["previous"], "neutral");
        assert_eq!(event["emotion"], "happy");
        assert_eq!(event["reply_id"], reply_id.to_string());
        assert!(event.get("target").is_none());
        assert!(event.get("text").is_none());
    }
    f.frame(&events, None);
    assert_eq!(f.runtime.process_count(), 0);
    assert_eq!(f.records().len(), 2);
}

#[test]
fn emotion_hook_observation_revocation_discards_captured_work() {
    use waifu_studio::{extension_protocol::EmotionOwner, reply_affect::AffectEmotion};
    let mut f = Fixture::new();
    let services = f.occupy_slots();
    let hook = Uuid::new_v4();
    f.register(hook, &[HookTrigger::EmotionChanged], false, true);
    let event = |sequence| SequencedEvent {
        sequence,
        event: LifecycleEvent::EmotionChanged {
            character: Uuid::from_u128(12),
            owner: EmotionOwner::Studio,
            previous: AffectEmotion::Happy,
            emotion: AffectEmotion::Neutral,
            reply_id: None,
        },
    };
    f.frame(&[event(1)], None);
    assert!(f.records().is_empty());
    f.runtime.set_grants(hook, []);
    f.runtime.set_grants(hook, [Capability::LifecycleObserve]);
    f.runtime.remove(services[0]);
    f.until(None, |f| f.runtime.process_count() == 3);
    assert!(f.records().is_empty(), "revoked queued emotion returned");
    f.frame(&[event(2)], None);
    f.until(None, |f| {
        f.records().len() == 1 && f.runtime.process_count() == 3
    });
    assert!(f.records()[0]["events"][0]["reply_id"].is_null());
}
