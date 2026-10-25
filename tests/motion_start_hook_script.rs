#![cfg(target_os = "linux")]
//! Real offline children validate general playback metadata without avatar authority.
#[test]
fn motion_start_observer_keeps_source_identity_separate_and_never_requests_actions() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/extensions/motion-start-observer/observer.py");
    let result = std::process::Command::new("/usr/bin/python3")
        .arg("-c")
        .arg(r#"
import json, pathlib, subprocess, sys, tempfile
outer = '00000000-0000-4000-8000-000000000001'
playback = '00000000-0000-4000-8000-000000000002'
target = dict(uuid='00000000-0000-4000-8000-000000000003', generation=7)
source = dict(type='extension', integration_id='org.example.source',
              session_id='00000000-0000-4000-8000-000000000004', process_generation=8, action_id='nod-1')
def packet(n, message):
    return dict(api_version=1,type='host',session_id=outer,process_generation=1,sequence=n,message=message)
handshake = packet(1,dict(type='handshake',capabilities=['lifecycle.observe']))
base = dict(type='motion.playback.started',playback_id=playback,source_target=None,
            motion_id=None,source={'type':'host'})
cases = 0
def run(event, success, first=handshake, active_target=None, session=outer):
    global cases
    cases += 1
    final = packet(3,dict(type='event',event=event));final['session_id']=session
    records = [first,packet(2,dict(type='event',event=dict(type='target.changed',target=active_target))),final]
    with tempfile.TemporaryDirectory() as folder:
        result = subprocess.run(['/usr/bin/python3',sys.argv[1]],
            input=b''.join(json.dumps(v).encode()+b'\n' for v in records),capture_output=True,
            timeout=6,env={'WAIFU_EXTENSION_DATA':folder})
        assert (result.returncode==0)==success,(event,result.stderr)
        assert result.stdout==b'', 'metadata hook requested an action'
        receipt=pathlib.Path(folder)/'latest-motion-start.json'
        assert receipt.exists()==success
        if success:assert json.loads(receipt.read_text())==event
run(base,True)
run(dict(base,source_target=target,motion_id='org.example.pack/nod',source=source),True)
for bad in [dict(base,playback_id='00000000-0000-0000-0000-000000000000'),
            dict(base,outcome='completed'),dict(base,outcome=True),dict(base,error='must not persist'),
            dict(base,motion_id='/home/private.bvh'),dict(base,motion_id='C:\\private.bvh'),
            dict(base,motion_id='x'*194),dict(base,source_target=dict(target,generation=True)),
            dict(base,source_target=dict(target,generation=-1)),dict(base,source_target=dict(target,path='private')),
            dict(base,source=dict(type='host',path='private')),
            dict(base,source=dict(source,process_generation=0)),dict(base,source=dict(source,process_generation=True)),
            dict(base,source=dict(source,session_id='not-a-uuid')),dict(base,source=dict(source,action_id=' ')),
            dict(base,source=dict(source,integration_id='x'*129)),dict(base,source=dict(type='unknown'))]:
    run(bad,False)
for extra in ['motion.play','expression.apply','message.deliver','event.emit','catalog.get','reply.read']:
    run(base,False,first=packet(1,dict(type='handshake',capabilities=['lifecycle.observe',extra])))
run(base,False,active_target=target)
run(base,False,session=source['session_id'])
print(f'{cases} real Python motion-start observer cases passed')
"#)
        .arg(script)
        .output()
        .expect("run motion end observer driver");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&result.stdout).trim());
}
