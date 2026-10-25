//! A private portal connection per explicitly enabled worker. All polling and
//! bounded teardown run off the render thread; dropping the handle only cancels.

use super::{Action, PortalEvent};
use bevy::tasks::futures_lite::{
    StreamExt,
    future::{block_on, poll_once},
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    future::Future,
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    thread,
    time::{Duration, Instant},
};
use zbus::{
    Connection, MatchRule, MessageStream,
    message::{Message, Sequence, Type},
    zvariant::{DynamicType, ObjectPath, OwnedObjectPath, OwnedValue, Str},
};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
const POLL: Duration = Duration::from_millis(10);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);
const DELIVERY_TIMEOUT: Duration = Duration::from_millis(500);
const CLOSE_TIMEOUT: Duration = Duration::from_millis(300);

type Properties = HashMap<String, OwnedValue>;
type Shortcuts = Vec<(String, Properties)>;
type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug)]
enum Failure {
    Cancelled,
    Error(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Self::Cancelled => "Global shortcuts cancelled (worker disabled).".into(),
            Self::Error(message) => message,
        }
    }
}

fn error(context: &str, detail: impl std::fmt::Display) -> Failure {
    Failure::Error(format!("Global shortcuts {context}: {detail}"))
}

/// There is deliberately no JoinHandle: Drop never waits for D-Bus or a dialog.
pub(super) struct PortalWorker {
    cancel: SyncSender<()>,
}

impl Drop for PortalWorker {
    fn drop(&mut self) {
        let _ = self.cancel.try_send(());
    }
}

pub(super) fn start(
    generation: u64,
    tx: SyncSender<(u64, PortalEvent)>,
) -> std::result::Result<PortalWorker, String> {
    let (cancel, rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name(format!("voice-shortcuts-{generation}"))
        .spawn(move || {
            worker(Control {
                generation,
                tx,
                cancel: rx,
                cancelled: Cell::new(false),
            })
        })
        .map_err(|e| format!("Could not start global shortcuts worker: {e}"))?;
    Ok(PortalWorker { cancel })
}

struct Control {
    generation: u64,
    tx: SyncSender<(u64, PortalEvent)>,
    cancel: Receiver<()>,
    cancelled: Cell<bool>,
}

impl Control {
    fn check(&self) -> Result<()> {
        if self.cancelled.get() {
            return Err(Failure::Cancelled);
        }
        match self.cancel.try_recv() {
            Err(mpsc::TryRecvError::Empty) => Ok(()),
            _ => {
                self.cancelled.set(true);
                Err(Failure::Cancelled)
            }
        }
    }

    fn pause(&self, deadline: Instant, context: &str) -> Result<()> {
        self.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(error(context, "timed out"));
        }
        match self.cancel.recv_timeout(POLL.min(remaining)) {
            Err(RecvTimeoutError::Timeout) => Ok(()),
            _ => {
                self.cancelled.set(true);
                Err(Failure::Cancelled)
            }
        }
    }

    fn wait<F: Future>(&self, future: F, timeout: Duration, context: &str) -> Result<F::Output> {
        let mut future = std::pin::pin!(future);
        let deadline = Instant::now() + timeout;
        loop {
            self.check()?;
            if let Some(result) = block_on(poll_once(future.as_mut())) {
                return Ok(result);
            }
            self.pause(deadline, context)?;
        }
    }

    fn send(&self, event: PortalEvent) -> Result<()> {
        let deadline = Instant::now() + DELIVERY_TIMEOUT;
        let mut event = (self.generation, event);
        loop {
            self.check()?;
            match self.tx.try_send(event) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => {
                    return Err(error("closed", "event receiver disconnected"));
                }
                Err(TrySendError::Full(value)) => event = value,
            }
            self.pause(deadline, "event delivery (queue full)")?;
        }
    }

    fn closed(&self, message: String) {
        // Preserve terminal status on cancellation and transient backpressure.
        // A disconnected or permanently undrained receiver cannot be notified.
        let deadline = Instant::now() + DELIVERY_TIMEOUT;
        let mut event = (self.generation, PortalEvent::Closed(message));
        loop {
            match self.tx.try_send(event) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(value)) => event = value,
            }
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(POLL);
        }
    }
}

