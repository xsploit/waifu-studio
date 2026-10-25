#![cfg(target_os = "linux")]
//! Defensive script controls complement the real installed process/motion test.
#[test]
fn offline_replay_pins_target_and_ignores_duplicate_delivery_results() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/extensions/twitch-replay/replay.py");
    let result = std::process::Command::new("/usr/bin/python3")
        .args(["-c", r#"
import contextlib, io, json, os, runpy, sys, tempfile
module = runpy.run_path(sys.argv[1], run_name='fixture')
session = '00000000-0000-4000-8000-000000000010'
target = {'uuid': '00000000-0000-4000-8000-000000000011', 'generation': 7}
def envelope(message):
    return {'session_id': session, 'message': message}
def run(choice, cancel):
    handshake = {'session_id': session, 'process_generation': 5,
        'message': {'type': 'handshake', 'settings': {'event': choice}}}
    messages = [handshake, envelope({'type': 'event', 'event': {'type': 'target.changed', 'target': target}})]
    if cancel:
        messages.append(envelope({'type': 'event', 'event': {'type': 'target.changed',
            'target': dict(target, generation=8)}}))
    ack = envelope({'type': 'result', 'id': 'replay-message', 'result': {'status': 'accepted'}})
    messages += [ack, ack]
    original = sys.stdin
    try:
        with tempfile.TemporaryDirectory() as data, contextlib.redirect_stdout(io.StringIO()) as output:
            os.environ['WAIFU_EXTENSION_DATA'] = data
            sys.stdin = io.StringIO(''.join(json.dumps(m) + '\n' for m in messages))
            module['run']()
            actions = [json.loads(line) for line in output.getvalue().splitlines()]
    finally:
        sys.stdin = original
    assert len(actions) == (1 if cancel else 2), actions
    assert actions[0]['action']['type'] == 'message.deliver'
    assert 'Synthetic Twitch' in actions[0]['action']['text']
    assert all(a['target'] == target for a in actions), actions
    assert [a['sequence'] for a in actions] == list(range(1, len(actions) + 1))
    if not cancel:
        assert actions[1]['action']['type'] == 'motion.play'
for choice in ['Subscription', 'Cheer', 'Chat message']:
    run(choice, False)
    run(choice, True)
"#])
        .arg(script)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
