#![cfg(target_os = "linux")]
//! Actual bundled handler over bounded local pipes; no providers, network or GPU.
use std::{collections::BTreeSet, path::Path};
use waifu_studio::{
    extension_manifest::{ContentKind, load_integration_pack, load_pack},
    extension_protocol::parse_action_line,
};

const DRIVER: &str = r#"
import json, os, pathlib, select, signal, subprocess, sys, tempfile, time

signal.alarm(30)  # Bound the entire driver as well as each individual child wait.

script = pathlib.Path(sys.argv[1]) / 'assets/extensions/reaction-hook/hook.py'
session = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
target = {'uuid':'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'generation':7}
other = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'
motion = 'studio.example.reaction-hook/nod'
grants = ['lifecycle.observe', 'motion.play', 'expression.apply']
events = {'Reply finished':'reply.finished', 'Speech started':'speech.started',
          'Speech stopped':'speech.stopped', 'Emotion changed':'emotion.changed'}

def source(kind='reply.finished', character=target['uuid'], outcome='completed'):
    value = {'type':kind, 'character':character}
    if kind == 'reply.finished':
        value.update(reply_id=other, outcome=outcome)
    elif kind in ('speech.started', 'speech.stopped'):
        value.update(playback_id=other, voice_generation=2, reply_id=None)
        if kind == 'speech.stopped': value['reason'] = 'observed_terminal'
    else:
        value.update(owner='studio', previous='neutral', emotion='happy', reply_id=None)
    return value

class Child:
    def __init__(self, settings=None, testing=False, allowed=grants, **identity):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self.tmp.name) / ('latest-test.json' if testing else 'latest-hook.json')
        self.sequence = 0
        self.action_sequence = 0
        self.p = subprocess.Popen(['/usr/bin/python3', str(script)] + (['--test'] if testing else []),
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={'WAIFU_EXTENSION_DATA': self.tmp.name})
        self.send({'type':'handshake', 'origin':{'integration_id':'studio.example.reaction-hook',
            'display_name':'Configurable reaction hook'}, 'settings':settings or {},
            'capabilities':allowed}, **identity)
    def __enter__(self): return self
    def __exit__(self, *_):
        if self.p.poll() is None: self.p.kill()
        self.p.communicate(timeout=2)
        self.tmp.cleanup()
    def send(self, message, **overrides):
        self.sequence += 1
        packet = dict(api_version=1, type='host', session_id=session, process_generation=5,
                      sequence=self.sequence, message=message)
        packet.update(overrides)
        self.p.stdin.write((json.dumps(packet)+'\n').encode())
        self.p.stdin.flush()
    def event(self, event, **overrides):
        self.send({'type':'event', 'event':event}, **overrides)
    def select(self, value=target): self.event({'type':'target.changed','target':value})
    def action(self):
        line = bytearray()
        deadline = time.monotonic()+2
        while not line.endswith(b'\n'):
            remaining = deadline-time.monotonic()
            assert remaining > 0 and select.select([self.p.stdout], [], [], remaining)[0], 'action timeout'
            byte = os.read(self.p.stdout.fileno(), 1)
            assert byte and len(line) < 65536, 'missing or oversized action'
            line.extend(byte)
        value = json.loads(line)
        assert set(value) == {'api_version','type','id','sequence','session_id',
                              'process_generation','target','action'}, value
        self.action_sequence += 1
        assert value['api_version'] == 1 and value['type'] == 'action'
        assert value['sequence'] == self.action_sequence
        assert value['id'] == 'reaction-' + str(self.action_sequence)
        assert value['target'] == target and value['session_id'] == session
        assert value['process_generation'] == 5
        # Rust also validates every emitted real action with parse_action_line.
        print(json.dumps(value))
        return value
    def ack(self, action, **overrides):
        value = {'type':'result', 'id':action['id'], 'target':target,
                 'result':{'status':'ok','value':{'type':'accepted'}}}
        value.update(overrides)
        self.send(value)
    def terminal(self, action, **overrides):
        value = {'type':'motion.finished','target':target,'action_id':action['id'],
                 'motion_id':action['action']['motion_id'],'outcome':'completed'}
        value.update(overrides)
        self.event(value)
    def quiet(self):
        assert self.p.poll() is None, 'finished before correlated acceptance/terminal'
        assert not select.select([self.p.stdout], [], [], .03)[0], 'unexpected output or early exit'
    def finish(self, state='completed'):
        code = self.p.wait(timeout=3)
        assert (code == 0) == (state != 'error'), (code, state)
        assert json.loads(self.path.read_text()) == {'state':state}
        assert self.path.stat().st_size <= 1024
        assert not self.p.stdout.read(65537), 'unexpected extra actions'
        error = self.p.stderr.read(65537)
        assert error == (b'Reaction hook stopped\n' if state == 'error' else b''), error

def normal_filters():
    for selected, kind in events.items():
        for character in ['', target['uuid']]:
            with Child({'event':selected,'character':character}) as child:
                # Both source-before-target and target-before-source are supported.
                if character: child.event(source(kind))
                child.select()
                if not character: child.event(source(kind))
                action = child.action()
                assert action['action'] == {'type':'motion.play','motion_id':motion,'mode':'one_shot'}
                child.ack(action)
                child.terminal(action)
                child.finish()
    for settings, event in [
        ({'event':'Speech started'}, source()),
        ({}, source(character=other)),
        ({}, source(character=None)),
        ({}, source(outcome='failed')),
        ({}, source(outcome='cancelled')),
        ({'character':other}, source()),
        ({'active':False}, source()),
    ]:
        with Child(settings) as child:
            # Inactive settings terminate before any host source is needed.
            if settings.get('active') is not False:
                child.select()
                if settings.get('character') != other: child.event(event)
            child.finish('ignored')

