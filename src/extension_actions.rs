//! Host action execution against validated registrations and shared controllers.
//! Admission/replay belongs to SessionGate; callers execute only its tickets.
//! This service rechecks current target and permissions immediately before effects.
use crate::{
    animation::{AnimationState, PlaybackOutcome, PlaybackReceipt, PlaybackToken},
    behavior::{BehaviorController, IntegrationExpressionToken, reserved_expression},
    extension_manifest::{ContentKind, ResolvedPack},
    extension_protocol::{
        Action, ActionEnvelope, ActionResult, Capability, ChatMessage, ErrorCode, HostEvent,
        IntegrationOrigin, MAX_CATALOG_ENTRIES, MAX_ID_BYTES, MotionOutcome, RegisteredExpression,
        RegisteredMotion, ResultValue, Target,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use uuid::Uuid;

const MAX_OWNERS: usize = 32;

#[derive(Default)]
pub struct ActionCatalog {
    motions: BTreeMap<String, (String, PathBuf)>,
    expressions: BTreeSet<String>,
}
impl ActionCatalog {
    /// Only host-resolved enabled packs enter this view. Client IDs never become paths.
    pub fn from_packs<'a>(
        packs: impl IntoIterator<Item = &'a ResolvedPack>,
        expressions: &[String],
    ) -> Self {
        let mut catalog = Self::default();
        for pack in packs {
            for motion in &pack.manifest.content.motions {
                if let Some(content) = pack.content.iter().find(|content| {
                    content.kind == ContentKind::Motion && content.local_id == motion.id
                }) && let Some(path) = &content.path
                {
                    let label = if motion.name.len() <= MAX_ID_BYTES {
                        &motion.name
                    } else {
                        &motion.id
                    };
                    catalog
                        .motions
                        .insert(content.id.clone(), (label.clone(), path.clone()));
                }
            }
        }
        catalog.expressions.extend(
            expressions
                .iter()
                .filter(|name| {
                    !reserved_expression(name)
                        && !name.trim().is_empty()
                        && name.len() <= MAX_ID_BYTES
                        && !name.contains('\0')
                })
                .cloned(),
        );
        catalog
    }
    fn result(&self) -> ActionResult {
        let mut motions: Vec<_> = self
            .motions
            .iter()
            .take(MAX_CATALOG_ENTRIES)
            .map(|(id, (name, _))| RegisteredMotion {
                motion_id: id.clone(),
                name: name.clone(),
            })
            .collect();
        let mut expressions: Vec<_> = self
            .expressions
            .iter()
            .take(MAX_CATALOG_ENTRIES - motions.len())
            .map(|name| RegisteredExpression { name: name.clone() })
            .collect();
        // Entry count alone does not bound JSON escaping. Reserve ample room
        // for the host envelope and correlated request ID within the 64 KiB line.
        while serde_json::to_vec(&(&motions, &expressions)).unwrap().len() > 48 * 1024 {
            if expressions.pop().is_none() {
                motions.pop();
            }
        }
        ActionResult::Ok {
            value: ResultValue::Catalog {
                motions,
                expressions,
            },
        }
    }
}

/// Identity is assigned by the host, never supplied by a child as attribution.
pub struct ActionContext<'a> {
    pub session_id: Uuid,
    pub process_generation: u64,
    pub target: Option<Target>,
    pub grants: &'a BTreeSet<Capability>,
    pub origin: &'a IntegrationOrigin,
    pub desktop_owned: bool,
}

struct OwnedActions {
    generation: u64,
    target: Target,
    motion: Option<PlaybackToken>,
    expression: Option<IntegrationExpressionToken>,
}

struct TrackedMotion {
    session: Uuid,
    generation: u64,
    target: Target,
    action_id: String,
    motion_id: String,
    receipt: PlaybackReceipt,
}

#[derive(Default)]
pub struct HostActions {
    owners: BTreeMap<Uuid, OwnedActions>,
    // Independent of current control ownership: a new action cannot overwrite
    // the terminal receipt of its predecessor. Admission bounds this at 32.
    motions: Vec<TrackedMotion>,
}

pub struct ActionOutcome {
    pub result: ActionResult,
    /// Only published by the host after acceptance; not a child-authored role.
    pub event: Option<HostEvent>,
}

pub struct ActionControllers<'a> {
    pub animation: &'a mut AnimationState,
    pub behavior: &'a mut BehaviorController,
    pub external_messages: Option<&'a mut dyn ExternalMessageSink>,
}

/// Host adapter for attributed external intake. Implementations must be bounded
/// and return delivery success only after durable sink acceptance for `target`.
/// The host supplies attribution; a child never chooses a conversation role.
pub trait ExternalMessageSink {
    /// Connection/player readiness, correlated to an eligible host-owned route.
    fn audio_playback_available(
        &mut self,
        _target: Target,
        _origin: &IntegrationOrigin,
        _session: Uuid,
        _process_generation: u64,
    ) -> Result<(), (ErrorCode, String)> {
        Err((
            ErrorCode::Unavailable,
            "Speech playback readiness is not enabled".into(),
        ))
    }

    /// Opt-in receipt correlation. An implementation must match its host-owned
    /// playback ID, route/incarnation, phase and sent-frame ledger before accepting.
    /// This callback alone must never authorize another media send.
    fn audio_playback_receipt(
        &mut self,
        _target: Target,
        _origin: &IntegrationOrigin,
        _session: Uuid,
        _process_generation: u64,
        _receipt: &crate::extension_protocol::AudioPlaybackReceipt,
    ) -> Result<(), (ErrorCode, String)> {
        Err((
            ErrorCode::Unavailable,
            "Speech playback feedback is not enabled".into(),
        ))
    }

    /// Audio admission is opt-in. Acceptance acknowledges this record only,
    /// never transcription, conversation persistence or provider completion.
    fn audio_input(
        &mut self,
        _target: Target,
        _origin: &IntegrationOrigin,
        _session: Uuid,
        _process_generation: u64,
        _input: &crate::extension_protocol::AudioInput,
    ) -> Result<(), (ErrorCode, String)> {
        Err((
            ErrorCode::Unavailable,
            "Participant audio intake is not enabled".into(),
        ))
    }
    /// Queue a candidate for later selection. This must never default to delivery.
    fn offer_chat(
        &mut self,
        _target: Target,
        _origin: &IntegrationOrigin,
        _message: &ChatMessage,
    ) -> Result<(), (ErrorCode, String)> {
        Err((ErrorCode::Unavailable, "Chat offers are not enabled".into()))
    }

    fn deliver(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        text: &str,
    ) -> Result<Uuid, (ErrorCode, String)>;
}

fn failure(code: ErrorCode, message: &str) -> ActionOutcome {
    ActionOutcome {
        result: ActionResult::Error {
            code,
            message: message.into(),
        },
        event: None,
    }
}
// Keep adapter failures bounded and valid on the wire for both intake paths.
fn external_message_failure(code: ErrorCode, message: &str) -> ActionOutcome {
    let message: String = message
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .scan(0usize, |bytes, ch| {
            *bytes += ch.len_utf8();
            (*bytes <= crate::extension_protocol::MAX_TEXT_BYTES).then_some(ch)
        })
        .collect();
    failure(
        code,
        if message.trim().is_empty() {
            "External message delivery failed"
        } else {
            &message
        },
    )
}

fn accepted() -> ActionOutcome {
    ActionOutcome {
        result: ActionResult::Ok {
            value: ResultValue::Accepted {},
        },
        event: None,
    }
}

