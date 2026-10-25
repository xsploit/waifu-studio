//! Bounded process/session coordinator; no application effects are executed here.
//!
//! Host installation policy supplies origin, grants and process generation. Each
//! connection creates a fresh session. Call `drain` once per frame with the live
//! avatar target. Before executing ANY returned action (especially deferred work),
//! recheck `can_execute` with the live target and check that registered content
//! still exists. Attribute messages/events with `origin()`, never child input.
//! Complete denied/deferred work with an appropriate error ActionResult. Completion
//! records an outcome; it cannot undo an effect or prove authorization at effect time.
use crate::{
    extension_process::{ProcessEvent, ProcessHandle, ProcessSpec, ProcessTermination},
    extension_protocol::{
        API_VERSION, ActionEnvelope, ActionResult, Capability, HostEnvelope, HostEnvelopeType,
        HostEvent, HostMessage, IntegrationOrigin, Target, parse_action_line,
    },
    extension_session::{Admission, ExecutionTicket, SessionGate},
};
use std::collections::{BTreeSet, VecDeque};
use uuid::Uuid;

pub const MAX_EVENTS_PER_FRAME: usize = 16;
pub const MAX_LOGS: usize = 32;
pub const MAX_STATUS_BYTES: usize = 2048;

#[derive(Debug)]
pub struct DispatchedAction {
    pub action: ActionEnvelope,
    pub ticket: ExecutionTicket,
}

struct Pending {
    action: ActionEnvelope,
}

pub struct ExtensionConnection {
    process: ProcessHandle,
    gate: SessionGate,
    origin: IntegrationOrigin,
    grants: BTreeSet<Capability>,
    session_id: Uuid,
    process_generation: u64,
    host_sequence: u64,
    pending: Option<Pending>,
    stopped: bool,
    quiescing: bool,
    shutdown_observer: bool,
    status: String,
    logs: VecDeque<String>,
    termination: Option<ProcessTermination>,
}

impl ExtensionConnection {
    pub fn spawn(
        spec: ProcessSpec,
        origin: IntegrationOrigin,
        grants: impl IntoIterator<Item = Capability>,
        process_generation: u64,
    ) -> Result<Self, String> {
        Self::spawn_configured(spec, origin, grants, process_generation, Default::default())
    }