def none_and_test():
    for testing in [False, True]:
        with Child({'motion':'','expression':''}, testing=testing, allowed=[]) as child:
            child.finish()
    with Child({'event':'Emotion changed','character':target['uuid']}, testing=True) as child:
        child.select()  # No lifecycle event: --test bypasses only trigger matching.
        action = child.action()
        child.ack(action)
        child.terminal(action)
        child.finish()
    with Child({'character':other}, testing=True) as child:
        child.select()
        child.finish('ignored')
    with Child({'active':False}, testing=True) as child:
        child.finish('ignored')

def invalid_settings():
    cases = [{'active':1}, {'active':'true'}, {'event':False}, {'event':[]},
             {'event':'Unknown'}, {'character':False}, {'character':None},
             {'character':target['uuid'].upper()}, {'character':'0'*36},
             {'character':'00000000-0000-0000-0000-000000000000'},
             {'character':target['uuid'].replace('-','')},
             {'motion':None}, {'motion':False}, {'motion':'x'*194}, {'motion':'bad motion'},
             {'expression':False}, {'expression':{}}, {'expression':'blinkLeft'},
             {'expression':' '}, {'expression':'x'*129}, {'unknown':'PRIVATE-SETTING'}]
    for settings in cases:
        with Child(settings) as child: child.finish('error')
    for identity in [dict(session_id=session.upper()), dict(process_generation=True),
                     dict(sequence=0)]:
        with Child(**identity) as child: child.finish('error')
    for value in [None, dict(target, uuid=target['uuid'].upper()), dict(target, generation=True)]:
        with Child(testing=True) as child:
            child.select(value)
            child.finish('error')

def correlation():
    with Child() as child:
        child.select(); child.event(source())
        action = child.action()
        child.ack(action)
        child.ack(action)  # A new host sequence carrying the same ACK is harmless.
        child.quiet()
        for mismatch in [dict(action_id='unrelated'), dict(motion_id='org.example.other/nod'),
                         dict(target=dict(target, generation=8))]:
            child.terminal(action, **mismatch)
            child.quiet()
        child.terminal(action)
        child.finish()
    with Child() as child:
        child.select(); child.event(source())
        action = child.action()
        child.terminal(action)  # Completion without admission must remain pending.
        child.quiet()
        child.ack(action, id='unrelated')
        child.ack(action, target=dict(target, generation=8))
        child.quiet()
        child.ack(action)
        child.finish()
    for failure in ['session','process','sequence','target','rejected','cancelled','failed']:
        with Child() as child:
            child.select(); child.event(source())
            action = child.action()
            if failure in ['session','process','sequence']:
                override = {'session':dict(session_id=other),
                            'process':dict(process_generation=6),
                            'sequence':dict(sequence=child.sequence)}[failure]
                child.event({'type':'target.changed','target':target}, **override)
            elif failure == 'target': child.select(dict(target, generation=8))
            elif failure == 'rejected':
                child.ack(action, result={'status':'error','code':'busy','message':'Busy'})
            else: child.terminal(action, outcome=failure)
            child.finish('error')

{'filters':normal_filters,'none_test':none_and_test,'invalid':invalid_settings,
 'correlation':correlation}[sys.argv[2]]()
"#;

fn run(case: &str) {
    let result = std::process::Command::new("/usr/bin/python3")
        .args(["-c", DRIVER, env!("CARGO_MANIFEST_DIR"), case])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{case}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    for line in result
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        parse_action_line(line).unwrap();
    }
}

#[test]
fn reaction_hook_filters_all_normal_triggers_and_source_characters() {
    run("filters");
}

#[test]
fn reaction_hook_none_and_test_mode_preserve_character_and_active_filters() {
    run("none_test");
}

#[test]
fn reaction_hook_rejects_malformed_settings_and_noncanonical_identities() {
    run("invalid");
}

#[test]
fn reaction_hook_requires_correlated_admission_and_terminal_in_current_session_and_target() {
    run("correlation");
}

#[test]
fn inspect_integration_validates_actual_reaction_hook_executable_and_motion_assets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/reaction-hook");
    assert!(
        load_pack(&root).is_err(),
        "ordinary content installation must reject code"
    );
    let pack = load_integration_pack(&root).unwrap();
    assert_eq!(pack.manifest.id, "studio.example.reaction-hook");
    assert!(pack.manifest.process.is_none());
    let triggers: BTreeSet<_> = pack
        .manifest
        .hooks
        .iter()
        .map(|hook| hook.event.as_str())
        .collect();
    assert_eq!(
        triggers,
        BTreeSet::from([
            "reply.finished",
            "speech.started",
            "speech.stopped",
            "emotion.changed"
        ])
    );
    assert_eq!(pack.manifest.hooks.len(), 4);
    for hook in &pack.manifest.hooks {
        assert_eq!(hook.handler.program, "hook.py");
        assert!(pack.root.join(&hook.handler.program).is_file());
    }
    let test = pack.manifest.test_process.as_ref().unwrap();
    assert_eq!(test.program, "hook.py");
    assert_eq!(test.args, ["--test"]);
    let motion = pack
        .content
        .iter()
        .find(|item| item.kind == ContentKind::Motion)
        .unwrap();
    assert_eq!(motion.id, "studio.example.reaction-hook/nod");
    let path = motion.path.as_ref().unwrap();
    assert!(path.is_absolute() && path.starts_with(&pack.root) && path.is_file());
    let source = std::fs::read_to_string(path).unwrap();
    let clip = waifu_studio::animation::parse_bvh(&source).unwrap();
    assert!(clip.frame_count > 0);
}
