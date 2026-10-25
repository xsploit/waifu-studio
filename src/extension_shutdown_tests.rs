//! Real local shutdown fixtures; no providers, accounts, GPU or sockets.
use super::*;
use crate::{
    animation::AnimationState,
    behavior::{BehaviorConfig, BehaviorController},
    extension_lifecycle::{LifecycleEvent, ReplyOutcome},
};

const OBSERVER: &str = r#"
import json, os, sys
from pathlib import Path
data = Path(os.environ['WAIFU_EXTENSION_DATA'])
h = json.loads(sys.stdin.readline())
def record(name, value):
    path = data / name
    temp = path.with_suffix('.tmp')
    temp.write_text(json.dumps(value))
    temp.replace(path)
record('boot.json', {'pid':os.getpid()})
sent = False
for line in sys.stdin:
    m = json.loads(line)['message']
    if m['type'] == 'result':
        record('applied.json', m['result'])
    elif m['type'] == 'event':
        e = m['event']
        if e['type'] == 'app.ready': record('ready.json', e)
        elif e['type'] == 'app.stopping':
            assert set(e) == {'type','app_run_id'}
            record('stopping.json', e)
            sys.exit(0)
        elif e['type'] == 'reply.text': record('private.json', e)
        elif e['type'] == 'target.changed' and e['target'] is not None and not sent and 'expression.apply' in h['message']['capabilities']:
            print(json.dumps({'api_version':1,'type':'action','id':'pose','sequence':1,
                'session_id':h['session_id'],'process_generation':h['process_generation'],
                'target':e['target'],'action':{'type':'expression.apply','name':'happy','strength':0.5,'duration_ms':10000}}), flush=True)
            sent = True
"#;

struct Fixture {
    runtime: ExtensionRuntime,
    root: std::path::PathBuf,
    animation: AnimationState,
    behavior: BehaviorController,
    target: Target,
    names: Vec<String>,
}
impl Fixture {
    fn new() -> Self {
        let names = vec!["happy".into()];
        let target = Target {
            uuid: Uuid::new_v4(),
            generation: 1,
        };
        let mut behavior = BehaviorController::default();
        behavior
            .select(Some(target.uuid), &BehaviorConfig::default(), &names)
            .unwrap();
        Self {
            runtime: ExtensionRuntime::default(),
            root: std::env::temp_dir().join(format!("runtime-shutdown-{}", Uuid::new_v4())),
            animation: AnimationState::default(),
            behavior,
            target,
            names,
        }
    }
    fn start(&self, id: Uuid, script: &str, grants: &[Capability]) {
        let root = self.root.join(id.to_string());
        std::fs::create_dir_all(&root).unwrap();
        self.runtime
            .start(
                id,
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-u".into(), "-c".into(), script.into()],
                    working_directory: root.clone(),
                    data_directory: root.join("data"),
                    lifetime: None,
                },
                IntegrationOrigin {
                    integration_id: "org.example.shutdown".into(),
                    display_name: "Shutdown fixture".into(),
                },
                grants.iter().copied(),
            )
            .unwrap();
    }
    fn path(&self, id: Uuid, name: &str) -> std::path::PathBuf {
        self.root.join(id.to_string()).join("data").join(name)
    }
    fn frame(&mut self, events: &[SequencedEvent]) {
        self.runtime.frame(
            events,
            Some(self.target),
            false,
            &ActionCatalog::from_packs([], &self.names),
            ActionControllers {
                external_messages: None,
                animation: &mut self.animation,
                behavior: &mut self.behavior,
            },
        );
    }
    fn wait_file(&mut self, id: Uuid, name: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            self.frame(&[]);
            if let Some(value) = std::fs::read(self.path(id, name))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "missing {name}: {:?}",
                self.runtime.views()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn begin(&mut self) {
        self.runtime
            .begin_shutdown(&mut self.animation, &mut self.behavior);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.begin();
        self.runtime.shutdown(Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn ready(app_run_id: Uuid) -> SequencedEvent {
    SequencedEvent {
        sequence: 1,
        event: LifecycleEvent::AppReady { app_run_id },
    }
}

#[test]
fn cooperative_shutdown_cancels_real_effects_rejects_clones_and_retains_exit_evidence() {
    let mut f = Fixture::new();
    let id = Uuid::new_v4();
    f.start(
        id,
        OBSERVER,
        &[
            Capability::LifecycleObserve,
            Capability::ExpressionApply,
            Capability::ReplyRead,
        ],
    );
    let app_run_id = Uuid::new_v4();
    f.frame(&[ready(app_run_id)]);
    f.wait_file(id, "ready.json");
    let applied = f.wait_file(id, "applied.json");
    assert_eq!(applied["status"], "ok");
    assert!(
        !f.behavior
            .weights_for(Some(f.target.uuid), &f.names, &BTreeMap::new(), "")
            .is_empty()
    );
    let clone = f.runtime.clone();
    f.begin();
    assert!(
        f.behavior
            .weights_for(Some(f.target.uuid), &f.names, &BTreeMap::new(), "")
            .is_empty()
    );
    assert!(clone.inner().quiescing);
    assert_eq!(clone.inner().current_target, None);
    assert!(clone.inner().pending_hooks.is_empty());
    clone.restart(id);
    clone.set_grants(
        id,
        [Capability::ExpressionApply, Capability::LifecycleObserve],
    );
    assert_eq!(
        clone.inner().entries[&id].connection.grants(),
        &BTreeSet::from([Capability::LifecycleObserve])
    );
    assert!(
        clone
            .start(
                Uuid::new_v4(),
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec![],
                    working_directory: f.root.clone(),
                    data_directory: f.root.join("forbidden"),
                    lifetime: None,
                },
                IntegrationOrigin {
                    integration_id: "org.example.forbidden".into(),
                    display_name: "Forbidden".into()
                },
                []
            )
            .is_err()
    );
    // No post-quiesce frame may route private data or dispatch new actions.
    f.frame(&[SequencedEvent {
        sequence: 2,
        event: LifecycleEvent::ReplyFinished {
            reply_id: Uuid::new_v4(),
            character: Some(f.target.uuid),
            outcome: ReplyOutcome::Completed {
                text: "synthetic private text".into(),
            },
        },
    }]);
    assert!(f.runtime.shutdown(Duration::from_secs(1)));
    let stopping: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path(id, "stopping.json")).unwrap()).unwrap();
    assert_eq!(
        stopping,
        serde_json::json!({"type":"app.stopping","app_run_id":app_run_id})
    );
    assert!(!f.path(id, "private.json").exists());
    let inner = f.runtime.inner();
    let terminal = inner.entries[&id].connection.termination().unwrap();
    assert_eq!(terminal.code, Some(0));
    assert_eq!(terminal.reason, "Completed");
}

