#![cfg(target_os = "linux")]
//! Offline real-child checks for the metadata-only bundled selection observer.
#[test]
fn selection_observer_rejects_wrong_authority_and_never_requests_actions() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/extensions/selection-observer/observer.py");
    let result = std::process::Command::new("/usr/bin/python3")
        .arg("-c")
        .arg(r#"
import json, os, pathlib, subprocess, sys, tempfile
script = sys.argv[1]
session = '00000000-0000-4000-8000-000000000001'
def packet(sequence, message, session_id=session):
    return dict(api_version=1, type='host', session_id=session_id,
                process_generation=1, sequence=sequence, message=message)
handshake = packet(1, dict(type='handshake', capabilities=['lifecycle.observe']))
event = dict(type='character.selected', previous=None,
             new='00000000-0000-4000-8000-000000000002', selection_revision=1)
selected = packet(3, dict(type='event', event=event))
def run(records, success):
    with tempfile.TemporaryDirectory() as folder:
        data = b''.join(json.dumps(record).encode()+b'\n' for record in records)
        child = subprocess.run(['/usr/bin/python3', script], input=data,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=6,
            env={'WAIFU_EXTENSION_DATA':folder})
        assert (child.returncode == 0) == success, child.stderr
        assert child.stdout == b'', 'metadata observer emitted an action'
        receipt = pathlib.Path(folder)/'latest-selection.json'
        assert receipt.exists() == success
        if success: assert json.loads(receipt.read_text()) == event
run([handshake, packet(2, dict(type='event', event=dict(type='target.changed', target=None))), selected], True)
run([handshake, selected], True)
run([packet(1, dict(type='handshake', capabilities=[])), selected], False)
run([handshake, packet(2, dict(type='event', event=dict(type='target.changed', target={'uuid':event['new'], 'generation':1}))), selected], False)
run([handshake, packet(2, dict(type='event', event=event), '00000000-0000-4000-8000-000000000099')], False)
run([handshake, packet(1, dict(type='event', event=event))], False)
run([handshake], False)
for bad in [dict(event, selection_revision=0), dict(event, selection_revision=True),
            dict(event, previous=event['new']), dict(event, new='not-a-uuid'),
            dict(event, reply_text='must not persist')]:
    run([handshake, packet(2, dict(type='event', event=bad))], False)
"#)
        .arg(script)
        .output()
        .expect("run selection observer driver");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
