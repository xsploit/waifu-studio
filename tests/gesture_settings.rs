#![cfg(target_os = "linux")]
//! Actual example processes with local JSONL host controls; no network or GPU.

const DRIVER: &str = r#"
import json, os, pathlib, subprocess, sys, tempfile, time

root = pathlib.Path(sys.argv[1]) / 'assets/extensions'
session = '00000000-0000-4000-8000-000000000010'
target = {'uuid': '00000000-0000-4000-8000-000000000011', 'generation': 7}

def envelope(message):
    return {'session_id': session, 'process_generation': 5, 'message': message}

def event(value):
    return envelope({'type': 'event', 'event': value})

reply = event({'type': 'reply.finished', 'character': target['uuid'], 'outcome': 'completed'})
selected = event({'type': 'target.changed', 'target': target})

def handshake(settings):
    return envelope({'type': 'handshake', 'settings': settings,
        'capabilities': ['lifecycle.observe', 'motion.play']})

def run(pack, script, receipt, settings, messages):
    with tempfile.TemporaryDirectory() as data:
        result = subprocess.run([sys.executable, str(root / pack / script)],
            input=''.join(json.dumps(m) + '\n' for m in [handshake(settings)] + messages),
            text=True, capture_output=True, timeout=3,
            env=dict(os.environ, WAIFU_EXTENSION_DATA=data))
        record = json.loads((pathlib.Path(data) / receipt).read_text())
        actions = [json.loads(line) for line in result.stdout.splitlines()]
        return result, record, actions

examples = [('reply-gesture', 'gesture.py', 'latest.json', 'gesture-1'),
            ('hook-gesture', 'hook.py', 'latest-hook.json', 'hook-nod')]

def selections():
    for pack, script, receipt, action_id in examples:
        own = 'studio.example.' + pack + '/nod'
        manifest = json.loads((root / pack / 'extension.json').read_text())
        definition = next(s for s in manifest['settings'] if s['id'] == 'motion')
        assert definition == {'id': 'motion', 'label': 'Gesture', 'type': 'motion', 'default': own}
        custom = 'org.example.custom/' + 'x' * (193 - len('org.example.custom/'))
        for choice in [None, own, custom, '雪' * 64 + 'x']:
            settings = {} if choice is None else {'motion': choice}
            expected = own if choice is None else choice
            accepted = envelope({'type': 'result', 'id': action_id,
                'result': {'status': 'ok', 'value': {'type': 'accepted'}}})
            terminal = {'type': 'motion.finished', 'action_id': action_id,
                'motion_id': expected, 'target': target, 'outcome': 'completed'}
            result, record, actions = run(pack, script, receipt, settings,
                [selected, reply, reply, accepted, accepted, reply, event(terminal), reply])
            assert result.returncode == 0, result.stderr
            assert len(actions) == 1, actions
            action = actions[0]
            assert action['action'] == {'type': 'motion.play', 'motion_id': expected, 'mode': 'one_shot'}
            assert action['target'] == target and action['session_id'] == session
            assert action['process_generation'] == 5 and action['sequence'] == 1
            assert action['id'] == action_id
            # Accepted results leave ownership pending until the terminal event.
            assert record == terminal, record

def disabled():
    pack, script, receipt, _ = examples[0]
    with tempfile.TemporaryDirectory() as data:
        child = subprocess.Popen([sys.executable, str(root / pack / script)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            env=dict(os.environ, WAIFU_EXTENSION_DATA=data))
        try:
            child.stdin.write(json.dumps(handshake({'motion': ''})) + '\n')
            child.stdin.flush()
            path = pathlib.Path(data) / receipt
            deadline = time.monotonic() + 2
            while True:
                try:
                    record = json.loads(path.read_text())
                    break
                except (FileNotFoundError, json.JSONDecodeError):
                    assert time.monotonic() < deadline, 'missing disabled receipt'
                    time.sleep(0.002)
            assert record['state'] == 'disabled', record
            child.stdin.write(json.dumps(selected) + '\n' + json.dumps(reply) + '\n')
            child.stdin.flush()
            time.sleep(0.05)
            assert child.poll() is None, 'persistent example exited instead of staying disabled'
            output, error = child.communicate(timeout=2)
            assert child.returncode == 0 and not output, (output, error)
            assert json.loads(path.read_text())['state'] == 'disabled'
        finally:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=2)
    pack, script, receipt, _ = examples[1]
    result, record, actions = run(pack, script, receipt, {'motion': ''}, [selected, reply])
    assert result.returncode == 0 and not actions, (result.stderr, actions)
    assert record['state'] == 'disabled', record

def invalid():
    for pack, script, receipt, _ in examples:
        for value in ['x' * 194, '雪' * 64 + 'xx', ' ', 'a b', 'a\u00a0b',
                      'a\u2003b', 'a\nb', 'a\x00b', 'a\x7fb', 'a\x85b', '\ud800', None, False]:
            result, record, actions = run(pack, script, receipt, {'motion': value}, [selected, reply])
            assert result.returncode != 0 and not actions, (value, actions)
            assert result.stderr.strip() == 'Invalid motion setting', result.stderr
            assert record == {'state': 'error', 'reason': 'Invalid motion setting'}, record

{'selections': selections, 'disabled': disabled, 'invalid': invalid}[sys.argv[2]]()
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
}

#[test]
fn gesture_examples_use_default_custom_and_boundary_ids_until_terminal() {
    run("selections");
}

#[test]
fn empty_motion_disables_actions_without_exiting_persistent_example() {
    run("disabled");
}

#[test]
fn gesture_examples_reject_invalid_motion_settings_without_echoing_values() {
    run("invalid");
}