#[test]
fn shutdown_does_not_invent_ready_or_notify_revoked_observers() {
    for initialized in [false, true] {
        let mut f = Fixture::new();
        let id = Uuid::new_v4();
        f.start(id, OBSERVER, &[Capability::LifecycleObserve]);
        f.wait_file(id, "boot.json");
        if initialized {
            f.frame(&[ready(Uuid::new_v4())]);
            f.wait_file(id, "ready.json");
            f.runtime.set_grants(id, []);
        }
        f.begin();
        assert!(f.runtime.shutdown(Duration::from_secs(1)));
        assert!(!f.path(id, "stopping.json").exists());
        assert!(
            f.runtime.inner().entries[&id]
                .connection
                .termination()
                .is_some()
        );
        assert_eq!(f.runtime.inner().app_run_id.is_some(), initialized);
    }
}

#[test]
fn four_blocked_children_share_one_budget_and_zero_deadlines_force_every_survivor() {
    for budget in [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_millis(200),
    ] {
        let mut f = Fixture::new();
        let script = "import json, os, pathlib, sys, time\nh=json.loads(sys.stdin.readline())\npathlib.Path(os.environ['WAIFU_EXTENSION_DATA'],'boot.json').write_text(json.dumps({'pid':os.getpid()}))\ntime.sleep(30)";
        let mut ids = Vec::new();
        for _ in 0..MAX_EXTENSIONS {
            let id = Uuid::new_v4();
            f.start(id, script, &[Capability::LifecycleObserve]);
            ids.push(id);
        }
        for id in &ids {
            f.wait_file(*id, "boot.json");
        }
        f.frame(&[ready(Uuid::new_v4())]);
        f.begin();
        let start = Instant::now();
        f.runtime.shutdown(budget);
        assert!(
            start.elapsed() < budget + Duration::from_millis(400),
            "per-child shutdown extended the budget"
        );
        assert!(f.runtime.views().iter().all(|view| !view.active));
        assert!(
            f.runtime.shutdown(Duration::from_secs(1)),
            "cutoff failed to signal every child"
        );
        for id in &ids {
            assert!(
                f.runtime.inner().entries[id]
                    .connection
                    .termination()
                    .is_some()
            );
        }
        assert_eq!(
            f.runtime.process_count(),
            MAX_EXTENSIONS,
            "terminal evidence must remain retained"
        );
    }
}

#[test]
fn shutdown_remains_bounded_with_output_flood_or_descendants_holding_pipes() {
    let scripts = [
        "import json,os,pathlib,sys\nh=json.loads(sys.stdin.readline())\npathlib.Path(os.environ['WAIFU_EXTENSION_DATA'],'boot.json').write_text('{}')\nwhile True: print('{}',flush=True)",
        "import json,os,pathlib,sys,time\nh=json.loads(sys.stdin.readline())\npid=os.fork()\nif pid==0: time.sleep(30); os._exit(0)\npathlib.Path(os.environ['WAIFU_EXTENSION_DATA'],'boot.json').write_text('{}')\nfor line in sys.stdin:\n if json.loads(line)['message'].get('event',{}).get('type')=='app.stopping': break",
    ];
    for script in scripts {
        let mut f = Fixture::new();
        let id = Uuid::new_v4();
        f.start(id, script, &[Capability::LifecycleObserve]);
        f.wait_file(id, "boot.json");
        f.frame(&[ready(Uuid::new_v4())]);
        f.begin();
        assert!(f.runtime.shutdown(Duration::from_secs(1)));
        assert!(
            f.runtime.inner().entries[&id]
                .connection
                .termination()
                .is_some()
        );
    }
}