impl HostActions {
    /// Frame-thread bridge from a supervised child to the same host controllers.
    /// The caller supplies its current registered catalog and avatar incarnation.
    /// No filesystem discovery, blocking process I/O, or provider call occurs
    /// here. An optional host message sink may persist synchronously before ack.
    pub fn poll_connection(
        &mut self,
        connection: &mut crate::extension_connection::ExtensionConnection,
        target: impl Into<Option<Target>>,
        desktop_owned: bool,
        catalog: &ActionCatalog,
        mut controllers: ActionControllers<'_>,
    ) {
        let target = target.into();
        let context = ActionContext {
            session_id: connection.session_id(),
            process_generation: connection.process_generation(),
            target,
            grants: connection.grants(),
            origin: connection.origin(),
            desktop_owned,
        };
        self.reconcile(&context, controllers.animation, controllers.behavior);
        let session = context.session_id;
        self.publish_motion_terminals(connection);
        self.process_drained(connection, target, desktop_owned, catalog, &mut controllers);
        self.publish_motion_terminals(connection);
        if !connection.is_active() {
            self.cancel(session, controllers.animation, controllers.behavior);
        }
    }

    fn process_drained(
        &mut self,
        connection: &mut crate::extension_connection::ExtensionConnection,
        target: Option<Target>,
        desktop_owned: bool,
        catalog: &ActionCatalog,
        controllers: &mut ActionControllers<'_>,
    ) {
        for work in connection.drain_target(target) {
            let outcome = if target.is_some_and(|target| connection.can_execute(&work, target)) {
                let context = ActionContext {
                    session_id: connection.session_id(),
                    process_generation: connection.process_generation(),
                    target,
                    grants: connection.grants(),
                    origin: connection.origin(),
                    desktop_owned,
                };
                self.execute_with_messages(
                    &work.action,
                    &context,
                    catalog,
                    ActionControllers {
                        animation: controllers.animation,
                        behavior: controllers.behavior,
                        external_messages: match controllers.external_messages {
                            Some(ref mut sink) => Some(&mut **sink),
                            None => None,
                        },
                    },
                )
            } else {
                failure(ErrorCode::Unavailable, "Execution was invalidated")
            };
            if let Some(event) = outcome.event
                && connection.publish_event(event).is_err()
            {
                break;
            }
            if connection.complete(&work.ticket, outcome.result).is_err() {
                break;
            }
        }
    }

    fn publish_motion_terminals(
        &mut self,
        connection: &mut crate::extension_connection::ExtensionConnection,
    ) {
        let session = connection.session_id();
        let generation = connection.process_generation();
        self.motions.retain(|motion| {
            if motion.session != session {
                return true;
            }
            if !connection.is_active() || motion.generation != generation {
                return false;
            }
            let outcome = match motion.receipt.outcome() {
                PlaybackOutcome::Pending => return true,
                PlaybackOutcome::Completed => MotionOutcome::Completed,
                PlaybackOutcome::Cancelled => MotionOutcome::Cancelled,
                PlaybackOutcome::Failed => MotionOutcome::Failed,
            };
            if connection.grants().contains(&Capability::LifecycleObserve) {
                // Sending is fail-closed; a failed send stops the connection.
                // Remove the receipt either way rather than retrying an event
                // that might already have reached the transport.
                let _ = connection.publish_lifecycle(HostEvent::MotionFinished {
                    target: motion.target,
                    action_id: motion.action_id.clone(),
                    motion_id: motion.motion_id.clone(),
                    outcome,
                });
            }
            false
        });
    }

    /// Disconnect/disable cleanup: cancel controls and discard terminal receipts.
    /// Target loss on a live connection should use poll_connection(None) instead.
    pub fn cancel(
        &mut self,
        session: Uuid,
        animation: &mut AnimationState,
        behavior: &mut BehaviorController,
    ) {
        self.cancel_controls(session, animation, behavior);
        self.motions.retain(|motion| motion.session != session);
    }

    /// Requires real controllers: forgetting tokens is not effect cancellation.
    pub(crate) fn cancel_all(
        &mut self,
        animation: &mut AnimationState,
        behavior: &mut BehaviorController,
    ) {
        let sessions: Vec<_> = self.owners.keys().copied().collect();
        for session in sessions {
            self.cancel(session, animation, behavior);
        }
        self.motions.clear();
    }

    fn cancel_controls(
        &mut self,
        session: Uuid,
        animation: &mut AnimationState,
        behavior: &mut BehaviorController,
    ) {
        if let Some(owner) = self.owners.remove(&session) {
            if let Some(token) = owner.motion {
                animation.cancel_owned_playback(token);
            }
            if let Some(token) = owner.expression {
                behavior.cancel_integration_expression(token);
            }
        }
    }

    /// Invoke on target, process, or permission changes, even without new requests.
    pub fn reconcile(
        &mut self,
        context: &ActionContext<'_>,
        animation: &mut AnimationState,
        behavior: &mut BehaviorController,
    ) {
        let invalid = self.owners.get(&context.session_id).is_some_and(|owner| {
            owner.generation != context.process_generation || Some(owner.target) != context.target
        });
        if invalid {
            self.cancel_controls(context.session_id, animation, behavior);
            return;
        }
        if let Some(owner) = self.owners.get_mut(&context.session_id) {
            if (context.desktop_owned || !context.grants.contains(&Capability::MotionPlay))
                && let Some(token) = owner.motion.take()
            {
                animation.cancel_owned_playback(token);
            }
            if !context.grants.contains(&Capability::ExpressionApply)
                && let Some(token) = owner.expression.take()
            {
                behavior.cancel_integration_expression(token);
            }
        }
        self.owners.retain(|_, owner| {
            if owner
                .motion
                .is_some_and(|token| !animation.owned_playback_active(token))
            {
                owner.motion = None;
            }
            if owner
                .expression
                .is_some_and(|token| !behavior.integration_expression_active(token))
            {
                owner.expression = None;
            }
            owner.motion.is_some() || owner.expression.is_some()
        });
    }

    pub fn execute(
        &mut self,
        request: &ActionEnvelope,
        context: &ActionContext<'_>,
        catalog: &ActionCatalog,
        animation: &mut AnimationState,
        behavior: &mut BehaviorController,
    ) -> ActionOutcome {
        self.execute_with_messages(
            request,
            context,
            catalog,
            ActionControllers {
                animation,
                behavior,
                external_messages: None,
            },
        )
    }

