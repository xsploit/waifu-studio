//! Pure API-v1 authorization/replay gate, not an executor or live extension API.
//! Integration checkpoint schema; not a stable public API.
//!
//! Construct one gate per host-assigned session/process incarnation. A restarted
//! process must receive a different process_generation or session_id; never reset
//! a live gate to clear its replay history. Supply the current avatar Target on
//! every admission. Speech changes are not target-incarnation changes.
//!
//! Admission compares session/process before consuming a sequence. Every new
//! positive sequence for that identity is consumed even when target, capability,
//! namespace or busy checks reject it. Sequence gaps are allowed; wrapping is not.
//! Request IDs are correlation labels, not replay keys. Cache eviction never
//! permits old sequences to execute again. Payload equality means equality of
//! the protocol's validated canonical encoding, not whitespace in a raw JSON line.
//!
//! At most one action is in flight. Complete its ticket with a validated result;
//! identical pending duplicates return Pending, never a second execution ticket.
//! Completed duplicates return the cached result without checking current grants
//! or reapplying effects. The host must publish rejected/cached replies itself.
//! Malformed programmatic requests consume new authenticated sequences but are
//! not cached; their retransmissions are rejected rather than executed.
//!
//! Authorization is a point-in-time check. The executor must look up registered
//! motions/expressions, supply message origin from the installation, and recheck
//! target/grants before deferred effects. This gate performs no such effects.

use crate::extension_protocol::{
    Action, ActionEnvelope, ActionResult, Capability, ErrorCode, MAX_ID_BYTES, Target,
    encode_action_line,
};
use std::{
    collections::{BTreeSet, VecDeque},
    fmt,
};
use uuid::Uuid;

