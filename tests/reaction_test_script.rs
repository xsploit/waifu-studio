#![cfg(target_os = "linux")]
//! Real offline example subprocesses; no credentials, network, or providers.
const DRIVER: &str = r#"
import json, os, pathlib, select, subprocess, sys, tempfile, time
script = pathlib.Path(sys.argv[1]) / 'assets/extensions/twitch-live/test.py'
session = '00000000-0000-4000-8000-000000000010'
target = {'uuid':'00000000-0000-4000-8000-000000000011', 'generation':7}
all_grants = ['lifecycle.observe', 'expression.apply', 'motion.play']

class Child:
    def __init__(self, settings, grants=all_grants):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self.tmp.name) / 'latest-test.json'
        # Deliberately no provider/proxy/token environment inherited.
        self.p = subprocess.Popen(['/usr/bin/python3', str(script)], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={'WAIFU_EXTENSION_DATA':self.tmp.name})
        self.sequence = 0
        self.send({'type':'handshake', 'settings':settings, 'capabilities':grants})
    def send(self, message, **overrides):
        self.sequence += 1
        packet = dict(api_version=1, type='host', session_id=session, process_generation=5,
                      sequence=self.sequence, message=message)
        packet.update(overrides)
        self.p.stdin.write((json.dumps(packet)+'\n').encode())
        self.p.stdin.flush()
    def event(self, event):
        self.send({'type':'event','event':event})
    def select(self, value=target):
        self.event({'type':'target.changed','target':value})
    def action(self):
        data = bytearray()
        deadline = time.monotonic()+2
        while not data.endswith(b'\n'):
            remaining = deadline-time.monotonic()
            assert remaining > 0 and select.select([self.p.stdout], [], [], remaining)[0], 'action timeout'
            part = os.read(self.p.stdout.fileno(), 1)
            assert part, 'child exited before action'
            data.extend(part)
            assert len(data) <= 65536
        action = json.loads(data)
        assert action['target'] == target and action['session_id'] == session
        assert action['process_generation'] == 5 and action['type'] == 'action'
        assert action['sequence'] > 0
        return action
    def ack(self, action, status='ok', **kwargs):
        message = {'type':'result', 'id':action['id'], 'target':target,
                   'result':{'status':status, 'value':{'type':'accepted'}}}
        message.update(kwargs)
        self.send(message)
    def terminal(self, action, **kwargs):
        event = dict(type='motion.finished', action_id=action['id'], target=target,
                     motion_id=action['action']['motion_id'], outcome='completed')
        event.update(kwargs)
        self.event(event)
    def state(self, expected):
        deadline = time.monotonic()+2
        while True:
            try:
                raw = self.path.read_bytes()
                assert len(raw) <= 1024
                value = json.loads(raw)
                if value == {'state':expected}: return
            except (FileNotFoundError, json.JSONDecodeError): pass
            assert time.monotonic() < deadline, (expected, self.path.read_text() if self.path.exists() else None)
            time.sleep(.002)
    def quiet(self):
        assert self.p.poll() is None, 'child exited before terminal/expiry'
        assert not select.select([self.p.stdout], [], [], .05)[0], 'unexpected action/output'
    def finish(self, state='completed'):
        code = self.p.wait(timeout=4)
        assert (code == 0) == (state in ('completed', 'nothing_selected_or_permitted')), (state, code)
        self.state(state)
        assert not self.p.stdout.read(), 'unexpected extra action'
    def close(self):
        if self.p.poll() is None: self.p.kill()
        self.p.communicate(timeout=2)
        self.tmp.cleanup()

def motion_cases():
    for settings, grants, expected in [
        ({}, ['lifecycle.observe','motion.play'], 'studio.integration.twitch/nod'),
        ({'celebration_motion':'org.example.pack/custom'}, all_grants, 'org.example.pack/custom'),
        ({'celebration_expression':'Happy Face'}, ['lifecycle.observe','motion.play'], 'studio.integration.twitch/nod'),
    ]:
        child = Child(settings, grants)
        try:
            child.select()
            action = child.action()
            assert action['action'] == {'type':'motion.play','motion_id':expected,'mode':'one_shot'}
            child.ack(action)
            child.ack(action)
            child.quiet()  # Acceptance and duplicate acceptance cannot finish.
            child.terminal(action, action_id='unrelated')
            child.terminal(action, target=dict(target, generation=8))
            child.terminal(action, motion_id='wrong/motion')
            child.quiet()
            child.terminal(action)
            child.finish()
        finally: child.close()