    /// Same current-identity checks as execute, with an optional host message
    /// adapter. Replay admission remains the caller's responsibility; production
    /// invokes this only for a fresh connection execution ticket.
    pub fn execute_with_messages(
        &mut self,
        request: &ActionEnvelope,
        context: &ActionContext<'_>,
        catalog: &ActionCatalog,
        controllers: ActionControllers<'_>,
    ) -> ActionOutcome {
        let ActionControllers {
            animation,
            behavior,
            external_messages,
        } = controllers;
        self.reconcile(context, animation, behavior);
        if request.validate().is_err() {
            return failure(ErrorCode::InvalidRequest, "Invalid action");
        }
        if request.session_id != context.session_id {
            return failure(ErrorCode::StaleSession, "Session changed");
        }
        if request.process_generation != context.process_generation {
            return failure(ErrorCode::StaleProcess, "Process changed");
        }
        if Some(request.target) != context.target {
            return failure(ErrorCode::StaleTarget, "Avatar changed");
        }
        if !context
            .grants
            .contains(&request.action.required_capability())
        {
            return failure(ErrorCode::PermissionDenied, "Capability is not granted");
        }
        match &request.action {
            Action::MotionPlay { motion_id, .. } if !catalog.motions.contains_key(motion_id) => {
                return failure(ErrorCode::UnknownMotion, "Motion is not registered");
            }
            Action::ExpressionApply { name, .. } if !catalog.expressions.contains(name) => {
                return failure(
                    ErrorCode::UnknownExpression,
                    "Expression is unavailable or reserved",
                );
            }
            _ => {}
        }
        if matches!(
            request.action,
            Action::MotionPlay { .. } | Action::ExpressionApply { .. }
        ) && !self.owners.contains_key(&context.session_id)
            && self.owners.len() >= MAX_OWNERS
        {
            return failure(ErrorCode::Busy, "Avatar control owner limit reached");
        }
        match &request.action {
            Action::CatalogGet {} => ActionOutcome {
                result: catalog.result(),
                event: None,
            },
            Action::AudioPlaybackAvailable {} => {
                let Some(sink) = external_messages else {
                    return failure(
                        ErrorCode::Unavailable,
                        "Speech playback readiness is not enabled",
                    );
                };
                match sink.audio_playback_available(
                    request.target,
                    context.origin,
                    context.session_id,
                    context.process_generation,
                ) {
                    Ok(()) => accepted(),
                    Err((code, message)) => external_message_failure(code, &message),
                }
            }
            Action::AudioPlaybackReceipt { receipt } => {
                let Some(sink) = external_messages else {
                    return failure(
                        ErrorCode::Unavailable,
                        "Speech playback feedback is not enabled",
                    );
                };
                match sink.audio_playback_receipt(
                    request.target,
                    context.origin,
                    context.session_id,
                    context.process_generation,
                    receipt,
                ) {
                    Ok(()) => accepted(),
                    Err((code, message)) => external_message_failure(code, &message),
                }
            }
            Action::AudioInput { input } => {
                if !context.grants.contains(&Capability::MessageDeliver) {
                    return failure(
                        ErrorCode::PermissionDenied,
                        "External-message delivery must also be granted",
                    );
                }
                let Some(sink) = external_messages else {
                    return failure(
                        ErrorCode::Unavailable,
                        "Participant audio intake is not enabled",
                    );
                };
                match sink.audio_input(
                    request.target,
                    context.origin,
                    context.session_id,
                    context.process_generation,
                    input,
                ) {
                    Ok(()) => accepted(),
                    Err((code, message)) => external_message_failure(code, &message),
                }
            }
            Action::MotionPlay { motion_id, .. } => {
                if self.motions.len() >= MAX_OWNERS {
                    return failure(ErrorCode::Busy, "Motion receipt limit reached");
                }
                if context.desktop_owned
                    || animation.manual_controlled()
                    || animation.loading
                    || animation.gesture_active()
                {
                    return failure(ErrorCode::Busy, "Body is controlled by another action");
                }
                let Some((_, path)) = catalog.motions.get(motion_id) else {
                    return failure(ErrorCode::UnknownMotion, "Motion is not registered");
                };
                let source = crate::animation::MotionSourceSnapshot {
                    source_target: request.target.into(),
                    motion_id: Some(motion_id.clone()),
                    source: crate::extension_protocol::MotionPlaybackSource::Extension {
                        integration_id: context.origin.integration_id.clone(),
                        session_id: context.session_id,
                        process_generation: context.process_generation,
                        action_id: request.id.clone(),
                    },
                };
                match animation.try_load_owned_one_shot_attributed(path, source) {
                    Ok(token) => {
                        let Some(receipt) = animation.owned_playback_receipt(token) else {
                            animation.cancel_owned_playback(token);
                            return failure(
                                ErrorCode::Unavailable,
                                "Motion receipt is unavailable",
                            );
                        };
                        self.owner(context, request.target).motion = Some(token);
                        self.motions.push(TrackedMotion {
                            session: context.session_id,
                            generation: context.process_generation,
                            target: request.target,
                            action_id: request.id.clone(),
                            motion_id: motion_id.clone(),
                            receipt,
                        });
                        accepted()
                    }
                    Err(crate::animation::MotionLoadError::Busy) => {
                        failure(ErrorCode::Busy, "Animation import is already running")
                    }
                    Err(_) => failure(ErrorCode::Unavailable, "Motion could not be loaded"),
                }
            }
            Action::ExpressionApply {
                name,
                strength,
                duration_ms,
            } => {
                if !catalog.expressions.contains(name) {
                    return failure(
                        ErrorCode::UnknownExpression,
                        "Expression is unavailable or reserved",
                    );
                }
                // Keep one expression per process: reject budget exhaustion without
                // deleting its existing contribution, then replace only on success.
                match behavior.add_integration_expression(
                    request.target.uuid,
                    name,
                    *strength,
                    *duration_ms as f32 / 1000.,
                ) {
                    Ok(token) => {
                        if let Some(old) = self
                            .owner(context, request.target)
                            .expression
                            .replace(token)
                        {
                            behavior.cancel_integration_expression(old);
                        }
                        accepted()
                    }
                    Err(_) => failure(ErrorCode::Unavailable, "Expression control is unavailable"),
                }
            }
            Action::EventEmit { name, data } => {
                let prefix = format!("integration.{}.", context.origin.integration_id);
                if !name
                    .strip_prefix(&prefix)
                    .is_some_and(|suffix| !suffix.is_empty())
                {
                    return failure(
                        ErrorCode::PermissionDenied,
                        "Event namespace belongs to another integration",
                    );
                }
                ActionOutcome {
                    result: accepted().result,
                    event: Some(HostEvent::Emitted {
                        target: request.target,
                        name: name.clone(),
                        data: data.clone(),
                        origin: context.origin.clone(),
                    }),
                }
            }
            Action::ChatOffer { message } => {
                let Some(sink) = external_messages else {
                    return failure(ErrorCode::Unavailable, "Chat offers are not enabled");
                };
                match sink.offer_chat(request.target, context.origin, message) {
                    Ok(()) => accepted(),
                    Err((code, message)) => external_message_failure(code, &message),
                }
            }
            Action::MessageDeliver { text } => {
                let Some(sink) = external_messages else {
                    return failure(
                        ErrorCode::Unavailable,
                        "Attributed external-message delivery is not enabled",
                    );
                };
                match sink.deliver(request.target, context.origin, text) {
                    // The returned host message identity belongs to intake; the
                    // current protocol receipt carries target/text/origin only.
                    Ok(_message_id) => ActionOutcome {
                        result: accepted().result,
                        event: Some(HostEvent::MessageDelivered {
                            target: request.target,
                            text: text.clone(),
                            origin: context.origin.clone(),
                        }),
                    },
                    Err((code, message)) => external_message_failure(code, &message),
                }
            }
        }
    }
    fn owner(&mut self, context: &ActionContext<'_>, target: Target) -> &mut OwnedActions {
        self.owners
            .entry(context.session_id)
            .or_insert(OwnedActions {
                generation: context.process_generation,
                target,
                motion: None,
                expression: None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        behavior::BehaviorConfig,
        extension_protocol::{API_VERSION, ActionEnvelopeType},
    };

    #[derive(Default)]
    struct MessageSink {
        calls: Vec<(Target, IntegrationOrigin, String)>,
        error: Option<(ErrorCode, String)>,
    }
    impl ExternalMessageSink for MessageSink {
        fn deliver(
            &mut self,
            target: Target,
            origin: &IntegrationOrigin,
            text: &str,
        ) -> Result<Uuid, (ErrorCode, String)> {
            self.calls.push((target, origin.clone(), text.into()));
            match &self.error {
                Some(error) => Err(error.clone()),
                None => Ok(Uuid::new_v4()),
            }
        }
    }

    #[derive(Default)]
    struct OfferQueue {
        queued: Vec<(Target, IntegrationOrigin, ChatMessage)>,
        error: Option<String>,
    }
    impl ExternalMessageSink for OfferQueue {
        fn deliver(
            &mut self,
            _: Target,
            _: &IntegrationOrigin,
            _: &str,
        ) -> Result<Uuid, (ErrorCode, String)> {
            panic!("offer must never deliver");
        }
        fn offer_chat(
            &mut self,
            target: Target,
            origin: &IntegrationOrigin,
            message: &ChatMessage,
        ) -> Result<(), (ErrorCode, String)> {
            if let Some(error) = &self.error {
                return Err((ErrorCode::Busy, error.clone()));
            }
            self.queued.push((target, origin.clone(), message.clone()));
            Ok(())
        }
    }

    #[test]
    fn chat_offers_require_explicit_queue_and_grant_and_never_emit_delivery() {
        for case in [
            "absent",
            "default",
            "queue",
            "grant",
            "invalid",
            "error",
            "empty-error",
        ] {
            let mut f = Fixture::new();
            let mut message = ChatMessage {
                source_id: "twitch-chat-id:opaque".into(),
                author_id: "viewer-id".into(),
                author_name: "Viewer".into(),
                text: "Hello 🦀".into(),
            };
            if case == "invalid" {
                message.text.push('\0');
            }
            if case == "grant" {
                f.grants.remove(&Capability::MessageDeliver);
            }
            let request = ActionEnvelope {
                api_version: API_VERSION,
                message_type: ActionEnvelopeType::Action,
                id: "offer".into(),
                sequence: 1,
                session_id: f.session,
                process_generation: 1,
                target: f.target,
                action: Action::ChatOffer {
                    message: message.clone(),
                },
            };
            let context = ActionContext {
                session_id: f.session,
                process_generation: 1,
                target: Some(f.target),
                grants: &f.grants,
                origin: &f.origin,
                desktop_owned: false,
            };
            let mut default_sink = MessageSink::default();
            let mut queue = OfferQueue::default();
            if case == "error" {
                queue.error = Some("🦀\n".repeat(2048));
            }
            if case == "empty-error" {
                queue.error = Some("\n\0".into());
            }
            let sink: Option<&mut dyn ExternalMessageSink> = match case {
                "absent" => None,
                "default" => Some(&mut default_sink),
                _ => Some(&mut queue),
            };
            let outcome = f.host.execute_with_messages(
                &request,
                &context,
                &f.catalog,
                ActionControllers {
                    animation: &mut f.animation,
                    behavior: &mut f.behavior,
                    external_messages: sink,
                },
            );
            assert!(outcome.event.is_none(), "{case}");
            outcome.result.validate().unwrap();
            assert!(default_sink.calls.is_empty());
            if case == "queue" {
                assert!(matches!(
                    outcome.result,
                    ActionResult::Ok {
                        value: ResultValue::Accepted {}
                    }
                ));
                assert_eq!(queue.queued, vec![(f.target, f.origin.clone(), message)]);
            } else {
                let expected = match case {
                    "grant" => ErrorCode::PermissionDenied,
                    "invalid" => ErrorCode::InvalidRequest,
                    "error" | "empty-error" => ErrorCode::Busy,
                    _ => ErrorCode::Unavailable,
                };
                assert!(
                    matches!(outcome.result, ActionResult::Error { code, .. } if code == expected),
                    "{case}"
                );
                assert!(queue.queued.is_empty());
            }
        }
    }

    #[test]
    fn message_sink_rechecks_identity_target_permission_and_validity_before_intake() {
        for (case, code) in [
            ("session", ErrorCode::StaleSession),
            ("process", ErrorCode::StaleProcess),
            ("target", ErrorCode::StaleTarget),
            ("no-target", ErrorCode::StaleTarget),
            ("grant", ErrorCode::PermissionDenied),
            ("invalid", ErrorCode::InvalidRequest),
        ] {
            let mut f = Fixture::new();
            let mut sink = MessageSink::default();
            let mut request = ActionEnvelope {
                api_version: API_VERSION,
                message_type: ActionEnvelopeType::Action,
                id: "intake".into(),
                sequence: 1,
                session_id: f.session,
                process_generation: 1,
                target: f.target,
                action: Action::MessageDeliver {
                    text: "External source text".into(),
                },
            };
            match case {
                "session" => request.session_id = Uuid::new_v4(),
                "process" => request.process_generation += 1,
                "target" => request.target.generation += 1,
                "grant" => {
                    f.grants.remove(&Capability::MessageDeliver);
                }
                "invalid" => request.sequence = 0,
                _ => {}
            }
            let context = ActionContext {
                session_id: f.session,
                process_generation: 1,
                target: (case != "no-target").then_some(f.target),
                grants: &f.grants,
                origin: &f.origin,
                desktop_owned: false,
            };
            let outcome = f.host.execute_with_messages(
                &request,
                &context,
                &f.catalog,
                ActionControllers {
                    animation: &mut f.animation,
                    behavior: &mut f.behavior,
                    external_messages: Some(&mut sink),
                },
            );
            assert!(
                matches!(outcome.result, ActionResult::Error { code: actual, .. } if actual == code),
                "{case}: {:?}",
                outcome.result
            );
            assert!(outcome.event.is_none(), "{case}");
            assert!(sink.calls.is_empty(), "{case} reached intake");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn real_connection_delivers_once_and_caches_message_success_and_denials() {
        use crate::{extension_connection::ExtensionConnection, extension_process::ProcessSpec};
        use std::time::{Duration, Instant};
        struct Child {
            connection: ExtensionConnection,
            root: PathBuf,
        }
        impl Drop for Child {
            fn drop(&mut self) {
                self.connection.shutdown(Duration::from_secs(2));
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
        for (case, expected_code, attempts) in [
            ("success", None, 1),
            ("sink-failure", Some("busy"), 1),
            ("revoked", Some("permission_denied"), 0),
            ("stale-target", Some("stale_target"), 0),
            ("absent", Some("unavailable"), 0),
        ] {
            let mut f = Fixture::new();
            let mut sink = MessageSink::default();
            if case == "sink-failure" {
                sink.error = Some((ErrorCode::Busy, "Intake queue is full".into()));
            }
            let root =
                std::env::temp_dir().join(format!("waifu-message-{case}-{}", Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let script = format!(
                r#"
import json, os, sys
from pathlib import Path
h = json.loads(sys.stdin.readline())
a = {{'api_version':1, 'type':'action', 'id':'intake', 'sequence':1,
     'session_id':h['session_id'], 'process_generation':h['process_generation'],
     'target':{}, 'action':{{'type':'message.deliver','text':'External source text'}}}}
records = []
for attempt in range(2):
    print(json.dumps(a), flush=True)
    for line in sys.stdin:
        envelope = json.loads(line)
        records.append(envelope)
        if envelope['message']['type'] == 'result':
            break
    else:
        raise AssertionError('Missing message acknowledgement')
data = Path(os.environ['WAIFU_EXTENSION_DATA'])
tmp = data / 'records.tmp'
tmp.write_text(json.dumps(records))
tmp.replace(data / 'records.json')
for line in sys.stdin:
    pass
"#,
                serde_json::to_string(&f.target).unwrap()
            );
            let mut child = Child {
                connection: ExtensionConnection::spawn(
                    ProcessSpec {
                        program: "/usr/bin/python3".into(),
                        args: vec!["-u".into(), "-c".into(), script],
                        working_directory: root.clone(),
                        data_directory: root.join("data"),
                        lifetime: Some(Duration::from_secs(10)),
                    },
                    f.origin.clone(),
                    [Capability::MessageDeliver],
                    1,
                )
                .unwrap(),
                root,
            };
            if case == "revoked" {
                child.connection.set_grants([]);
            }
            let target = Target {
                generation: f.target.generation + u64::from(case == "stale-target"),
                ..f.target
            };
            let deadline = Instant::now() + Duration::from_secs(4);
            let records: Vec<serde_json::Value> = loop {
                f.host.poll_connection(
                    &mut child.connection,
                    target,
                    false,
                    &f.catalog,
                    ActionControllers {
                        animation: &mut f.animation,
                        behavior: &mut f.behavior,
                        external_messages: if case == "absent" {
                            None
                        } else {
                            Some(&mut sink)
                        },
                    },
                );
                if let Ok(bytes) = std::fs::read(child.root.join("data/records.json")) {
                    break serde_json::from_slice(&bytes).unwrap();
                }
                assert!(
                    Instant::now() < deadline,
                    "{case}: {}",
                    child.connection.status()
                );
                std::thread::sleep(Duration::from_millis(2));
            };
            let results: Vec<_> = records
                .iter()
                .filter(|record| record["message"]["type"] == "result")
                .collect();
            assert_eq!(results.len(), 2, "{case}");
            assert_eq!(
                results[0]["message"]["result"], results[1]["message"]["result"],
                "duplicate must use cached response: {case}"
            );
            if let Some(code) = expected_code {
                assert_eq!(results[0]["message"]["result"]["status"], "error", "{case}");
                assert_eq!(results[0]["message"]["result"]["code"], code, "{case}");
            } else {
                assert_eq!(results[0]["message"]["result"]["value"]["type"], "accepted");
            }
            let events: Vec<_> = records
                .iter()
                .filter(|record| record["message"]["type"] == "event")
                .collect();
            assert_eq!(events.len(), usize::from(case == "success"), "{case}");
            assert_eq!(sink.calls.len(), attempts, "duplicate reached sink: {case}");
            if case == "success" {
                assert_eq!(
                    sink.calls[0],
                    (f.target, f.origin.clone(), "External source text".into())
                );
                assert_eq!(
                    events[0]["message"]["event"],
                    serde_json::json!({
                        "type":"message.delivered", "target": f.target,
                        "text":"External source text", "origin": f.origin,
                    })
                );
            }
            assert!(f.host.owners.is_empty());
            assert!(f.host.motions.is_empty());
        }
    }

    struct Fixture {
        session: Uuid,
        target: Target,
        origin: IntegrationOrigin,
        grants: BTreeSet<Capability>,
        catalog: ActionCatalog,
        animation: AnimationState,
        behavior: BehaviorController,
        host: HostActions,
    }

    #[test]
    fn playback_feedback_rechecks_host_identity_and_requires_opt_in_sink() {
        struct Sink {
            calls: usize,
        }
        impl ExternalMessageSink for Sink {
            fn deliver(
                &mut self,
                _: Target,
                _: &IntegrationOrigin,
                _: &str,
            ) -> Result<Uuid, (ErrorCode, String)> {
                panic!("receipt became text")
            }
            fn audio_playback_receipt(
                &mut self,
                target: Target,
                origin: &IntegrationOrigin,
                session: Uuid,
                generation: u64,
                receipt: &crate::extension_protocol::AudioPlaybackReceipt,
            ) -> Result<(), (ErrorCode, String)> {
                assert!(!target.uuid.is_nil());
                assert!(!session.is_nil());
                assert_eq!(generation, 1);
                assert_eq!(origin.integration_id, "org.example.fixture");
                receipt.validate().unwrap();
                self.calls += 1;
                Ok(())
            }
        }
        for case in [
            "ok",
            "grant",
            "session",
            "process",
            "target",
            "malformed",
            "absent",
        ] {
            let mut f = Fixture::new();
            if case != "grant" {
                f.grants.insert(Capability::AudioPlayback);
            }
            let mut request = ActionEnvelope {
                api_version: API_VERSION,
                message_type: ActionEnvelopeType::Action,
                id: "playback-1".into(),
                sequence: 1,
                session_id: f.session,
                process_generation: 1,
                target: f.target,
                action: Action::AudioPlaybackReceipt {
                    receipt: crate::extension_protocol::AudioPlaybackReceipt::Ready {
                        playback_id: if case == "malformed" {
                            Uuid::nil()
                        } else {
                            Uuid::new_v4()
                        },
                    },
                },
            };
            match case {
                "session" => request.session_id = Uuid::new_v4(),
                "process" => request.process_generation = 2,
                "target" => request.target.generation += 1,
                _ => {}
            }
            let mut sink = Sink { calls: 0 };
            let outcome = f.host.execute_with_messages(
                &request,
                &ActionContext {
                    session_id: f.session,
                    process_generation: 1,
                    target: Some(f.target),
                    grants: &f.grants,
                    origin: &f.origin,
                    desktop_owned: false,
                },
                &f.catalog,
                ActionControllers {
                    animation: &mut f.animation,
                    behavior: &mut f.behavior,
                    external_messages: if case == "absent" {
                        None
                    } else {
                        Some(&mut sink)
                    },
                },
            );
            assert!(outcome.event.is_none());
            if case == "ok" {
                assert_eq!(sink.calls, 1);
                assert!(matches!(outcome.result, ActionResult::Ok { .. }));
            } else {
                assert_eq!(sink.calls, 0, "{case}");
                let expected = match case {
                    "grant" => ErrorCode::PermissionDenied,
                    "session" => ErrorCode::StaleSession,
                    "process" => ErrorCode::StaleProcess,
                    "target" => ErrorCode::StaleTarget,
                    "malformed" => ErrorCode::InvalidRequest,
                    _ => ErrorCode::Unavailable,
                };
                assert!(
                    matches!(outcome.result, ActionResult::Error { code, .. } if code == expected),
                    "{case}"
                );
            }
        }
    }
    #[test]
    fn participant_audio_rechecks_grants_and_route_before_sink_admission() {
        struct AudioSink {
            calls: usize,
        }
        impl ExternalMessageSink for AudioSink {
            fn deliver(
                &mut self,
                _: Target,
                _: &IntegrationOrigin,
                _: &str,
            ) -> Result<Uuid, (ErrorCode, String)> {
                panic!("audio became direct text delivery")
            }
            fn audio_input(
                &mut self,
                _: Target,
                origin: &IntegrationOrigin,
                _: Uuid,
                generation: u64,
                _: &crate::extension_protocol::AudioInput,
            ) -> Result<(), (ErrorCode, String)> {
                assert_eq!(origin.integration_id, "org.example.fixture");
                assert_eq!(generation, 1);
                self.calls += 1;
                Ok(())
            }
        }
        for case in [
            "ok",
            "audio-grant",
            "message-grant",
            "session",
            "process",
            "target",
            "malformed",
            "absent",
        ] {
            let mut f = Fixture::new();
            f.grants.insert(Capability::AudioTranscribe);
            if case == "audio-grant" {
                f.grants.remove(&Capability::AudioTranscribe);
            }
            if case == "message-grant" {
                f.grants.remove(&Capability::MessageDeliver);
            }
            let mut request = ActionEnvelope {
                api_version: API_VERSION,
                message_type: ActionEnvelopeType::Action,
                id: "audio".into(),
                sequence: 1,
                session_id: f.session,
                process_generation: 1,
                target: f.target,
                action: Action::AudioInput {
                    input: crate::extension_protocol::AudioInput::Frame {
                        turn_id: "turn".into(),
                        frame_sequence: 0,
                        pcm: vec![0; if case == "malformed" { 319 } else { 320 }],
                    },
                },
            };
            match case {
                "session" => request.session_id = Uuid::new_v4(),
                "process" => request.process_generation = 2,
                "target" => request.target.generation += 1,
                _ => {}
            }
            let context = ActionContext {
                session_id: f.session,
                process_generation: 1,
                target: Some(f.target),
                grants: &f.grants,
                origin: &f.origin,
                desktop_owned: false,
            };
            let mut sink = AudioSink { calls: 0 };
            let outcome = f.host.execute_with_messages(
                &request,
                &context,
                &f.catalog,
                ActionControllers {
                    animation: &mut f.animation,
                    behavior: &mut f.behavior,
                    external_messages: if case == "absent" {
                        None
                    } else {
                        Some(&mut sink)
                    },
                },
            );
            assert!(outcome.event.is_none());
            if case == "ok" {
                assert_eq!(sink.calls, 1);
                assert!(matches!(outcome.result, ActionResult::Ok { .. }));
            } else {
                assert_eq!(sink.calls, 0, "{case}");
                let expected = match case {
                    "audio-grant" | "message-grant" => ErrorCode::PermissionDenied,
                    "session" => ErrorCode::StaleSession,
                    "process" => ErrorCode::StaleProcess,
                    "target" => ErrorCode::StaleTarget,
                    "malformed" => ErrorCode::InvalidRequest,
                    _ => ErrorCode::Unavailable,
                };
                assert!(
                    matches!(outcome.result, ActionResult::Error { code, .. } if code == expected),
                    "{case}"
                );
            }
        }
    }
    impl Fixture {
        fn new() -> Self {
            let target = Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            };
            let names = vec!["happy".into(), "blink".into()];
            let mut behavior = BehaviorController::default();
            behavior
                .select(Some(target.uuid), &BehaviorConfig::default(), &names)
                .unwrap();
            let pack = crate::extension_manifest::load_pack(
                &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/extensions/companion-starter"),
            )
            .unwrap();
            Self {
                session: Uuid::new_v4(),
                target,
                origin: IntegrationOrigin {
                    integration_id: "org.example.fixture".into(),
                    display_name: "Fixture".into(),
                },
                grants: [
                    Capability::MotionPlay,
                    Capability::ExpressionApply,
                    Capability::CatalogGet,
                    Capability::EventEmit,
                    Capability::MessageDeliver,
                ]
                .into(),
                catalog: ActionCatalog::from_packs([&pack], &names),
                animation: AnimationState::default(),
                behavior,
                host: HostActions::default(),
            }
        }
        fn execute(&mut self, action: Action) -> ActionOutcome {
            self.execute_named("test", action)
        }
        fn execute_named(&mut self, id: &str, action: Action) -> ActionOutcome {
            let request = ActionEnvelope {
                api_version: API_VERSION,
                message_type: ActionEnvelopeType::Action,
                id: id.into(),
                sequence: 1,
                session_id: self.session,
                process_generation: 1,
                target: self.target,
                action,
            };
            let context = ActionContext {
                session_id: self.session,
                process_generation: 1,
                target: Some(self.target),
                grants: &self.grants,
                origin: &self.origin,
                desktop_owned: false,
            };
            self.host.execute(
                &request,
                &context,
                &self.catalog,
                &mut self.animation,
                &mut self.behavior,
            )
        }
    }

    #[cfg(target_os = "linux")]
    struct MotionObserver {
        fixture: Fixture,
        connection: crate::extension_connection::ExtensionConnection,
        root: PathBuf,
    }

    #[cfg(target_os = "linux")]
    impl MotionObserver {
        fn new() -> Self {
            let mut fixture = Fixture::new();
            let root = std::env::temp_dir().join(format!("waifu-motion-events-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let script = r#"
import json, os, sys
from pathlib import Path
h = json.loads(sys.stdin.readline())
data = Path(os.environ['WAIFU_EXTENSION_DATA'])
records = []
for line in sys.stdin:
    envelope = json.loads(line)
    assert envelope['session_id'] == h['session_id']
    assert envelope['process_generation'] == h['process_generation']
    records.append(envelope)
    temporary = data / 'events.tmp'
    temporary.write_text(json.dumps(records))
    temporary.replace(data / 'events.json')
"#;
            let grants = [Capability::MotionPlay, Capability::LifecycleObserve];
            let connection = crate::extension_connection::ExtensionConnection::spawn(
                crate::extension_process::ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-u".into(), "-c".into(), script.into()],
                    working_directory: root.clone(),
                    data_directory: root.join("data"),
                    lifetime: Some(std::time::Duration::from_secs(10)),
                },
                fixture.origin.clone(),
                grants,
                1,
            )
            .unwrap();
            fixture.session = connection.session_id();
            fixture.grants = grants.into();
            Self {
                fixture,
                connection,
                root,
            }
        }

        fn admit(&mut self, id: &str, motion_id: &str) -> PlaybackReceipt {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
            loop {
                let outcome = self.fixture.execute_named(
                    id,
                    Action::MotionPlay {
                        motion_id: motion_id.into(),
                        mode: crate::extension_protocol::PlaybackMode::OneShot,
                    },
                );
                if matches!(outcome.result, ActionResult::Ok { .. }) {
                    return self.fixture.host.motions.last().unwrap().receipt.clone();
                }
                // The production importer has one process-wide permit. Retry
                // its real busy admission without injecting a playback outcome.
                assert!(
                    matches!(
                        outcome.result,
                        ActionResult::Error {
                            code: ErrorCode::Busy,
                            ..
                        }
                    ),
                    "{:?}",
                    outcome.result
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "animation importer stayed busy"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }

        fn poll(&mut self, target: Option<Target>) {
            self.fixture.host.poll_connection(
                &mut self.connection,
                target,
                false,
                &self.fixture.catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut self.fixture.animation,
                    behavior: &mut self.fixture.behavior,
                },
            );
        }

        fn events(&self) -> Vec<serde_json::Value> {
            match std::fs::read(self.root.join("data/events.json")) {
                Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
                Err(error) => panic!("cannot read motion receipts: {error}"),
            }
        }

        fn until_events(&mut self, count: usize) -> Vec<serde_json::Value> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
            loop {
                self.poll(Some(self.fixture.target));
                let events = self.events();
                if events.len() >= count {
                    return events;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "missing motion receipt: {}",
                    self.connection.status()
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for MotionObserver {
        fn drop(&mut self) {
            let _ = self.connection.shutdown(std::time::Duration::from_secs(2));
            self.fixture.host.cancel(
                self.fixture.session,
                &mut self.fixture.animation,
                &mut self.fixture.behavior,
            );
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn manual_same_file_replacement_preserves_both_correlated_terminals_once() {
        let mut observer = MotionObserver::new();
        let motion_id = observer
            .fixture
            .catalog
            .motions
            .keys()
            .next()
            .unwrap()
            .clone();
        let path = observer.fixture.catalog.motions[&motion_id].1.clone();
        let first = observer.admit("first", &motion_id);
        let clip = std::sync::Arc::new(
            crate::animation::parse_bvh(include_str!(
                "../assets/animations/companion/talking-nod.bvh"
            ))
            .unwrap(),
        );
        observer
            .fixture
            .animation
            .select_clip(clip.clone(), Some(path.clone()));
        observer.fixture.animation.take_manual_control();
        observer.fixture.animation.pause();
        assert_eq!(first.outcome(), PlaybackOutcome::Cancelled);
        observer.fixture.animation.resume_automatic_control();
        let second = observer.admit("second", &motion_id);
        assert_eq!(
            observer.fixture.host.motions.len(),
            2,
            "new admission overwrote a terminal receipt"
        );
        observer.fixture.animation.take_manual_control();
        assert_eq!(second.outcome(), PlaybackOutcome::Cancelled);
        let events = observer.until_events(2);
        for (event, action_id) in events.iter().zip(["first", "second"]) {
            assert_eq!(
                event["message"]["event"],
                serde_json::json!({
                    "type":"motion.finished", "target": observer.fixture.target,
                    "action_id": action_id, "motion_id": motion_id, "outcome":"cancelled"
                })
            );
        }
        for _ in 0..8 {
            observer.poll(Some(observer.fixture.target));
        }
        assert!(observer.fixture.host.motions.is_empty());
        assert_eq!(observer.events().len(), 2);
        assert!(observer.fixture.animation.manual_controlled());
        assert!(!observer.fixture.animation.playing);
        assert_eq!(observer.fixture.animation.path.as_ref(), Some(&path));
        assert!(std::sync::Arc::ptr_eq(
            observer.fixture.animation.clip.as_ref().unwrap(),
            &clip
        ));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn target_and_motion_grant_loss_report_original_target_but_observation_revocation_drops_events()
    {
        let mut observer = MotionObserver::new();
        let motion_id = observer
            .fixture
            .catalog
            .motions
            .keys()
            .next()
            .unwrap()
            .clone();
        let target = observer.fixture.target;
        let first = observer.admit("target-lost", &motion_id);
        observer.poll(None);
        assert_eq!(first.outcome(), PlaybackOutcome::Cancelled);
        let events = observer.until_events(1);
        assert_eq!(
            events[0]["message"]["event"]["target"],
            serde_json::to_value(target).unwrap()
        );
        assert_eq!(events[0]["message"]["event"]["action_id"], "target-lost");

        observer.fixture.animation.resume_automatic_control();
        let second = observer.admit("grant-lost", &motion_id);
        observer
            .connection
            .set_grants([Capability::LifecycleObserve]);
        observer.poll(Some(target));
        assert_eq!(second.outcome(), PlaybackOutcome::Cancelled);
        let events = observer.until_events(2);
        assert_eq!(events[1]["message"]["event"]["action_id"], "grant-lost");
        assert_eq!(events[1]["message"]["event"]["outcome"], "cancelled");

        observer.connection.set_grants([Capability::MotionPlay]);
        observer.fixture.animation.resume_automatic_control();
        let third = observer.admit("unobserved", &motion_id);
        observer.fixture.animation.take_manual_control();
        observer.poll(Some(target));
        assert_eq!(third.outcome(), PlaybackOutcome::Cancelled);
        assert!(observer.fixture.host.motions.is_empty());
        observer
            .connection
            .set_grants([Capability::MotionPlay, Capability::LifecycleObserve]);
        for _ in 0..8 {
            observer.poll(Some(target));
        }
        assert_eq!(
            observer.events().len(),
            2,
            "regrant resurrected an unobserved terminal"
        );

        observer.fixture.animation.resume_automatic_control();
        let fourth = observer.admit("disconnected", &motion_id);
        observer.connection.stop("fixture disconnect");
        observer.poll(None);
        assert_eq!(fourth.outcome(), PlaybackOutcome::Cancelled);
        assert!(observer.fixture.host.owners.is_empty());
        assert!(observer.fixture.host.motions.is_empty());
        let (terminals, loss) = observer
            .fixture
            .animation
            .motion_observations()
            .take_terminals();
        assert_eq!(loss, 0);
        assert_eq!(terminals.iter().filter(|(event,_)| matches!(event,
            crate::extension_lifecycle::LifecycleEvent::MotionPlaybackEnded {
                source: crate::extension_protocol::MotionPlaybackSource::Extension { action_id,session_id,.. },
                source_target:Some(source_target),outcome:crate::extension_protocol::MotionOutcome::Cancelled,..
            } if action_id=="disconnected" && *session_id==observer.fixture.session && *source_target==target)).count(),1,
            "disconnect erased general source evidence");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn real_import_failure_reports_failed_without_inventing_completion() {
        use bevy::prelude::*;
        let mut observer = MotionObserver::new();
        let path = observer.root.join("broken.bvh");
        std::fs::write(&path, b"not a BVH motion").unwrap();
        let motion_id = "org.example.fixture/broken";
        observer
            .fixture
            .catalog
            .motions
            .insert(motion_id.into(), ("Broken".into(), path));
        let receipt = observer.admit("failed-import", motion_id);
        assert_eq!(receipt.outcome(), PlaybackOutcome::Pending);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<crate::loading::AvatarLoadState>()
            .add_plugins(crate::animation::BvhAnimationPlugin);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while receipt.outcome() == PlaybackOutcome::Pending {
            app.insert_resource(std::mem::take(&mut observer.fixture.animation));
            app.update();
            observer.fixture.animation =
                app.world_mut().remove_resource::<AnimationState>().unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "import failure did not arrive"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(receipt.outcome(), PlaybackOutcome::Failed);
        let events = observer.until_events(1);
        assert_eq!(events[0]["message"]["event"]["action_id"], "failed-import");
        assert_eq!(events[0]["message"]["event"]["outcome"], "failed");
        assert!(observer.fixture.host.motions.is_empty());
        let (generic, lost) = observer
            .fixture
            .animation
            .motion_observations()
            .take_terminals();
        assert_eq!(lost, 0);
        assert_eq!(generic.len(), 1);
        assert!(
            matches!(&generic[0].0,crate::extension_lifecycle::LifecycleEvent::MotionPlaybackEnded {
            motion_id:Some(id),source:crate::extension_protocol::MotionPlaybackSource::Extension {action_id,..},outcome:crate::extension_protocol::MotionOutcome::Failed,..
        } if id==motion_id && action_id=="failed-import")
        );
    }

    #[test]
    fn undelivered_motion_receipts_are_bounded_and_disconnect_reclaims_them() {
        let mut fixture = Fixture::new();
        let clip = std::sync::Arc::new(
            crate::animation::parse_bvh(include_str!(
                "../assets/animations/companion/talking-nod.bvh"
            ))
            .unwrap(),
        );
        let motion_id = fixture.catalog.motions.keys().next().unwrap().clone();
        // Real source selections cancel each predecessor. Leave their receipts
        // undelivered to exercise admission backpressure, not a fake completion.
        for index in 0..MAX_OWNERS {
            let token = fixture.animation.select_owned_one_shot(clip.clone(), None);
            fixture.host.motions.push(TrackedMotion {
                session: fixture.session,
                generation: 1,
                target: fixture.target,
                action_id: format!("queued-{index}"),
                motion_id: motion_id.clone(),
                receipt: fixture.animation.owned_playback_receipt(token).unwrap(),
            });
            fixture.host.owners.insert(
                fixture.session,
                OwnedActions {
                    generation: 1,
                    target: fixture.target,
                    motion: Some(token),
                    expression: None,
                },
            );
        }
        let outcome = fixture.execute(Action::MotionPlay {
            motion_id,
            mode: crate::extension_protocol::PlaybackMode::OneShot,
        });
        assert!(
            matches!(outcome.result, ActionResult::Error { code: ErrorCode::Busy, ref message } if message.contains("receipt limit"))
        );
        assert_eq!(fixture.host.motions.len(), MAX_OWNERS);
        fixture.host.cancel(
            fixture.session,
            &mut fixture.animation,
            &mut fixture.behavior,
        );
        assert!(fixture.host.motions.is_empty());
        assert!(fixture.host.owners.is_empty());
        assert!(!fixture.animation.playing);
    }
    #[test]
    fn catalog_exposes_registered_ids_without_paths_or_reserved_channels() {
        let mut f = Fixture::new();
        let ActionResult::Ok {
            value:
                ResultValue::Catalog {
                    motions,
                    expressions,
                },
        } = f.execute(Action::CatalogGet {}).result
        else {
            panic!("catalog rejected");
        };
        assert_eq!(motions.len(), 3);
        assert!(motions.iter().all(|motion| {
            motion
                .motion_id
                .starts_with("org.waifu-studio.companion-starter/")
        }));
        assert_eq!(
            expressions,
            vec![RegisteredExpression {
                name: "happy".into()
            }]
        );
    }
    #[test]
    fn permission_and_manual_hold_rejections_have_no_motion_side_effect() {
        let mut f = Fixture::new();
        let action = Action::MotionPlay {
            motion_id: f.catalog.motions.keys().next().unwrap().clone(),
            mode: crate::extension_protocol::PlaybackMode::OneShot,
        };
        f.animation.take_manual_control();
        assert!(matches!(
            f.execute(action.clone()).result,
            ActionResult::Error {
                code: ErrorCode::Busy,
                ..
            }
        ));
        f.grants.remove(&Capability::MotionPlay);
        assert!(matches!(
            f.execute(action).result,
            ActionResult::Error {
                code: ErrorCode::PermissionDenied,
                ..
            }
        ));
        assert!(!f.animation.loading);
        assert!(f.animation.manual_controlled());
    }
    #[test]
    fn cancelled_overlay_restores_baseline_and_external_text_is_not_delivered_as_user() {
        let mut f = Fixture::new();
        assert!(matches!(
            f.execute(Action::ExpressionApply {
                name: "happy".into(),
                strength: 0.5,
                duration_ms: 1000
            })
            .result,
            ActionResult::Ok { .. }
        ));
        let names = vec!["happy".into()];
        assert!(
            !f.behavior
                .weights_for(Some(f.target.uuid), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        f.host.cancel(f.session, &mut f.animation, &mut f.behavior);
        assert!(
            f.behavior
                .weights_for(Some(f.target.uuid), &names, &BTreeMap::new(), "")
                .is_empty()
        );
        assert!(matches!(
            f.execute(Action::MessageDeliver {
                text: "I live in Paris".into()
            })
            .result,
            ActionResult::Error {
                code: ErrorCode::Unavailable,
                ..
            }
        ));
    }
    #[test]
    fn event_origin_is_host_owned_and_namespace_is_rechecked() {
        let mut f = Fixture::new();
        assert!(matches!(
            f.execute(Action::EventEmit {
                name: "integration.org.other.subscription".into(),
                data: serde_json::json!({})
            })
            .result,
            ActionResult::Error {
                code: ErrorCode::PermissionDenied,
                ..
            }
        ));
        let result = f.execute(Action::EventEmit {
            name: "integration.org.example.fixture.subscription".into(),
            data: serde_json::json!({}),
        });
        assert!(
            matches!(result.event, Some(HostEvent::Emitted { origin, .. }) if origin == f.origin)
        );
    }

    #[test]
    fn revocation_without_another_action_cancels_owned_expression() {
        let mut f = Fixture::new();
        f.execute(Action::ExpressionApply {
            name: "happy".into(),
            strength: 0.5,
            duration_ms: 1000,
        });
        f.grants.remove(&Capability::ExpressionApply);
        let context = ActionContext {
            session_id: f.session,
            process_generation: 1,
            target: Some(f.target),
            grants: &f.grants,
            origin: &f.origin,
            desktop_owned: false,
        };
        f.host
            .reconcile(&context, &mut f.animation, &mut f.behavior);
        assert!(
            f.behavior
                .weights_for(Some(f.target.uuid), &["happy".into()], &BTreeMap::new(), "")
                .is_empty()
        );
    }

    #[test]
    fn target_change_rejects_previously_admitted_request() {
        let mut f = Fixture::new();
        let request = ActionEnvelope {
            api_version: API_VERSION,
            message_type: ActionEnvelopeType::Action,
            id: "old-avatar".into(),
            sequence: 1,
            session_id: f.session,
            process_generation: 1,
            target: f.target,
            action: Action::ExpressionApply {
                name: "happy".into(),
                strength: 0.5,
                duration_ms: 1000,
            },
        };
        let context = ActionContext {
            session_id: f.session,
            process_generation: 1,
            target: Some(Target {
                generation: 2,
                ..f.target
            }),
            grants: &f.grants,
            origin: &f.origin,
            desktop_owned: false,
        };
        let outcome = f.host.execute(
            &request,
            &context,
            &f.catalog,
            &mut f.animation,
            &mut f.behavior,
        );
        assert!(matches!(
            outcome.result,
            ActionResult::Error {
                code: ErrorCode::StaleTarget,
                ..
            }
        ));
        assert!(f.host.owners.is_empty());
    }

    #[test]
    fn escaped_catalog_names_fit_protocol_and_expired_owners_are_reclaimed() {
        let mut f = Fixture::new();
        for index in 0..128 {
            f.catalog.motions.insert(
                format!("org.example.fixture/motion-{index}"),
                ("\u{1}".repeat(128), "unused.bvh".into()),
            );
        }
        let outcome = f.execute(Action::CatalogGet {});
        outcome.result.validate().unwrap();
        assert!(serde_json::to_vec(&outcome.result).unwrap().len() < 50 * 1024);
        for _ in 0..40 {
            f.session = Uuid::new_v4();
            assert!(matches!(
                f.execute(Action::ExpressionApply {
                    name: "happy".into(),
                    strength: 0.5,
                    duration_ms: 1
                })
                .result,
                ActionResult::Ok { .. }
            ));
            f.behavior.tick(0.01);
        }
        assert_eq!(f.host.owners.len(), 1);
    }

    #[test]
    fn desktop_transfer_cannot_park_a_live_plugin_request() {
        let mut f = Fixture::new();
        let clip = crate::animation::parse_bvh(include_str!(
            "../assets/animations/companion/talking-nod.bvh"
        ))
        .unwrap();
        let token = f
            .animation
            .select_owned_one_shot(std::sync::Arc::new(clip), Some("test.bvh".into()));
        assert!(f.animation.owned_playback_active(token));
        f.animation.cancel_owned_requests();
        let parked = std::mem::take(&mut f.animation);
        assert!(!parked.playing);
        assert!(!parked.owned_playback_active(token));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn supervised_child_drives_and_releases_real_host_expression() {
        use crate::{extension_connection::ExtensionConnection, extension_process::ProcessSpec};
        use std::time::{Duration, Instant};
        let mut f = Fixture::new();
        let root = std::env::temp_dir().join(format!("waifu-host-bridge-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = format!(
            r#"
import json, sys, time
h = json.loads(sys.stdin.readline())
a = {{'api_version': 1, 'type': 'action', 'id': 'smile', 'sequence': 1,
     'session_id': h['session_id'], 'process_generation': h['process_generation'],
     'target': {}, 'action': {{'type': 'expression.apply', 'name': 'happy', 'strength': 0.5, 'duration_ms': 10000}}}}
print(json.dumps(a), flush=True)
r = json.loads(sys.stdin.readline())
assert r['message']['result']['status'] == 'ok'
print('accepted', file=sys.stderr, flush=True)
time.sleep(30)
"#,
            serde_json::to_string(&f.target).unwrap()
        );
        let mut connection = ExtensionConnection::spawn(
            ProcessSpec {
                program: "/usr/bin/python3".into(),
                args: vec!["-u".into(), "-c".into(), script],
                working_directory: root.clone(),
                data_directory: root.join("data"),
                lifetime: Some(Duration::from_secs(5)),
            },
            f.origin.clone(),
            [Capability::ExpressionApply],
            1,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            f.host.poll_connection(
                &mut connection,
                f.target,
                false,
                &f.catalog,
                ActionControllers {
                    external_messages: None,
                    animation: &mut f.animation,
                    behavior: &mut f.behavior,
                },
            );
            if connection
                .logs()
                .iter()
                .any(|line| line.contains("accepted"))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child did not receive expression acknowledgement: {}",
                connection.status()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !f.behavior
                .weights_for(Some(f.target.uuid), &["happy".into()], &BTreeMap::new(), "")
                .is_empty()
        );
        connection.stop("fixture disable");
        f.host.poll_connection(
            &mut connection,
            f.target,
            false,
            &f.catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut f.animation,
                behavior: &mut f.behavior,
            },
        );
        assert!(
            f.behavior
                .weights_for(Some(f.target.uuid), &["happy".into()], &BTreeMap::new(), "")
                .is_empty()
        );
        while connection.termination().is_none() {
            connection.drain(f.target);
            assert!(Instant::now() < deadline, "child shutdown hung");
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