fn worker(control: Control) {
    let connection = match control.wait(
        Connection::session(),
        IO_TIMEOUT,
        "connecting to session bus",
    ) {
        Ok(Ok(connection)) => connection,
        Ok(Err(e)) => return control.closed(error("unavailable", e).message()),
        Err(e) => return control.closed(e.message()),
    };
    let mut portal = Portal {
        connection,
        owner: DESTINATION.to_owned(),
        session: None,
        pending_request: None,
    };
    let result = portal.run(&control);
    // Also covers cancellation with CreateSession/BindShortcuts still pending.
    portal.close();
    control.closed(match result {
        Ok(()) => "Global shortcuts closed.".into(),
        Err(e) => e.message(),
    });
}

struct Portal {
    connection: Connection,
    owner: String,
    session: Option<OwnedObjectPath>,
    pending_request: Option<OwnedObjectPath>,
}

struct Signals {
    shortcuts: MessageStream,
    closed: MessageStream,
    owner_lost: MessageStream,
}

impl Signals {
    fn check_closed(&mut self) -> Result<()> {
        if let Some(message) = block_on(poll_once(self.closed.next())) {
            return match message {
                Some(Ok(_)) => Err(error("closed", "portal session ended")),
                _ => Err(error("unavailable", "session signal stream ended")),
            };
        }
        if let Some(message) = block_on(poll_once(self.owner_lost.next())) {
            let message = stream_message(message, "portal owner signal stream")?;
            let (_, _, owner): (String, String, String) = message
                .body()
                .deserialize()
                .map_err(|e| error("invalid owner signal", e))?;
            if owner.is_empty() {
                return Err(error("unavailable", "desktop portal exited"));
            }
        }
        Ok(())
    }
}

fn stream_message(message: Option<zbus::Result<Message>>, context: &str) -> Result<Message> {
    match message {
        Some(Ok(message)) => Ok(message),
        Some(Err(e)) => Err(error(context, e)),
        None => Err(error(context, "stream ended")),
    }
}

impl Portal {
    fn run(&mut self, control: &Control) -> Result<()> {
        // Detect unsupported portals without opening a permission dialog, then
        // pin the unique owner so another bus peer cannot spoof portal signals.
        let reply = control
            .wait(
                self.connection.call_method(
                    Some(DESTINATION),
                    DESKTOP,
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &(SHORTCUTS, "version"),
                ),
                IO_TIMEOUT,
                "querying portal version",
            )?
            .map_err(|e| error("unavailable", e))?;
        let version: OwnedValue = reply
            .body()
            .deserialize()
            .map_err(|e| error("invalid version", e))?;
        if u32::try_from(&version).map_err(|e| error("invalid version", e))? == 0 {
            return Err(error("unavailable", "GlobalShortcuts is unsupported"));
        }
        self.owner = reply
            .header()
            .sender()
            .ok_or_else(|| error("unavailable", "portal reply has no owner"))?
            .as_str()
            .to_owned();

        let sender = self
            .connection
            .unique_name()
            .ok_or_else(|| error("unavailable", "session bus assigned no unique name"))?
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let session_token = token();
        let session = object_path(format!(
            "/org/freedesktop/portal/desktop/session/{sender}/{session_token}"
        ))?;
        // Record the predicted session *before* CreateSession can create it.
        self.session = Some(session.clone());
        let mut signals = self.subscribe(control, &session)?;

        let create_token = token();
        let mut create_options = options(&create_token);
        create_options.insert("session_handle_token".into(), string_value(&session_token));
        let (mut created, _) = self.request(
            control,
            &mut signals,
            &sender,
            &create_token,
            "CreateSession",
            &create_options,
            REQUEST_TIMEOUT,
        )?;
        let returned = created
            .remove("session_handle")
            .ok_or_else(|| error("CreateSession", "missing session_handle"))?;
        // The portal spec historically encoded this object path as a string.
        let returned = <&ObjectPath<'_>>::try_from(&returned)
            .map(|p| p.as_str())
            .or_else(|_| <&str>::try_from(&returned))
            .map_err(|e| error("invalid session_handle", e))?;
        if returned != session.as_str() {
            return Err(error(
                "CreateSession",
                "returned an unexpected session path",
            ));
        }

        let bind_token = token();
        self.request(
            control,
            &mut signals,
            &sender,
            &bind_token,
            "BindShortcuts",
            &(
                session.clone(),
                requested_shortcuts(),
                "",
                options(&bind_token),
            ),
            PERMISSION_TIMEOUT,
        )?;
        let list_token = token();
        let (mut listed, snapshot) = self.request(
            control,
            &mut signals,
            &sender,
            &list_token,
            "ListShortcuts",
            &(session.clone(), options(&list_token)),
            REQUEST_TIMEOUT,
        )?;
        let bindings = select_bindings(shortcuts_result(&mut listed)?)?;
        let mut state = State {
            session,
            bindings,
            pressed: HashSet::new(),
        };
        control.send(PortalEvent::Ready(state.bindings.clone()))?;

        loop {
            control.check()?;
            signals.check_closed()?;
            if self.connection.is_closed() {
                return Err(error("unavailable", "session bus disconnected"));
            }
            if let Some(message) = block_on(poll_once(signals.shortcuts.next())) {
                let message = stream_message(message, "shortcut signal stream")?;
                // Earlier changes must not overwrite the ListShortcuts snapshot.
                // Track setup-time presses silently so repeats cannot fire until
                // a matching Deactivated, even when held across setup completion.
                if message.recv_position() > snapshot {
                    if let Some(event) = state.signal(&message)? {
                        control.send(event)?;
                    }
                } else if matches!(
                    message.header().member().map(|member| member.as_str()),
                    Some("Activated" | "Deactivated")
                ) {
                    let _ = state.signal(&message)?;
                }
            } else {
                control.pause(Instant::now() + IO_TIMEOUT, "waiting for shortcuts")?;
            }
        }
    }