    pub fn spawn_configured(
        spec: ProcessSpec,
        origin: IntegrationOrigin,
        grants: impl IntoIterator<Item = Capability>,
        process_generation: u64,
        settings: crate::extension_settings::Settings,
    ) -> Result<Self, String> {
        crate::extension_settings::validate_wire_values(&settings)?;
        origin.validate().map_err(|e| e.to_string())?;
        let grants: BTreeSet<_> = grants.into_iter().collect();
        let session_id = Uuid::new_v4();
        let gate = SessionGate::new(
            session_id,
            process_generation,
            &origin.integration_id,
            grants.iter().copied(),
        )
        .map_err(|e| e.to_string())?;
        let process = ProcessHandle::spawn(spec)?;
        let mut connection = Self {
            process,
            gate,
            origin,
            grants,
            session_id,
            process_generation,
            host_sequence: 0,
            pending: None,
            stopped: false,
            quiescing: false,
            shutdown_observer: false,
            status: "Starting".into(),
            logs: VecDeque::new(),
            termination: None,
        };
        if let Err(error) = connection.send(HostMessage::Handshake {
            settings,
            origin: connection.origin.clone(),
            capabilities: connection.grants.iter().copied().collect(),
        }) {
            // Keep the supervisor and its retained OS-spawn evidence even if a
            // fast child exits before handshake admission. This is not Running.
            connection.stop(&format!("Handshake admission failed: {error}"));
        }
        Ok(connection)
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn process_generation(&self) -> u64 {
        self.process_generation
    }
    pub(crate) fn spawn_evidence(&self) -> Option<crate::extension_process::ProcessStarted> {
        self.process.started()
    }
    #[cfg(test)]
    pub(crate) fn discard_terminal_for_test(&self) {
        self.process.discard_terminal_for_test();
    }
    pub(crate) fn stop_request_boundary(&self) -> Option<std::time::Instant> {
        self.process.stop_request_boundary()
    }
    pub(crate) fn supervisor_finished(&self) -> bool {
        self.process.finished()
    }
    pub(crate) fn terminal_evidence(&self) -> Option<crate::extension_process::ProcessTermination> {
        self.process.termination()
    }
    pub fn grants(&self) -> &BTreeSet<Capability> {
        &self.grants
    }

    /// Publish an own-namespace emission or own delivery acknowledgement only.
    /// EventEmit authorizes emissions; MessageDeliver authorizes acknowledgements
    /// after the caller has persisted delivery. Both require this exact origin.
    /// This method creates no subscriptions; lifecycle observations use
    /// publish_lifecycle and separate grants.
    /// The caller must recheck the live target before publishing deferred events.
    pub fn publish_event(&mut self, event: HostEvent) -> Result<(), String> {
        match &event {
            HostEvent::AudioInputFinished { .. }
                if self.grants.contains(&Capability::AudioTranscribe)
                    && self.grants.contains(&Capability::MessageDeliver) => {}
            HostEvent::MessageDelivered { origin, .. }
                if self.grants.contains(&Capability::MessageDeliver) && origin == &self.origin => {}
            HostEvent::Emitted { origin, name, .. }
                if self.grants.contains(&Capability::EventEmit)
                    && origin == &self.origin
                    && name
                        .strip_prefix(&format!("integration.{}.", self.origin.integration_id))
                        .is_some_and(|suffix| !suffix.is_empty()) => {}
            _ => return Err("Event is not an authorized own emission or acknowledgement".into()),
        }
        self.send(HostMessage::Event { event })
    }

    /// Private PCM transport, independent of text/lifecycle observation grants.
    pub fn publish_audio_output(
        &mut self,
        output: crate::extension_protocol::AudioOutput,
    ) -> Result<(), String> {
        if !self.grants.contains(&Capability::AudioPlayback) {
            return Err("Audio playback permission is not granted".into());
        }
        self.send(HostMessage::AudioOutput { output })
    }

    /// Private media control. Playback permission alone permits current target
    /// identity, not general lifecycle/reply observation or message payloads.
    pub(crate) fn publish_audio_target(&mut self, target: Option<Target>) -> Result<(), String> {
        if !self.grants.contains(&Capability::AudioPlayback) {
            return Err("Audio playback permission is not granted".into());
        }
        self.send(HostMessage::Event {
            event: HostEvent::TargetChanged { target },
        })
    }

    /// Publish target changes, source-owned reply/speech/emotion observations and the
    /// originating session's correlated motion receipts. The caller routes
    /// motion receipts to their original connection and retains their original
    /// action target/IDs; ownership is never inferred from the selected avatar.
    /// Metadata requires LifecycleObserve, text ReplyRead.
    /// Grants are checked on every call, including after mid-reply revocation.
    /// This sends one event only; callers must budget lazy reply_events iteration
    /// per frame. Queue saturation follows send's fail-closed transport policy.
    pub fn publish_lifecycle(&mut self, event: HostEvent) -> Result<(), String> {
        let capability = match &event {
            HostEvent::ReplyText { .. } => Capability::ReplyRead,
            HostEvent::ExtensionStarted { .. }
            | HostEvent::MotionPlaybackStarted { .. }
            | HostEvent::MotionPlaybackEnded { .. }
            | HostEvent::ExtensionStopping { .. }
            | HostEvent::ExtensionStopped { .. }
            | HostEvent::AppReady { .. }
            | HostEvent::CharacterSelected { .. }
            | HostEvent::ReplyFinished { .. }
            | HostEvent::SpeechStarted { .. }
            | HostEvent::SpeechStopped { .. }
            | HostEvent::EmotionChanged { .. }
            | HostEvent::TargetChanged { .. }
            | HostEvent::MotionFinished { .. } => Capability::LifecycleObserve,
            _ => return Err("Event is not a supported lifecycle observation".into()),
        };
        if !self.grants.contains(&capability) {
            return Err("Lifecycle event capability is not granted".into());
        }
        self.send(HostMessage::Event { event })
    }

    pub fn origin(&self) -> &IntegrationOrigin {
        &self.origin
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn logs(&self) -> &VecDeque<String> {
        &self.logs
    }
    /// Actual supervisor receipt, never inferred from a stop request or queued event.
    /// Call drain even after stopping to observe asynchronous kill/reap completion.
    pub fn termination(&self) -> Option<&ProcessTermination> {
        self.termination.as_ref()
    }
    pub fn is_active(&self) -> bool {
        !self.stopped
            && !self.quiescing
            && self.process.termination().is_none()
            && !self.process.finished()
    }

    /// Revocation takes effect immediately for admissions and can_execute checks.
    /// This does not resend the startup handshake or erase replay history.
    pub fn set_grants(&mut self, grants: impl IntoIterator<Item = Capability>) {
        let grants: BTreeSet<_> = grants.into_iter().collect();
        if self.quiescing {
            if !grants.contains(&Capability::LifecycleObserve) {
                self.stop("Shutdown observation revoked");
            }
            return;
        }
        let playback_revoked = self.grants.contains(&Capability::AudioPlayback)
            && !grants.contains(&Capability::AudioPlayback);
        self.grants = grants;
        self.gate.set_grants(self.grants.iter().copied());
        if playback_revoked {
            // Revoke queued media as well as future admissions. Never replay it
            // on a regrant; this incarnation must be explicitly restarted.
            self.stop("Audio playback permission revoked");
        }
    }

    /// Recheck immediately before the effect, on the same host thread as changes
    /// to target/grants. Do not treat admission or a previous check as a lease.
    pub fn can_execute(&self, work: &DispatchedAction, current_target: Target) -> bool {
        self.is_active()
            && work.action.session_id == self.session_id
            && work.action.process_generation == self.process_generation
            && work.action.target == current_target
            && self
                .grants
                .contains(&work.action.action.required_capability())
            && self.pending.as_ref().is_some_and(|p| {
                p.action.sequence == work.ticket.sequence() && p.action == work.action
            })
    }

    /// Bounded event work, including logs/start/exit events (not just action lines).
    /// At most one execution can be outstanding. A failure invalidates all work
    /// collected during this drain; pending duplicates produce no dispatch/reply.
    pub fn drain(&mut self, current_target: Target) -> Vec<DispatchedAction> {
        self.drain_target(Some(current_target))
    }

    /// A missing avatar rejects new requests with StaleTarget; cached/pending
    /// duplicates retain their original semantics. No synthetic target is used.
    pub fn drain_target(&mut self, current_target: Option<Target>) -> Vec<DispatchedAction> {
        self.observe_termination();
        let mut dispatched = Vec::new();
        if self.stopped || self.quiescing {
            if self.stopped {
                // A retained terminal receipt can arrive before the render
                // frame drains stderr. Keep bounded diagnostics, but never
                // admit actions from an already terminated incarnation.
                for _ in 0..MAX_EVENTS_PER_FRAME {
                    match self.process.poll() {
                        Some(ProcessEvent::Log(log)) => self.log(&log),
                        Some(_) => {}
                        None => break,
                    }
                }
            }
            return dispatched;
        }
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Some(event) = self.process.poll() else {
                break;
            };
            match event {
                ProcessEvent::Started { .. } => self.status = "Running".into(),
                ProcessEvent::Log(log) => {
                    self.log(&log);
                }
                ProcessEvent::Exited { .. } => {
                    // Event queues can overflow. Only the retained receipt is authoritative.
                    self.observe_termination();
                    if !self.stopped {
                        self.stop("Process exit notification");
                    }
                    break;
                }
                ProcessEvent::Line(line) => {
                    let action = match parse_action_line(&line) {
                        Ok(action) => action,
                        Err(error) => {
                            self.stop(&format!("Invalid extension output: {error}"));
                            break;
                        }
                    };
                    match self.gate.admit_target(&action, current_target) {
                        Admission::Execute(ticket) => {
                            self.pending = Some(Pending {
                                action: action.clone(),
                            });
                            dispatched.push(DispatchedAction { action, ticket });
                        }
                        Admission::Pending => self.log("Duplicate action still pending"),
                        Admission::Cached(result) | Admission::Rejected(result) => {
                            if self
                                .send(HostMessage::Result {
                                    id: action.id,
                                    target: action.target,
                                    result,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            }
        }
        self.observe_termination();
        if self.stopped {
            dispatched.clear();
        }
        dispatched
    }

    /// Uses stored correlation/target, not caller-modifiable envelope fields.
    /// Invalid/foreign tickets or results fail closed. Never retries a failed send.
    pub fn complete(
        &mut self,
        ticket: &ExecutionTicket,
        result: ActionResult,
    ) -> Result<(), String> {
        self.observe_termination();
        if !self.is_active() {
            return Err("Extension connection is stopped".into());
        }
        if let Err(error) = self.gate.complete(ticket, result.clone()) {
            let error = error.to_string();
            self.stop(&error);
            return Err(error);
        }
        let Some(pending) = self.pending.take() else {
            self.stop("Missing pending execution");
            return Err("Missing pending execution".into());
        };
        self.send(HostMessage::Result {
            id: pending.action.id,
            target: pending.action.target,
            result,
        })
    }

    /// Nonblocking: termination remains None until the supervisor has killed/reaped.
    pub fn stop(&mut self, reason: &str) {
        if !self.stopped {
            self.status = bounded(reason);
        }
        self.stopped = true;
        self.pending = None;
        self.process.stop();
    }

    /// No dispatch, response replay or ordinary lifecycle publication survives
    /// this boundary. Notification eligibility is pinned without adding grants.
    pub(crate) fn begin_quiesce(&mut self, persistent_observer: bool) {
        if self.quiescing {
            return;
        }
        self.observe_termination();
        self.shutdown_observer = persistent_observer
            && self.is_active()
            && self.grants.contains(&Capability::LifecycleObserve)
            && self.process.has_written_record(); // handshake must precede teardown
        self.quiescing = true;
        self.pending = None;
        self.gate.quiesce();
        self.grants
            .retain(|grant| self.shutdown_observer && *grant == Capability::LifecycleObserve);
        self.process.quiesce_input();
        if !self.shutdown_observer {
            self.stop("Host shutdown");
        } else {
            self.status = "Quiescing for host shutdown".into();
        }
    }

    /// Admission into the transport is not evidence that the child received it.
    pub(crate) fn offer_stopping(&mut self, app_run_id: Uuid, deadline: std::time::Instant) {
        if !self.quiescing
            || self.stopped
            || !self.shutdown_observer
            || !self.grants.contains(&Capability::LifecycleObserve)
        {
            return;
        }
        let Some(sequence) = self.host_sequence.checked_add(1) else {
            self.stop("Host sequence exhausted");
            return;
        };
        let envelope = HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: self.session_id,
            sequence,
            process_generation: self.process_generation,
            message: HostMessage::Event {
                event: HostEvent::AppStopping { app_run_id },
            },
        };
        if let Err(error) = self.process.request_graceful(&envelope, deadline) {
            self.stop(&error);
        } else {
            self.host_sequence = sequence;
            self.status = "Shutdown metadata admitted (best effort)".into();
        }
    }

    pub(crate) fn reap_shutdown(&mut self) -> bool {
        // Discard valid queued requests without admission or responses. Invalid
        // protocol still force-stops immediately during the grace interval.
        for _ in 0..MAX_EVENTS_PER_FRAME {
            match self.process.poll() {
                Some(ProcessEvent::Line(line)) if parse_action_line(&line).is_err() => {
                    self.stop("Invalid extension output during shutdown");
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        let finished = self.process.reap_finished();
        self.observe_termination();
        finished
    }

    /// Explicit shutdown outside render-frame teardown. Signals stop immediately,
    /// then waits at most the transport's timeout for supervisor completion.
    pub fn shutdown(&mut self, timeout: std::time::Duration) -> bool {
        self.stop("Connection shutdown requested");
        let finished = self.process.shutdown(timeout);
        self.observe_termination();
        finished
    }

    fn log(&mut self, message: &str) {
        if self.logs.len() == MAX_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back(bounded(message));
    }

    fn send(&mut self, message: HostMessage) -> Result<(), String> {
        if !self.is_active() {
            self.stop("Extension transport stopped");
            return Err("Extension transport stopped".into());
        }
        let Some(sequence) = self.host_sequence.checked_add(1) else {
            self.stop("Host sequence exhausted");
            return Err("Host sequence exhausted".into());
        };
        let envelope = HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: self.session_id,
            sequence,
            process_generation: self.process_generation,
            message,
        };
        if let Err(error) = self.process.send_host(&envelope) {
            self.stop(&error);
            return Err(error);
        }
        self.host_sequence = sequence;
        Ok(())
    }

    fn observe_termination(&mut self) {
        if self.termination.is_none() {
            if let Some(receipt) = self.process.termination() {
                if !self.stopped {
                    self.status = bounded(&receipt.reason);
                }
                self.stopped = true;
                self.pending = None;
                self.termination = Some(receipt);
            } else if self.process.finished() {
                self.stop("Supervisor finished without terminal receipt");
            }
        }
    }
}

fn bounded(text: &str) -> String {
    let mut end = text.len().min(MAX_STATUS_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::extension_protocol::ResultValue;
    use std::time::{Duration, Instant};

    struct Fixture {
        connection: ExtensionConnection,
        root: std::path::PathBuf,
    }
    impl Fixture {
        fn new(body: &str) -> Self {
            let root = std::env::temp_dir().join(format!("waifu-connection-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let script = format!(
                r#"
import json, sys, time
h = json.loads(sys.stdin.readline())
assert h['type'] == 'host' and h['sequence'] == 1
assert h['message']['type'] == 'handshake'
assert h['message']['origin']['integration_id'] == 'org.example.connection'
assert h['message']['capabilities'] == ['catalog.get']
a = {{'api_version': 1, 'type': 'action', 'id': 'one', 'sequence': 1,
     'session_id': h['session_id'], 'process_generation': h['process_generation'],
     'target': {{'uuid': '00000000-0000-0000-0000-000000000002', 'generation': 3}},
     'action': {{'type': 'catalog.get'}}}}
def emit():
    print(json.dumps(a), flush=True)
def result(sequence):
    r = json.loads(sys.stdin.readline())
    assert r['sequence'] == sequence and r['session_id'] == h['session_id']
    assert r['process_generation'] == h['process_generation']
    assert r['message']['type'] == 'result' and r['message']['id'] == 'one'
    assert r['message']['target'] == a['target']
    assert r['message']['result']['status'] == 'ok'
{body}
"#
            );
            let connection = ExtensionConnection::spawn(
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-u".into(), "-c".into(), script],
                    working_directory: root.clone(),
                    data_directory: root.join("data"),
                    lifetime: Some(Duration::from_secs(5)),
                },
                IntegrationOrigin {
                    integration_id: "org.example.connection".into(),
                    display_name: "Connection fixture".into(),
                },
                [Capability::CatalogGet],
                7,
            )
            .unwrap();
            Self { connection, root }
        }

        fn until(
            &mut self,
            mut visit: impl FnMut(&mut ExtensionConnection, Vec<DispatchedAction>) -> bool,
        ) {
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                let work = self.connection.drain(target());
                if visit(&mut self.connection, work) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "coordinator timed out: {}",
                    self.connection.status()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Test cleanup only; ordinary connection teardown never joins.
            self.connection.process.shutdown(Duration::from_secs(1));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn target() -> Target {
        Target {
            uuid: Uuid::from_u128(2),
            generation: 3,
        }
    }
    fn accepted() -> ActionResult {
        ActionResult::Ok {
            value: ResultValue::Accepted {},
        }
    }

    #[test]
    fn quiesce_invalidates_already_admitted_ticket_and_never_dispatches_queued_actions() {
        let mut fixture = Fixture::new("emit()\nfor line in sys.stdin: pass");
        fixture
            .connection
            .set_grants([Capability::CatalogGet, Capability::LifecycleObserve]);
        let mut admitted = None;
        fixture.until(|_, mut work| {
            if let Some(action) = work.pop() {
                admitted = Some(action);
                true
            } else {
                false
            }
        });
        let work = admitted.unwrap();
        assert!(fixture.connection.can_execute(&work, target()));
        fixture.connection.begin_quiesce(true);
        assert!(!fixture.connection.can_execute(&work, target()));
        assert!(fixture.connection.pending.is_none());
        assert!(fixture.connection.drain(target()).is_empty());
        assert!(
            fixture
                .connection
                .complete(&work.ticket, accepted())
                .is_err()
        );
        // Restoring action grants after the boundary cannot revive admission.
        fixture
            .connection
            .set_grants([Capability::CatalogGet, Capability::LifecycleObserve]);
        assert_eq!(
            fixture.connection.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        assert!(!fixture.connection.is_active());
        // Dropping observation permission during grace still force-stops.
        fixture.connection.set_grants([]);
        assert!(fixture.connection.stopped);
        assert!(fixture.connection.shutdown(Duration::from_secs(1)));
        assert!(fixture.connection.termination().is_some());
    }

    #[test]
    fn real_child_own_delivery_requires_exact_origin_and_live_delivery_grant() {
        let mut fixture = Fixture::new(
            r#"
for sequence in [2, 3]:
    r = json.loads(sys.stdin.readline())
    assert r['sequence'] == sequence and r['session_id'] == h['session_id']
    assert r['process_generation'] == h['process_generation']
    assert r['message']['type'] == 'event'
    e = r['message']['event']
    assert e['type'] == 'message.delivered'
    assert e['origin'] == h['message']['origin']
    assert e['target'] == a['target'] and e['text'] == 'Persisted message'
"#,
        );
        let connection = &mut fixture.connection;
        let own = HostEvent::MessageDelivered {
            target: target(),
            text: "Persisted message".into(),
            origin: connection.origin().clone(),
        };
        assert!(connection.publish_event(own.clone()).is_err());
        connection.set_grants([Capability::LifecycleObserve]);
        assert!(connection.publish_event(own.clone()).is_err());
        assert_eq!(connection.host_sequence, 1);

        connection.set_grants([Capability::MessageDeliver]);
        for alter_display_name in [false, true] {
            let mut foreign = own.clone();
            let HostEvent::MessageDelivered { origin, .. } = &mut foreign else {
                unreachable!();
            };
            if alter_display_name {
                origin.display_name = "Other display name".into();
            } else {
                origin.integration_id = "org.example.other".into();
            }
            assert!(connection.publish_event(foreign).is_err());
            assert_eq!(connection.host_sequence, 1);
        }
        // An acknowledgement does not require observing anyone's lifecycle.
        connection.publish_event(own.clone()).unwrap();
        assert_eq!(connection.host_sequence, 2);
        connection.set_grants([Capability::EventEmit, Capability::LifecycleObserve]);
        assert!(connection.publish_event(own.clone()).is_err());
        assert_eq!(connection.host_sequence, 2);
        connection.set_grants([Capability::MessageDeliver]);
        connection.publish_event(own).unwrap();
        assert_eq!(connection.host_sequence, 3);
        fixture.until(|connection, work| {
            assert!(work.is_empty());
            connection.termination().is_some()
        });
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
    }

    #[test]
    fn real_child_emissions_keep_own_namespace_and_separate_grant() {
        let mut fixture = Fixture::new(
            r#"
r = json.loads(sys.stdin.readline())
assert r['sequence'] == 2 and r['session_id'] == h['session_id']
assert r['process_generation'] == h['process_generation']
assert r['message']['type'] == 'event'
e = r['message']['event']
assert e['type'] == 'event.emitted' and e['origin'] == h['message']['origin']
assert e['name'] == 'integration.org.example.connection.ready'
assert e['target'] == a['target'] and e['data'] == {'ready': True}
"#,
        );
        let connection = &mut fixture.connection;
        let own = HostEvent::Emitted {
            target: target(),
            name: "integration.org.example.connection.ready".into(),
            data: serde_json::json!({"ready": true}),
            origin: connection.origin().clone(),
        };
        connection.set_grants([Capability::MessageDeliver]);
        assert!(connection.publish_event(own.clone()).is_err());
        connection.set_grants([Capability::EventEmit]);
        for name in [
            "integration.org.example.other.ready",
            "integration.org.example.connection.",
            "integration.org.example.connectionExtra.ready",
        ] {
            let mut foreign = own.clone();
            let HostEvent::Emitted { name: field, .. } = &mut foreign else {
                unreachable!();
            };
            *field = name.into();
            assert!(connection.publish_event(foreign).is_err());
        }
        let mut foreign = own.clone();
        let HostEvent::Emitted { origin, .. } = &mut foreign else {
            unreachable!();
        };
        origin.integration_id = "org.example.other".into();
        assert!(connection.publish_event(foreign).is_err());
        assert_eq!(connection.host_sequence, 1);
        connection.publish_event(own).unwrap();
        assert_eq!(connection.host_sequence, 2);
        fixture.until(|connection, work| {
            assert!(work.is_empty());
            connection.termination().is_some()
        });
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
    }

    #[test]
    fn real_child_missing_avatar_rejection_stays_cached_after_avatar_returns() {
        let mut fixture = Fixture::new(
            r#"
emit()
r = json.loads(sys.stdin.readline())
assert r['sequence'] == 2 and r['message']['result']['code'] == 'stale_target'
emit()
r = json.loads(sys.stdin.readline())
assert r['sequence'] == 3 and r['message']['result']['code'] == 'stale_target'
a['sequence'] = 2
emit()
result(4)
"#,
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut executions = 0;
        loop {
            let current = if fixture.connection.host_sequence == 1 {
                None
            } else {
                Some(target())
            };
            for work in fixture.connection.drain_target(current) {
                assert_eq!(work.action.sequence, 2);
                fixture
                    .connection
                    .complete(&work.ticket, accepted())
                    .unwrap();
                executions += 1;
            }
            if fixture.connection.termination().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "missing-avatar exchange timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(executions, 1);
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
    }

    #[test]
    fn real_child_target_notifications_require_lifecycle_observe() {
        let mut fixture = Fixture::new(
            r#"
for sequence in [2, 3]:
    r = json.loads(sys.stdin.readline())
    assert r['sequence'] == sequence
    e = r['message']['event']
    assert e['type'] == 'target.changed'
    if sequence == 2:
        assert e['target'] is None
    else:
        assert e['target'] == a['target']
"#,
        );
        fixture.connection.set_grants([Capability::ReplyRead]);
        assert!(
            fixture
                .connection
                .publish_lifecycle(HostEvent::TargetChanged { target: None })
                .is_err()
        );
        fixture
            .connection
            .set_grants([Capability::LifecycleObserve]);
        fixture
            .connection
            .publish_lifecycle(HostEvent::TargetChanged { target: None })
            .unwrap();
        fixture
            .connection
            .publish_lifecycle(HostEvent::TargetChanged {
                target: Some(target()),
            })
            .unwrap();
        fixture.until(|connection, work| {
            assert!(work.is_empty());
            connection.termination().is_some()
        });
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
    }

    #[test]
    fn real_child_motion_receipts_require_observation_and_recheck_revocation() {
        use crate::extension_protocol::MotionOutcome;
        let mut fixture = Fixture::new(
            r#"
for sequence, outcome in [(2, 'completed'), (3, 'cancelled'), (4, 'failed')]:
    r = json.loads(sys.stdin.readline())
    assert r['sequence'] == sequence and r['session_id'] == h['session_id']
    assert r['process_generation'] == h['process_generation']
    assert r['message']['type'] == 'event'
    e = r['message']['event']
    assert e['type'] == 'motion.finished'
    assert e['motion_id'] == 'org.example.pack/nod'
    assert e['action_id'] == 'motion-' + str(sequence - 1)
    assert e['outcome'] == outcome
    assert e['target'] == {'uuid': a['target']['uuid'], 'generation': 2}
"#,
        );
        let connection = &mut fixture.connection;
        // A terminal receipt retains the original target even when a newer
        // target is supplied to drain. Action/ownership checks belong upstream.
        let mut original_target = target();
        original_target.generation -= 1;
        let receipt = |action: &str, outcome| HostEvent::MotionFinished {
            target: original_target,
            motion_id: "org.example.pack/nod".into(),
            action_id: action.into(),
            outcome,
        };
        let completed = receipt("motion-1", MotionOutcome::Completed);
        assert!(connection.publish_lifecycle(completed.clone()).is_err());
        assert_eq!(connection.host_sequence, 1);
        assert!(connection.is_active());
        connection.set_grants([Capability::LifecycleObserve]);
        connection.publish_lifecycle(completed).unwrap();

        let cancelled = receipt("motion-2", MotionOutcome::Cancelled);
        connection.set_grants([Capability::MotionPlay, Capability::ReplyRead]);
        assert!(connection.publish_lifecycle(cancelled.clone()).is_err());
        assert_eq!(connection.host_sequence, 2);
        assert!(connection.is_active());
        connection.set_grants([Capability::LifecycleObserve]);
        connection.publish_lifecycle(cancelled).unwrap();

        let failed = receipt("motion-3", MotionOutcome::Failed);
        connection.set_grants([]);
        assert!(connection.publish_lifecycle(failed.clone()).is_err());
        assert_eq!(connection.host_sequence, 3);
        assert!(connection.is_active());
        connection.set_grants([Capability::LifecycleObserve]);
        connection.publish_lifecycle(failed).unwrap();
        assert_eq!(connection.host_sequence, 4);
        fixture.until(|connection, work| {
            assert!(work.is_empty());
            connection.termination().is_some()
        });
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
        assert!(
            fixture
                .connection
                .publish_lifecycle(receipt("motion-4", MotionOutcome::Completed))
                .is_err()
        );
        assert_eq!(fixture.connection.host_sequence, 4);
    }

    #[test]
    fn terminal_receipt_before_frame_preserves_logs_without_dispatching_queued_actions() {
        let mut fixture = Fixture::new(
            "emit()\nprint('Owned startup diagnostic', file=sys.stderr, flush=True)\nsys.exit(1)",
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        while !fixture.connection.process.finished() {
            assert!(Instant::now() < deadline, "fixture never exited");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture.connection.drain_target(Some(target())).is_empty());
        assert_eq!(fixture.connection.termination().unwrap().code, Some(1));
        assert!(
            fixture
                .connection
                .logs()
                .iter()
                .any(|line| line == "Owned startup diagnostic")
        );
        assert!(!fixture.connection.is_active());
    }

    #[test]
    fn lifecycle_and_text_grants_do_not_authorize_audio_and_revocation_is_terminal() {
        use crate::extension_protocol::AudioOutput;
        let mut fixture = Fixture::new("time.sleep(30)");
        let connection = &mut fixture.connection;
        let playback_id = Uuid::new_v4();
        let output = AudioOutput::Start {
            playback_id,
            target: target(),
        };
        connection.set_grants([Capability::LifecycleObserve, Capability::ReplyRead]);
        let sequence = connection.host_sequence;
        assert!(connection.publish_audio_output(output.clone()).is_err());
        assert_eq!(connection.host_sequence, sequence);
        connection.set_grants([Capability::AudioPlayback]);
        assert!(connection.publish_audio_output(output.clone()).is_ok());
        connection.set_grants([]);
        assert!(!connection.is_active());
        connection.set_grants([Capability::AudioPlayback]);
        assert!(connection.publish_audio_output(output).is_err());
        assert!(connection.shutdown(Duration::from_secs(2)));
    }

    #[test]
    fn explicit_shutdown_observes_authoritative_terminal_receipt() {
        let mut fixture = Fixture::new("time.sleep(30)");
        assert!(fixture.connection.shutdown(Duration::from_secs(2)));
        assert!(fixture.connection.termination().is_some());
        assert!(!fixture.connection.is_active());
        assert!(fixture.connection.drain_target(None).is_empty());
    }

    #[test]
    fn real_child_lifecycle_publication_separates_metadata_and_reply_text_grants() {
        use crate::extension_protocol::{
            EmotionOwner, MotionOutcome, ReplyOutcome, SpeechStopReason,
        };
        let mut fixture = Fixture::new(
            r#"
for sequence, kind in [(2, 'reply.finished'), (3, 'reply.text'), (4, 'speech.started'), (5, 'speech.stopped'), (6, 'emotion.changed')]:
    r = json.loads(sys.stdin.readline())
    assert r['sequence'] == sequence and r['session_id'] == h['session_id']
    e = r['message']['event']
    assert e['type'] == kind and 'target' not in e
    assert e['character'] == '00000000-0000-0000-0000-000000000063'
    if kind == 'reply.text':
        assert e['text'] == '  hello\n' and e['offset'] == 0
    if kind == 'speech.stopped':
        assert e['reason'] == 'observed_terminal'
    if kind == 'emotion.changed':
        assert e['owner'] == 'scene_chat'
        assert e['previous'] == 'neutral' and e['emotion'] == 'curious'
"#,
        );
        let connection = &mut fixture.connection;
        let reply_id = Uuid::from_u128(41);
        let playback_id = Uuid::from_u128(42);
        let character = Some(Uuid::from_u128(99));
        let metadata = HostEvent::ReplyFinished {
            reply_id,
            character,
            outcome: ReplyOutcome::Completed,
        };
        let text = HostEvent::ReplyText {
            reply_id,
            character,
            offset: 0,
            text: "  hello\n".into(),
        };
        assert!(!connection.session_id().is_nil());
        assert_eq!(connection.process_generation(), 7);
        assert!(connection.publish_lifecycle(metadata.clone()).is_err());
        connection.set_grants([Capability::LifecycleObserve]);
        assert!(connection.grants().contains(&Capability::LifecycleObserve));
        assert!(connection.publish_lifecycle(text.clone()).is_err());
        connection.publish_lifecycle(metadata.clone()).unwrap();
        connection.set_grants([Capability::ReplyRead]);
        assert!(connection.publish_lifecycle(metadata).is_err());
        connection.publish_lifecycle(text.clone()).unwrap();
        connection.set_grants([Capability::LifecycleObserve]);
        assert!(connection.publish_lifecycle(text).is_err());
        connection
            .publish_lifecycle(HostEvent::SpeechStarted {
                playback_id,
                character,
                voice_generation: 9,
                reply_id: Some(reply_id),
            })
            .unwrap();
        connection
            .publish_lifecycle(HostEvent::SpeechStopped {
                playback_id,
                character,
                voice_generation: 9,
                reply_id: Some(reply_id),
                reason: SpeechStopReason::ObservedTerminal,
            })
            .unwrap();
        connection.set_grants([Capability::ReplyRead]);
        let emotion = HostEvent::EmotionChanged {
            character: Uuid::from_u128(99),
            owner: EmotionOwner::SceneChat,
            previous: crate::reply_affect::AffectEmotion::Neutral,
            emotion: crate::reply_affect::AffectEmotion::Curious,
            reply_id: Some(reply_id),
        };
        assert!(connection.publish_lifecycle(emotion.clone()).is_err());
        assert!(
            connection
                .publish_lifecycle(HostEvent::MotionFinished {
                    target: target(),
                    motion_id: "org.example.pack/nod".into(),
                    action_id: "motion-1".into(),
                    outcome: MotionOutcome::Completed,
                })
                .is_err()
        );
        connection.set_grants([Capability::LifecycleObserve]);
        connection.publish_lifecycle(emotion).unwrap();
        assert_eq!(connection.host_sequence, 6);
        fixture.until(|connection, work| {
            assert!(work.is_empty());
            connection.termination().is_some()
        });
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
    }

    #[test]
    fn real_child_handshake_action_result_and_authoritative_exit() {
        let mut fixture = Fixture::new("emit()\nresult(2)");
        let mut executions = 0;
        fixture.until(|connection, work| {
            for work in work {
                assert!(connection.can_execute(&work, target()));
                let mut changed = target();
                changed.generation += 1;
                assert!(!connection.can_execute(&work, changed));
                connection.set_grants([]);
                assert!(!connection.can_execute(&work, target()));
                connection.set_grants([Capability::CatalogGet]);
                connection.complete(&work.ticket, accepted()).unwrap();
                assert!(!connection.can_execute(&work, target()));
                executions += 1;
            }
            connection.termination().is_some()
        });
        assert_eq!(executions, 1);
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
        assert!(!fixture.connection.is_active());
    }

    #[test]
    fn real_child_pending_and_cached_duplicates_never_reexecute() {
        let mut fixture = Fixture::new("emit()\nemit()\nresult(2)\nemit()\nresult(3)");
        let mut executions = 0;
        let mut held = None;
        fixture.until(|connection, work| {
            for work in work {
                executions += 1;
                assert!(held.is_none());
                held = Some(work);
            }
            if let Some(work) = held.as_ref() {
                // Wait until the queued duplicate has actually been consumed.
                // It cannot advance host sequence or produce another dispatch.
                if !connection
                    .logs()
                    .iter()
                    .any(|log| log == "Duplicate action still pending")
                {
                    return false;
                }
                assert_eq!(connection.host_sequence, 1);
                connection.complete(&work.ticket, accepted()).unwrap();
                held = None;
            }
            connection.termination().is_some()
        });
        assert_eq!(executions, 1);
        assert_eq!(fixture.connection.host_sequence, 3);
        assert_eq!(fixture.connection.termination().unwrap().code, Some(0));
    }

    #[test]
    fn real_child_malformed_output_is_stopped_and_reaped() {
        let mut fixture = Fixture::new("print('{broken', flush=True)\ntime.sleep(30)");
        fixture.until(|connection, work| {
            assert!(work.is_empty());
            connection.termination().is_some()
        });
        assert!(
            fixture
                .connection
                .status()
                .contains("Invalid extension output")
        );
        assert!(!fixture.connection.is_active());
        assert_eq!(fixture.connection.termination().unwrap().code, None);
    }

    #[test]
    fn diagnostic_storage_truncates_at_utf8_boundaries() {
        let text = "界".repeat(MAX_STATUS_BYTES);
        let result = bounded(&text);
        assert!(result.len() <= MAX_STATUS_BYTES);
        assert!(text.starts_with(&result));
    }
}