pub const MAX_CACHED_RESPONSES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateError {
    pub code: ErrorCode,
    pub message: String,
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GateError {}

#[derive(Debug)]
pub enum Admission {
    /// The only outcome that authorizes one execution attempt.
    Execute(ExecutionTicket),
    /// Identical request already in flight; wait for its original completion.
    Pending,
    Cached(ActionResult),
    Rejected(ActionResult),
}

/// Opaque gate-issued receipt. It cannot be constructed by a protocol client.
#[derive(Debug)]
pub struct ExecutionTicket {
    session_id: Uuid,
    process_generation: u64,
    sequence: u64,
}

impl ExecutionTicket {
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

struct PendingRequest {
    sequence: u64,
    payload: Vec<u8>,
}

struct CachedResponse {
    sequence: u64,
    payload: Vec<u8>,
    result: ActionResult,
}

pub struct SessionGate {
    session_id: Uuid,
    process_generation: u64,
    event_prefix: String,
    grants: BTreeSet<Capability>,
    highest_sequence: u64,
    pending: Option<PendingRequest>,
    responses: VecDeque<CachedResponse>,
}

impl SessionGate {
    /// package_id and grants must come from host installation/permission state.
    pub fn new(
        session_id: Uuid,
        process_generation: u64,
        package_id: &str,
        grants: impl IntoIterator<Item = Capability>,
    ) -> Result<Self, GateError> {
        if session_id.is_nil() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "session_id must not be nil",
            ));
        }
        if package_id.len() > MAX_ID_BYTES
            || !package_id.contains('.')
            || !package_id.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.len() <= 64
                    && segment.as_bytes()[0].is_ascii_alphanumeric()
                    && segment.bytes().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'
                    })
            })
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "expected a dotted lowercase package_id",
            ));
        }
        Ok(Self {
            session_id,
            process_generation,
            event_prefix: format!("integration.{package_id}."),
            grants: grants.into_iter().collect(),
            highest_sequence: 0,
            pending: None,
            responses: VecDeque::new(),
        })
    }

    /// Updating permissions does not discard replay history or cached responses.
    /// The executor must recheck revocation before any deferred effect.
    pub fn set_grants(&mut self, grants: impl IntoIterator<Item = Capability>) {
        self.grants = grants.into_iter().collect();
    }

    pub const fn highest_sequence(&self) -> u64 {
        self.highest_sequence
    }

    pub(crate) fn quiesce(&mut self) {
        self.grants.clear();
        self.pending = None;
        self.responses.clear();
    }

    pub fn admit(&mut self, request: &ActionEnvelope, current_target: Target) -> Admission {
        self.admit_target(request, Some(current_target))
    }

    /// Absence of a selected avatar rejects new authenticated requests without
    /// changing pending/cached duplicate semantics or resetting replay history.
    pub fn admit_target(
        &mut self,
        request: &ActionEnvelope,
        current_target: Option<Target>,
    ) -> Admission {
        if request.session_id != self.session_id {
            return rejected(
                ErrorCode::StaleSession,
                "Request belongs to another session",
            );
        }
        if request.process_generation != self.process_generation {
            return rejected(
                ErrorCode::StaleProcess,
                "Request belongs to another process incarnation",
            );
        }
        if request.sequence == 0 {
            return rejected(
                ErrorCode::InvalidRequest,
                "sequence must be greater than zero",
            );
        }

        if request.sequence <= self.highest_sequence {
            let Ok(payload) = encode_action_line(request) else {
                return rejected(ErrorCode::InvalidRequest, "Invalid duplicate request");
            };
            if let Some(pending) = &self.pending
                && pending.sequence == request.sequence
            {
                return if pending.payload == payload {
                    Admission::Pending
                } else {
                    rejected(
                        ErrorCode::InvalidRequest,
                        "Sequence reused with a different payload",
                    )
                };
            }
            if let Some(cached) = self
                .responses
                .iter()
                .find(|r| r.sequence == request.sequence)
            {
                return if cached.payload == payload {
                    Admission::Cached(cached.result.clone())
                } else {
                    rejected(
                        ErrorCode::InvalidRequest,
                        "Sequence reused with a different payload",
                    )
                };
            }
            return rejected(
                ErrorCode::InvalidRequest,
                "Old or evicted sequence cannot execute again",
            );
        }

        // Commit the high-water mark before any semantic/authorization rejection.
        self.highest_sequence = request.sequence;
        let payload = match encode_action_line(request) {
            Ok(payload) => payload,
            Err(_) => return rejected(ErrorCode::InvalidRequest, "Invalid protocol request"),
        };
        let Some(current_target) = current_target else {
            return self.cache_rejection(
                request.sequence,
                payload,
                ErrorCode::StaleTarget,
                "No avatar is selected",
            );
        };
        if current_target.validate().is_err() {
            return self.cache_rejection(
                request.sequence,
                payload,
                ErrorCode::InternalError,
                "Host target identity is invalid",
            );
        }
        if request.target != current_target {
            return self.cache_rejection(
                request.sequence,
                payload,
                ErrorCode::StaleTarget,
                "Target UUID or generation has changed",
            );
        }
        if !self.grants.contains(&request.action.required_capability()) {
            return self.cache_rejection(
                request.sequence,
                payload,
                ErrorCode::PermissionDenied,
                "Action capability is not granted",
            );
        }
        if let Action::EventEmit { name, .. } = &request.action
            && !name
                .strip_prefix(&self.event_prefix)
                .is_some_and(|suffix| !suffix.is_empty())
        {
            return self.cache_rejection(
                request.sequence,
                payload,
                ErrorCode::PermissionDenied,
                "Event must use integration.<own package>.<nonempty suffix>",
            );
        }
        if self.pending.is_some() {
            return self.cache_rejection(
                request.sequence,
                payload,
                ErrorCode::Busy,
                "An earlier action is still in flight; retry with a new sequence",
            );
        }
        self.pending = Some(PendingRequest {
            sequence: request.sequence,
            payload,
        });
        Admission::Execute(ExecutionTicket {
            session_id: self.session_id,
            process_generation: self.process_generation,
            sequence: request.sequence,
        })
    }

    /// Store the final outcome exactly once. Invalid results leave the request
    /// pending so the host can correct its response using the same ticket.
    pub fn complete(
        &mut self,
        ticket: &ExecutionTicket,
        result: ActionResult,
    ) -> Result<(), GateError> {
        if ticket.session_id != self.session_id {
            return Err(error(
                ErrorCode::StaleSession,
                "Ticket belongs to another session",
            ));
        }
        if ticket.process_generation != self.process_generation {
            return Err(error(
                ErrorCode::StaleProcess,
                "Ticket belongs to another process incarnation",
            ));
        }
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| p.sequence == ticket.sequence)
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "No matching pending request",
            ));
        }
        result
            .validate()
            .map_err(|_| error(ErrorCode::InvalidRequest, "Invalid action result"))?;
        let pending = self.pending.take().expect("pending identity checked above");
        self.cache(CachedResponse {
            sequence: pending.sequence,
            payload: pending.payload,
            result,
        });
        Ok(())
    }

    fn cache_rejection(
        &mut self,
        sequence: u64,
        payload: Vec<u8>,
        code: ErrorCode,
        message: &str,
    ) -> Admission {
        let result = ActionResult::Error {
            code,
            message: message.into(),
        };
        self.cache(CachedResponse {
            sequence,
            payload,
            result: result.clone(),
        });
        Admission::Rejected(result)
    }

    fn cache(&mut self, response: CachedResponse) {
        if self.responses.len() == MAX_CACHED_RESPONSES {
            self.responses.pop_front();
        }
        self.responses.push_back(response);
    }
}