    fn subscribe(&self, control: &Control, session: &OwnedObjectPath) -> Result<Signals> {
        let shortcuts = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(self.owner.as_str())
            .map_err(|e| error("signal rule", e))?
            .path(DESKTOP)
            .map_err(|e| error("signal rule", e))?
            .interface(SHORTCUTS)
            .map_err(|e| error("signal rule", e))?
            .arg_path(0, session.as_str())
            .map_err(|e| error("signal rule", e))?
            .build();
        let closed = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(self.owner.as_str())
            .map_err(|e| error("session rule", e))?
            .path(session.as_str())
            .map_err(|e| error("session rule", e))?
            .interface(SESSION)
            .map_err(|e| error("session rule", e))?
            .member("Closed")
            .map_err(|e| error("session rule", e))?
            .build();
        let owner_lost = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")
            .map_err(|e| error("owner rule", e))?
            .interface("org.freedesktop.DBus")
            .map_err(|e| error("owner rule", e))?
            .path("/org/freedesktop/DBus")
            .map_err(|e| error("owner rule", e))?
            .member("NameOwnerChanged")
            .map_err(|e| error("owner rule", e))?
            .arg(0, self.owner.as_str())
            .map_err(|e| error("owner rule", e))?
            .build();
        Ok(Signals {
            shortcuts: control
                .wait(
                    MessageStream::for_match_rule(shortcuts, &self.connection, Some(32)),
                    IO_TIMEOUT,
                    "subscribing to shortcuts",
                )?
                .map_err(|e| error("subscribing to shortcuts", e))?,
            closed: control
                .wait(
                    MessageStream::for_match_rule(closed, &self.connection, Some(1)),
                    IO_TIMEOUT,
                    "subscribing to session closure",
                )?
                .map_err(|e| error("subscribing to session closure", e))?,
            owner_lost: control
                .wait(
                    MessageStream::for_match_rule(owner_lost, &self.connection, Some(1)),
                    IO_TIMEOUT,
                    "subscribing to portal exit",
                )?
                .map_err(|e| error("subscribing to portal exit", e))?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn request<B: serde::Serialize + DynamicType>(
        &mut self,
        control: &Control,
        signals: &mut Signals,
        sender: &str,
        token: &str,
        method: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<(Properties, Sequence)> {
        let path = object_path(format!(
            "/org/freedesktop/portal/desktop/request/{sender}/{token}"
        ))?;
        self.pending_request = Some(path.clone());
        let deadline = Instant::now() + timeout;
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(self.owner.as_str())
            .map_err(|e| error("response rule", e))?
            .path(path.as_str())
            .map_err(|e| error("response rule", e))?
            .interface(REQUEST)
            .map_err(|e| error("response rule", e))?
            .member("Response")
            .map_err(|e| error("response rule", e))?
            .build();
        // Complete AddMatch BEFORE sending the request. The predicted UUID path
        // catches Response even if it precedes the method's object-path reply.
        let mut response = control
            .wait(
                MessageStream::for_match_rule(rule, &self.connection, Some(1)),
                IO_TIMEOUT,
                "subscribing to request response",
            )?
            .map_err(|e| error("subscribing to request response", e))?;
        signals.check_closed()?;
        let reply = control
            .wait(
                self.connection.call_method(
                    Some(self.owner.as_str()),
                    DESKTOP,
                    Some(SHORTCUTS),
                    method,
                    body,
                ),
                IO_TIMEOUT,
                method,
            )?
            .map_err(|e| error(method, e))?;
        let returned: OwnedObjectPath = reply.body().deserialize().map_err(|e| error(method, e))?;
        if returned != path {
            return Err(error(method, "returned an unexpected request path"));
        }
        loop {
            control.check()?;
            signals.check_closed()?;
            if let Some(message) = block_on(poll_once(response.next())) {
                let message = stream_message(message, method)?;
                let (code, results): (u32, Properties) =
                    message.body().deserialize().map_err(|e| error(method, e))?;
                self.pending_request = None;
                response_status(method, code)?;
                return Ok((results, message.recv_position()));
            }
            control.pause(deadline, method)?;
        }
    }

    fn close(self) {
        if let Some(path) = &self.pending_request {
            bounded_cleanup(self.connection.call_method(
                Some(self.owner.as_str()),
                path.as_str(),
                Some(REQUEST),
                "Close",
                &(),
            ));
        }
        if let Some(path) = &self.session {
            bounded_cleanup(self.connection.call_method(
                Some(self.owner.as_str()),
                path.as_str(),
                Some(SESSION),
                "Close",
                &(),
            ));
        }
        // Actively close the private socket, even with zbus internal references.
        bounded_cleanup(self.connection.close());
    }
}

fn bounded_cleanup<F: Future>(future: F) {
    let mut future = std::pin::pin!(future);
    let deadline = Instant::now() + CLOSE_TIMEOUT;
    loop {
        if block_on(poll_once(future.as_mut())).is_some() || Instant::now() >= deadline {
            return;
        }
        thread::sleep(POLL);
    }
}

fn token() -> String {
    format!("waifu_{}", uuid::Uuid::new_v4().simple())
}

fn object_path(path: String) -> Result<OwnedObjectPath> {
    OwnedObjectPath::try_from(path).map_err(|e| error("invalid object path", e))
}

fn string_value(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

fn options(token: &str) -> Properties {
    HashMap::from([("handle_token".into(), string_value(token))])
}

fn requested_shortcuts() -> Shortcuts {
    Action::ALL
        .into_iter()
        .map(|action| {
            (
                action.id().into(),
                HashMap::from([
                    ("description".into(), string_value(action.label())),
                    (
                        "preferred_trigger".into(),
                        string_value(action.preferred_trigger()),
                    ),
                ]),
            )
        })
        .collect()
}

fn response_status(method: &str, code: u32) -> Result<()> {
    match code {
        0 => Ok(()),
        1 => Err(error(method, "cancelled by the user (portal response 1)")),
        2 => Err(error(method, "failed or denied by the portal (response 2)")),
        _ => Err(error(method, format!("unknown portal response {code}"))),
    }
}

fn shortcuts_result(results: &mut Properties) -> Result<Shortcuts> {
    results
        .remove("shortcuts")
        .ok_or_else(|| error("ListShortcuts", "missing shortcuts"))?
        .try_into()
        .map_err(|e| error("invalid shortcuts", e))
}

fn action_for_id(id: &str) -> Option<Action> {
    Action::ALL.into_iter().find(|action| action.id() == id)
}

fn select_bindings(shortcuts: Shortcuts) -> Result<Vec<(Action, String)>> {
    let mut bindings = Vec::with_capacity(Action::ALL.len());
    for action in Action::ALL {
        let mut matches = shortcuts.iter().filter(|(id, _)| id == action.id());
        let Some((_, properties)) = matches.next() else {
            continue;
        };
        // Ambiguous duplicate IDs cannot establish a reliable approved binding.
        if matches.next().is_some() {
            continue;
        }
        if let Some(description) = properties
            .get("trigger_description")
            .and_then(|v| <&str>::try_from(v).ok())
            .filter(|description| !description.trim().is_empty())
        {
            bindings.push((action, description.to_owned()));
        }
    }
    if bindings.is_empty() {
        return Err(error(
            "unavailable",
            "no supported shortcuts were approved with a trigger description",
        ));
    }
    Ok(bindings)
}

struct State {
    session: OwnedObjectPath,
    bindings: Vec<(Action, String)>,
    pressed: HashSet<Action>,
}

impl State {
    fn signal(&mut self, message: &Message) -> Result<Option<PortalEvent>> {
        let header = message.header();
        let member = header.member().map(|member| member.as_str());
        match member {
            Some("Activated" | "Deactivated") => {
                let (session, id, _timestamp, _options): (
                    OwnedObjectPath,
                    String,
                    u64,
                    Properties,
                ) = message
                    .body()
                    .deserialize()
                    .map_err(|e| error("invalid shortcut signal", e))?;
                if session != self.session {
                    return Ok(None);
                }
                let Some(action) = action_for_id(&id) else {
                    return Ok(None);
                };
                if member == Some("Deactivated") {
                    self.pressed.remove(&action);
                } else if self.bindings.iter().any(|(bound, _)| *bound == action)
                    && self.pressed.insert(action)
                {
                    return Ok(Some(PortalEvent::Trigger(action)));
                }
            }
            Some("ShortcutsChanged") => {
                let (session, shortcuts): (OwnedObjectPath, Shortcuts) = message
                    .body()
                    .deserialize()
                    .map_err(|e| error("invalid shortcut change", e))?;
                if session != self.session {
                    return Ok(None);
                }
                let bindings = select_bindings(shortcuts)?;
                if bindings != self.bindings {
                    self.bindings = bindings;
                    // Keep held-key latches across removal/rebinding until release.
                    return Ok(Some(PortalEvent::Ready(self.bindings.clone())));
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> OwnedObjectPath {
        OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_42/test").unwrap()
    }

    fn shortcut(id: &str, description: &str) -> (String, Properties) {
        (
            id.into(),
            HashMap::from([("trigger_description".into(), string_value(description))]),
        )
    }

    fn signal(member: &str, session: OwnedObjectPath, id: &str) -> Message {
        Message::signal(DESKTOP, SHORTCUTS, member)
            .unwrap()
            .build(&(session, id, 123_u64, Properties::new()))
            .unwrap()
    }

    fn state() -> State {
        State {
            session: session(),
            bindings: vec![(Action::Microphone, "Actual desktop binding".into())],
            pressed: HashSet::new(),
        }
    }

    fn control(capacity: usize) -> (PortalWorker, Control, Receiver<(u64, PortalEvent)>) {
        let (cancel, rx) = mpsc::sync_channel(1);
        let (tx, events) = mpsc::sync_channel(capacity);
        (
            PortalWorker { cancel },
            Control {
                generation: 7,
                tx,
                cancel: rx,
                cancelled: Cell::new(false),
            },
            events,
        )
    }

    #[test]
    fn selects_only_exact_unique_bound_ids_and_actual_descriptions() {
        let bindings = select_bindings(vec![
            shortcut("unknown", "Ctrl+X"),
            shortcut(&format!("{}-suffix", Action::Stop.id()), "Ctrl+S"),
            shortcut(Action::Microphone.id(), "Custom microphone key"),
            shortcut(Action::Voice.id(), "   "),
            shortcut(Action::Settings.id(), "First"),
            shortcut(Action::Settings.id(), "Duplicate"),
        ])
        .unwrap();
        assert_eq!(
            bindings,
            vec![(Action::Microphone, "Custom microphone key".into())]
        );
        assert!(select_bindings(vec![]).is_err());
        assert!(select_bindings(vec![(Action::Voice.id().into(), Properties::new())]).is_err());
    }

    #[test]
    fn repeat_latch_requires_matching_session_deactivation() {
        let mut state = state();
        let activate = signal("Activated", session(), Action::Microphone.id());
        assert!(matches!(
            state.signal(&activate).unwrap(),
            Some(PortalEvent::Trigger(Action::Microphone))
        ));
        assert!(state.signal(&activate).unwrap().is_none());
        let other = OwnedObjectPath::try_from("/other/session").unwrap();
        state
            .signal(&signal(
                "Deactivated",
                other.clone(),
                Action::Microphone.id(),
            ))
            .unwrap();
        assert!(state.signal(&activate).unwrap().is_none());
        assert!(
            state
                .signal(&signal("Activated", other, Action::Stop.id()))
                .unwrap()
                .is_none()
        );
        state
            .signal(&signal("Deactivated", session(), Action::Microphone.id()))
            .unwrap();
        assert!(matches!(
            state.signal(&activate).unwrap(),
            Some(PortalEvent::Trigger(Action::Microphone))
        ));
        assert!(
            state
                .signal(&signal("Activated", session(), Action::Stop.id()))
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .signal(&signal("Activated", session(), "Microphone"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn changes_use_matching_session_and_never_invent_ready() {
        let mut state = state();
        let changed = |session, shortcuts: Shortcuts| {
            Message::signal(DESKTOP, SHORTCUTS, "ShortcutsChanged")
                .unwrap()
                .build(&(session, shortcuts))
                .unwrap()
        };
        assert!(
            state
                .signal(&changed(
                    OwnedObjectPath::try_from("/other").unwrap(),
                    vec![]
                ))
                .unwrap()
                .is_none()
        );
        let change = changed(
            session(),
            vec![shortcut(Action::Voice.id(), "Desktop-selected key")],
        );
        match state.signal(&change).unwrap() {
            Some(PortalEvent::Ready(bindings)) => assert_eq!(
                bindings,
                vec![(Action::Voice, "Desktop-selected key".into())]
            ),
            _ => panic!("expected actual changed bindings"),
        }
        assert!(state.signal(&change).unwrap().is_none());
        assert!(state.signal(&changed(session(), vec![])).is_err());
    }

    #[test]
    fn rebinding_does_not_reset_a_held_shortcut() {
        let mut state = state();
        let activate = signal("Activated", session(), Action::Microphone.id());
        state.signal(&activate).unwrap();
        let change = Message::signal(DESKTOP, SHORTCUTS, "ShortcutsChanged")
            .unwrap()
            .build(&(
                session(),
                vec![shortcut(Action::Microphone.id(), "New key")],
            ))
            .unwrap();
        state.signal(&change).unwrap();
        assert!(state.signal(&activate).unwrap().is_none());
    }

    #[test]
    fn wire_signatures_and_early_response_are_offline() {
        let method = |name, signature: &str, body: Message| {
            assert_eq!(body.header().member().unwrap().as_str(), name);
            assert_eq!(body.body().signature().to_string_no_parens(), signature);
        };
        method(
            "CreateSession",
            "a{sv}",
            Message::method_call(DESKTOP, "CreateSession")
                .unwrap()
                .build(&options("create"))
                .unwrap(),
        );
        method(
            "BindShortcuts",
            "oa(sa{sv})sa{sv}",
            Message::method_call(DESKTOP, "BindShortcuts")
                .unwrap()
                .build(&(session(), requested_shortcuts(), "", options("bind")))
                .unwrap(),
        );
        method(
            "ListShortcuts",
            "oa{sv}",
            Message::method_call(DESKTOP, "ListShortcuts")
                .unwrap()
                .build(&(session(), options("list")))
                .unwrap(),
        );
        assert_eq!(
            signal("Activated", session(), Action::Microphone.id())
                .body()
                .signature()
                .to_string_no_parens(),
            "osta{sv}"
        );

        // Fake transport: Response arrives before the method's path reply.
        // No session bus or registration is contacted by these tests.
        let path = "/org/freedesktop/portal/desktop/request/1_42/test";
        let (tx, rx) = mpsc::sync_channel(1);
        let results: Properties = HashMap::from([(
            "shortcuts".into(),
            OwnedValue::try_from(zbus::zvariant::Value::from(vec![shortcut(
                Action::Voice.id(),
                "Actual",
            )]))
            .unwrap(),
        )]);
        tx.try_send(
            Message::signal(path, REQUEST, "Response")
                .unwrap()
                .build(&(0_u32, results))
                .unwrap(),
        )
        .unwrap();
        let returned = OwnedObjectPath::try_from(path).unwrap();
        assert_eq!(returned.as_str(), path);
        let response = rx.try_recv().unwrap();
        assert_eq!(response.body().signature().to_string_no_parens(), "ua{sv}");
        let (code, mut results): (u32, Properties) = response.body().deserialize().unwrap();
        response_status("BindShortcuts", code).unwrap();
        assert_eq!(
            select_bindings(shortcuts_result(&mut results).unwrap()).unwrap(),
            vec![(Action::Voice, "Actual".into())]
        );
        assert!(
            response_status("BindShortcuts", 1)
                .unwrap_err()
                .message()
                .contains("cancelled")
        );
        assert!(response_status("BindShortcuts", 2).is_err());
        assert!(response_status("BindShortcuts", 99).is_err());
    }

    #[test]
    fn cancellation_is_sticky_and_prevents_polling_new_work() {
        let (worker, control, _events) = control(1);
        drop(worker);
        let polled = Cell::new(false);
        assert!(matches!(
            control.wait(async { polled.set(true) }, IO_TIMEOUT, "test"),
            Err(Failure::Cancelled)
        ));
        assert!(!polled.get());
        assert!(matches!(control.check(), Err(Failure::Cancelled)));
    }

    #[test]
    fn pending_future_is_dropped_on_cancellation_and_drop_does_not_join() {
        let (worker, control, events) = control(1);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let join = thread::spawn(move || {
            struct Dropped(SyncSender<()>);
            impl Drop for Dropped {
                fn drop(&mut self) {
                    let _ = self.0.try_send(());
                }
            }
            let guard = Dropped(done_tx);
            let future = async move {
                let _guard = guard;
                started_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            };
            assert!(matches!(
                control.wait(future, IO_TIMEOUT, "pending"),
                Err(Failure::Cancelled)
            ));
            control.closed("cancelled".into());
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let before = Instant::now();
        drop(worker);
        assert!(before.elapsed() < Duration::from_millis(100));
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(2)).unwrap(),
            (7, PortalEvent::Closed(_))
        ));
        join.join().unwrap();
    }

    #[test]
    fn timeout_backpressure_and_generation_are_bounded() {
        let (worker, control, events) = control(1);
        assert!(
            control
                .wait(
                    std::future::pending::<()>(),
                    Duration::ZERO,
                    "offline timeout"
                )
                .unwrap_err()
                .message()
                .contains("timed out")
        );
        control
            .send(PortalEvent::Ready(vec![(Action::Voice, "Actual".into())]))
            .unwrap();
        assert_eq!(events.try_recv().unwrap().0, 7);
        control.send(PortalEvent::Trigger(Action::Voice)).unwrap();
        let before = Instant::now();
        assert!(
            control
                .send(PortalEvent::Trigger(Action::Stop))
                .unwrap_err()
                .message()
                .contains("queue full")
        );
        assert!(before.elapsed() < Duration::from_secs(2));
        drop(worker);
        assert!(matches!(
            control.send(PortalEvent::Trigger(Action::Voice)),
            Err(Failure::Cancelled)
        ));
    }

    #[test]
    fn uuid_tokens_are_independent_and_valid_path_components() {
        let a = token();
        let b = token();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert!(object_path(format!("/org/freedesktop/portal/desktop/request/1_42/{a}")).is_ok());
    }
}