def expression_cases():
    for motion, grants in [('', all_grants), ('org.example.pack/custom', all_grants),
                           ('org.example.pack/custom', ['lifecycle.observe','expression.apply'])]:
        child = Child({'celebration_expression':'Smiling Eyes 笑顔', 'celebration_motion':motion}, grants)
        try:
            child.select()
            expression = child.action()
            assert expression['action'] == {'type':'expression.apply','name':'Smiling Eyes 笑顔',
                                             'strength':.75,'duration_ms':2000}
            child.ack(expression, target=dict(target, generation=8))
            child.quiet()
            started = time.monotonic()
            child.ack(expression)
            child.ack(expression)
            if motion and 'motion.play' in grants:
                action = child.action()
                assert action['sequence'] > expression['sequence'] and action['id'] != expression['id']
                assert action['action']['motion_id'] == motion
                child.ack(action)
                child.terminal(action)
                child.state('motion_completed')
            else: child.state('expression_accepted')
            child.quiet()
            child.finish()
            assert time.monotonic()-started >= 1.95, 'preview expression cancelled before full duration'
        finally: child.close()

def no_actions_and_invalid():
    for settings, grants, state in [
        ({'celebration_expression':'','celebration_motion':''}, all_grants, 'nothing_selected_or_permitted'),
        ({}, ['lifecycle.observe'], 'nothing_selected_or_permitted'),
        ({}, ['motion.play'], 'observation_required'),
        ({'celebration_expression':'blinkLeft'}, all_grants, 'invalid_settings'),
        ({'celebration_expression':'x'*129}, all_grants, 'invalid_settings'),
        ({'celebration_motion':'x'*194}, all_grants, 'invalid_settings'),
        ({'celebration_motion':'bad motion'}, all_grants, 'invalid_settings'),
    ]:
        child = Child(settings, grants)
        try: child.finish(state)
        finally: child.close()

def abort_cases():
    for reason in ['target', 'reject', 'session', 'generation', 'expression_wait_target', 'motion_failed']:
        child = Child({'celebration_expression':'Happy Face', 'celebration_motion':''})
        try:
            child.select()
            action = child.action()
            if reason == 'motion_failed':
                child.close()
                child = Child({'celebration_motion':'org.example.pack/custom'})
                child.select()
                action = child.action()
                child.ack(action)
                child.terminal(action, outcome='failed')
                child.finish('motion_not_completed')
            elif reason == 'expression_wait_target':
                child.ack(action)
                child.state('expression_accepted')
                child.select(dict(target, generation=8))
                child.finish('target_changed')
            elif reason == 'target':
                child.select(dict(target, generation=8))
                child.finish('target_changed')
            elif reason == 'reject':
                child.ack(action, status='error')
                child.finish('rejected')
            else:
                child.send({'type':'event','event':{'type':'target.changed','target':target}},
                    **({'session_id':'00000000-0000-4000-8000-000000000099'} if reason == 'session'
                       else {'process_generation':6}))
                child.finish('aborted')
        finally: child.close()

{'motion':motion_cases, 'expression':expression_cases, 'none':no_actions_and_invalid,
 'abort':abort_cases}[sys.argv[2]]()
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
fn offline_motion_uses_default_custom_old_grants_and_actual_correlated_terminal() {
    run("motion");
}
#[test]
fn offline_expression_preserves_full_duration_with_or_without_early_motion_terminal() {
    run("expression");
}
#[test]
fn offline_none_missing_permissions_and_invalid_settings_emit_no_actions() {
    run("none");
}
#[test]
fn offline_rejection_target_or_session_change_aborts_without_retarget_or_retry() {
    run("abort");
}