fn error(code: ErrorCode, message: &str) -> GateError {
    GateError {
        code,
        message: message.into(),
    }
}

fn rejected(code: ErrorCode, message: &str) -> Admission {
    Admission::Rejected(ActionResult::Error {
        code,
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_protocol::{API_VERSION, ActionEnvelopeType, PlaybackMode, ResultValue};
    use serde_json::json;

    fn target() -> Target {
        Target {
            uuid: Uuid::from_u128(2),
            generation: 19,
        }
    }

    fn gate(grants: impl IntoIterator<Item = Capability>) -> SessionGate {
        SessionGate::new(Uuid::from_u128(1), 7, "org.example.pack", grants).unwrap()
    }

    fn request(sequence: u64) -> ActionEnvelope {
        ActionEnvelope {
            api_version: API_VERSION,
            message_type: ActionEnvelopeType::Action,
            id: "reusable-correlation-id".into(),
            sequence,
            session_id: Uuid::from_u128(1),
            process_generation: 7,
            target: target(),
            action: Action::MotionPlay {
                motion_id: "org.example.pack/nod".into(),
                mode: PlaybackMode::OneShot,
            },
        }
    }

    fn accepted() -> ActionResult {
        ActionResult::Ok {
            value: ResultValue::Accepted {},
        }
    }

    fn execution_ticket(admission: Admission) -> ExecutionTicket {
        match admission {
            Admission::Execute(ticket) => ticket,
            other => panic!("Expected execution, got {other:?}"),
        }
    }

    fn denied(admission: Admission, expected: ErrorCode) {
        match admission {
            Admission::Rejected(ActionResult::Error { code, .. }) => assert_eq!(code, expected),
            other => panic!("Expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn missing_avatar_consumes_and_caches_new_requests_without_replay() {
        let mut gate = gate([Capability::MotionPlay]);
        denied(gate.admit_target(&request(1), None), ErrorCode::StaleTarget);
        assert_eq!(gate.highest_sequence(), 1);
        for current in [None, Some(target())] {
            assert!(matches!(
                gate.admit_target(&request(1), current),
                Admission::Cached(ActionResult::Error {
                    code: ErrorCode::StaleTarget,
                    ..
                })
            ));
        }
        let ticket = execution_ticket(gate.admit(&request(2), target()));
        assert!(matches!(
            gate.admit_target(&request(2), None),
            Admission::Pending
        ));
        gate.complete(&ticket, accepted()).unwrap();
        gate.set_grants([]);
        assert!(
            matches!(gate.admit_target(&request(2), None), Admission::Cached(result) if result == accepted())
        );
        denied(gate.admit_target(&request(3), None), ErrorCode::StaleTarget);
        assert_eq!(gate.highest_sequence(), 3);
    }

    #[test]
    fn missing_avatar_does_not_consume_foreign_or_zero_sequences() {
        let mut gate = gate([Capability::MotionPlay]);
        let mut foreign = request(10);
        foreign.session_id = Uuid::from_u128(99);
        denied(gate.admit_target(&foreign, None), ErrorCode::StaleSession);
        foreign = request(10);
        foreign.process_generation += 1;
        denied(gate.admit_target(&foreign, None), ErrorCode::StaleProcess);
        denied(
            gate.admit_target(&request(0), None),
            ErrorCode::InvalidRequest,
        );
        assert_eq!(gate.highest_sequence(), 0);
    }

    #[test]
    fn duplicate_pending_and_completed_never_execute_twice() {
        let mut gate = gate([Capability::MotionPlay]);
        let request = request(1);
        let ticket = execution_ticket(gate.admit(&request, target()));
        assert_eq!(ticket.sequence(), 1);
        assert!(matches!(gate.admit(&request, target()), Admission::Pending));
        gate.complete(&ticket, accepted()).unwrap();
        match gate.admit(&request, target()) {
            Admission::Cached(result) => assert_eq!(result, accepted()),
            other => panic!("Duplicate executed: {other:?}"),
        }
        assert!(gate.complete(&ticket, accepted()).is_err());
        assert_eq!(gate.highest_sequence(), 1);
    }

    #[test]
    fn same_sequence_different_payload_is_rejected_before_and_after_completion() {
        let mut gate = gate([Capability::MotionPlay]);
        let original = request(1);
        let ticket = execution_ticket(gate.admit(&original, target()));
        let mut different = original.clone();
        different.id = "different-id".into();
        denied(gate.admit(&different, target()), ErrorCode::InvalidRequest);
        gate.complete(&ticket, accepted()).unwrap();
        different = original.clone();
        different.action = Action::CatalogGet {};
        denied(gate.admit(&different, target()), ErrorCode::InvalidRequest);
        different = original.clone();
        different.target.generation += 1;
        denied(gate.admit(&different, target()), ErrorCode::InvalidRequest);
    }

    #[test]
    fn evicted_or_skipped_sequences_never_execute_again_and_ids_are_not_replay_keys() {
        let mut gate = gate([Capability::MotionPlay]);
        for sequence in 1..=(MAX_CACHED_RESPONSES as u64 + 10) {
            let ticket = execution_ticket(gate.admit(&request(sequence), target()));
            gate.complete(&ticket, accepted()).unwrap();
        }
        assert_eq!(gate.responses.len(), MAX_CACHED_RESPONSES);
        denied(gate.admit(&request(1), target()), ErrorCode::InvalidRequest);
        let ticket = execution_ticket(gate.admit(&request(1000), target()));
        gate.complete(&ticket, accepted()).unwrap();
        denied(
            gate.admit(&request(999), target()),
            ErrorCode::InvalidRequest,
        );
        assert_eq!(gate.highest_sequence(), 1000);
    }

    #[test]
    fn target_uuid_and_incarnation_are_checked_but_speech_does_not_change_identity() {
        let mut gate = gate([Capability::MotionPlay]);
        let mut replaced = target();
        replaced.generation += 1;
        denied(gate.admit(&request(1), replaced), ErrorCode::StaleTarget);
        assert_eq!(gate.highest_sequence(), 1);
        assert!(matches!(
            gate.admit(&request(1), target()),
            Admission::Cached(ActionResult::Error {
                code: ErrorCode::StaleTarget,
                ..
            })
        ));
        replaced = target();
        replaced.uuid = Uuid::from_u128(3);
        denied(gate.admit(&request(2), replaced), ErrorCode::StaleTarget);
        // A new utterance leaves this Target unchanged; no speech generation is
        // substituted for the avatar generation supplied by the host.
        let ticket = execution_ticket(gate.admit(&request(3), target()));
        gate.complete(&ticket, accepted()).unwrap();
        let ticket = execution_ticket(gate.admit(&request(4), target()));
        gate.complete(&ticket, accepted()).unwrap();
    }

    #[test]
    fn foreign_sessions_and_process_restarts_cannot_replay_or_poison_sequence() {
        let mut old = gate([Capability::MotionPlay]);
        let old_ticket = execution_ticket(old.admit(&request(1), target()));
        let mut restarted = SessionGate::new(
            Uuid::from_u128(1),
            8,
            "org.example.pack",
            [Capability::MotionPlay],
        )
        .unwrap();
        denied(
            restarted.admit(&request(999), target()),
            ErrorCode::StaleProcess,
        );
        assert_eq!(restarted.highest_sequence(), 0);
        assert_eq!(
            restarted
                .complete(&old_ticket, accepted())
                .unwrap_err()
                .code,
            ErrorCode::StaleProcess
        );
        let mut fresh = request(1);
        fresh.process_generation = 8;
        let ticket = execution_ticket(restarted.admit(&fresh, target()));
        restarted.complete(&ticket, accepted()).unwrap();
        let mut another_session = SessionGate::new(
            Uuid::from_u128(4),
            7,
            "org.example.pack",
            [Capability::MotionPlay],
        )
        .unwrap();
        denied(
            another_session.admit(&request(999), target()),
            ErrorCode::StaleSession,
        );
        assert_eq!(another_session.highest_sequence(), 0);
        assert_eq!(
            another_session
                .complete(&old_ticket, accepted())
                .unwrap_err()
                .code,
            ErrorCode::StaleSession
        );
    }

    #[test]
    fn denied_capabilities_consume_sequence_and_grant_changes_keep_history() {
        let mut gate = gate([]);
        denied(
            gate.admit(&request(1), target()),
            ErrorCode::PermissionDenied,
        );
        gate.set_grants([Capability::MotionPlay]);
        assert!(matches!(
            gate.admit(&request(1), target()),
            Admission::Cached(ActionResult::Error {
                code: ErrorCode::PermissionDenied,
                ..
            })
        ));
        let ticket = execution_ticket(gate.admit(&request(2), target()));
        gate.complete(&ticket, accepted()).unwrap();
        gate.set_grants([]);
        assert!(matches!(
            gate.admit(&request(2), target()),
            Admission::Cached(_)
        ));
        denied(
            gate.admit(&request(3), target()),
            ErrorCode::PermissionDenied,
        );
    }

    #[test]
    fn every_action_requires_its_own_grant() {
        for action in [
            Action::MotionPlay {
                motion_id: "org.example.pack/nod".into(),
                mode: PlaybackMode::OneShot,
            },
            Action::ExpressionApply {
                name: "happy".into(),
                strength: 0.5,
                duration_ms: 100,
            },
            Action::MessageDeliver {
                text: "Hello".into(),
            },
            Action::EventEmit {
                name: "integration.org.example.pack.notice".into(),
                data: json!({}),
            },
            Action::CatalogGet {},
        ] {
            let mut request = request(1);
            request.action = action;
            denied(
                gate([]).admit(&request, target()),
                ErrorCode::PermissionDenied,
            );
            let mut allowed = gate([request.action.required_capability()]);
            let ticket = execution_ticket(allowed.admit(&request, target()));
            allowed.complete(&ticket, accepted()).unwrap();
        }
    }

    #[test]
    fn events_require_exact_own_namespace_and_nonempty_suffix() {
        let mut gate = gate([Capability::EventEmit]);
        let names = [
            "host.notice",
            "integration.org.other.pack.notice",
            "integration.org.example.packish.notice",
            "integration.org.example.pack",
            "integration.org.example.pack.",
        ];
        for (index, name) in names.into_iter().enumerate() {
            let mut request = request(index as u64 + 1);
            request.action = Action::EventEmit {
                name: name.into(),
                data: json!(null),
            };
            assert!(matches!(
                gate.admit(&request, target()),
                Admission::Rejected(_)
            ));
        }
        let mut valid = request(6);
        valid.action = Action::EventEmit {
            name: "integration.org.example.pack.notice.ready".into(),
            data: json!({"value": 1}),
        };
        let ticket = execution_ticket(gate.admit(&valid, target()));
        gate.complete(&ticket, accepted()).unwrap();
    }

    #[test]
    fn busy_request_is_consumed_pending_is_not_evicted_and_duplicate_does_not_execute() {
        let mut gate = gate([Capability::MotionPlay]);
        let ticket = execution_ticket(gate.admit(&request(1), target()));
        for sequence in 2..=(MAX_CACHED_RESPONSES as u64 + 5) {
            denied(gate.admit(&request(sequence), target()), ErrorCode::Busy);
        }
        assert_eq!(gate.responses.len(), MAX_CACHED_RESPONSES);
        assert!(matches!(
            gate.admit(&request(1), target()),
            Admission::Pending
        ));
        gate.complete(&ticket, accepted()).unwrap();
        assert!(matches!(
            gate.admit(&request(1), target()),
            Admission::Cached(_)
        ));
        assert!(matches!(
            gate.admit(&request(MAX_CACHED_RESPONSES as u64 + 5), target()),
            Admission::Cached(ActionResult::Error {
                code: ErrorCode::Busy,
                ..
            })
        ));
    }

    #[test]
    fn malformed_requests_are_consumed_and_invalid_results_do_not_publish() {
        let mut gate = gate([Capability::MotionPlay]);
        let mut malformed = request(1);
        malformed.id.clear();
        denied(gate.admit(&malformed, target()), ErrorCode::InvalidRequest);
        assert_eq!(gate.highest_sequence(), 1);
        denied(gate.admit(&request(1), target()), ErrorCode::InvalidRequest);
        let ticket = execution_ticket(gate.admit(&request(2), target()));
        assert!(
            gate.complete(
                &ticket,
                ActionResult::Error {
                    code: ErrorCode::InternalError,
                    message: "".into()
                }
            )
            .is_err()
        );
        assert!(matches!(
            gate.admit(&request(2), target()),
            Admission::Pending
        ));
        gate.complete(&ticket, accepted()).unwrap();
        malformed = request(3);
        malformed.sequence = 0;
        denied(gate.admit(&malformed, target()), ErrorCode::InvalidRequest);
        assert_eq!(gate.highest_sequence(), 2);
    }

    #[test]
    fn maximum_sequence_does_not_wrap_and_configuration_is_bounded() {
        let mut gate = gate([Capability::MotionPlay]);
        let ticket = execution_ticket(gate.admit(&request(u64::MAX), target()));
        gate.complete(&ticket, accepted()).unwrap();
        denied(gate.admit(&request(1), target()), ErrorCode::InvalidRequest);
        assert_eq!(gate.highest_sequence(), u64::MAX);
        for package in [
            "",
            "single",
            "Org.example",
            "org..example",
            "org/example",
            "org.-example",
        ] {
            assert!(SessionGate::new(Uuid::from_u128(1), 7, package, []).is_err());
        }
        assert!(SessionGate::new(Uuid::nil(), 7, "org.example", []).is_err());
    }
}
