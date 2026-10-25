#![cfg(target_os = "linux")]
//! Real offline children validate the stop-request/confirmed-exit distinction.
#[test]
fn integration_exit_observer_requires_typed_metadata_and_never_requests_actions() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/extensions/integration-stop-observer/observer.py");
    let result = std::process::Command::new("/usr/bin/python3")
        .arg("-c")
        .arg(r#"
import json, pathlib, subprocess, sys, tempfile
session = '00000000-0000-4000-8000-000000000001'
source = dict(installation_id='00000000-0000-4000-8000-000000000002',
              session_id='00000000-0000-4000-8000-000000000003', process_generation=6)
def packet(n, message):
    return dict(api_version=1, type='host', session_id=session, process_generation=1,
                sequence=n, message=message)
handshake = packet(1, dict(type='handshake', capabilities=['lifecycle.observe']))
cases = 0
def run(event, expected, first=handshake, target=None, outer=session, sequence=3):
    global cases
    cases += 1
    final = packet(sequence, dict(type='event', event=event))
    final['session_id'] = outer
    records = [first, packet(2, dict(type='event', event=dict(type='target.changed', target=target))), final]
    with tempfile.TemporaryDirectory() as folder:
        result = subprocess.run(['/usr/bin/python3', sys.argv[1]],
            input=b''.join(json.dumps(v).encode()+b'\n' for v in records),
            capture_output=True, timeout=6, env={'WAIFU_EXTENSION_DATA':folder})
        assert (result.returncode == 0) == expected, (event, result.stderr)
        assert result.stdout == b'', 'metadata observer requested an action'
        receipts = list(pathlib.Path(folder).glob('*.json'))
        assert len(receipts) == int(expected)
        if expected:
            name = 'latest-stop-request.json' if event['type'] == 'extension.stopping' else 'latest-exit.json'
            assert receipts[0].name == name
            assert json.loads(receipts[0].read_text()) == event
for reason in ['disabled', 'removed', 'restarted', 'reconfigured', 'host_shutdown']:
    run(dict(source, type='extension.stopping', reason=reason), True)
    run(dict(source, type='extension.stopped', exit_code=None, requested_reason=reason), True)
for code in [0, 7]:
    run(dict(source, type='extension.stopped', exit_code=code, requested_reason=None), True)
terminal = dict(source, type='extension.stopped', exit_code=0, requested_reason=None)
for bad in [dict(terminal, installation_id='00000000-0000-0000-0000-000000000000'),
            dict(terminal, session_id='not-a-uuid'), dict(terminal, process_generation=0),
            dict(terminal, process_generation=True), dict(terminal, process_generation=1 << 64),
            dict(terminal, error='must not persist'), dict(terminal, exit_code=True),
            dict(terminal, exit_code=1 << 31), dict(terminal, requested_reason='unknown'),
            dict(terminal, requested_reason=[]),
            dict(source, type='extension.stopping', reason='unknown'),
            dict(source, type='extension.stopping', reason='disabled', exit_code=0)]:
    run(bad, False)
run(terminal, False, first=packet(1, dict(type='handshake', capabilities=['lifecycle.observe','motion.play'])))
run(terminal, False, target={'uuid':source['installation_id'], 'generation':1})
run(terminal, False, outer=source['session_id'])
run(terminal, False, sequence=2)
print(f'{cases} real Python stop/exit observer cases passed')
"#)
        .arg(script)
        .output()
        .expect("run integration exit observer driver");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&result.stdout).trim());
}
