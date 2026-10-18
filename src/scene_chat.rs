//! Explicit group replies, serialized and isolated per recipient.
use crate::{
    data::{Library, Store, Turn, trim_history},
    memory::MemoryState,
    provider::{self, ProviderConfig, ProviderEvent},
    scene_roster::SceneRoster,
    voice::VoiceConfig,
};
use bevy::prelude::Resource;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
#[derive(Clone)]
struct Recipient {
    id: Uuid,
    name: String,
    persona: String,
    provider: ProviderConfig,
    voice: VoiceConfig,
}
#[derive(Clone, Debug)]
pub struct SceneLine {
    pub character: Option<Uuid>,
    pub name: String,
    pub text: String,
}
#[derive(Debug)]
enum Event {
    Routed(Result<Option<Uuid>, String>),
    Progress(String, String),
    Line(SceneLine),
    Speech(Uuid, VoiceConfig, String),
    Done,
    Failed(String),
}
struct RoutingSnapshot {
    draft: String,
    revision: u64,
    candidates: Vec<(Uuid, String)>,
}
#[derive(Resource)]
pub struct SceneChat {
    pub draft: String,
    pub recipients: Vec<Uuid>,
    pub transcript: VecDeque<SceneLine>,
    pub status: String,
    pub partial: String,
    pub cancellation_revision: u64,
    routing_snapshot: Option<RoutingSnapshot>,
    root: PathBuf,
    cancellation: Option<Arc<AtomicBool>>,
    events: Option<Mutex<mpsc::Receiver<Event>>>,
    active_recipients: Vec<Uuid>,
    speech: VecDeque<(Uuid, VoiceConfig, String)>,
}
fn available(library: &Library, roster: &SceneRoster, id: Uuid) -> bool {
    library.characters.iter().any(|c| c.id == id)
        && (library.selected == Some(id) || roster.contains(id))
}
/// Exact case-insensitive @names; longer names win over their prefixes.
/// Explicit checkbox recipients bypass this helper entirely.
fn local_recipients(text: &str, candidates: &[(Uuid, String)]) -> Result<Vec<Uuid>, String> {
    let lower = text.to_lowercase();
    let mut selected = Vec::new();
    for (offset, _) in lower.match_indices('@') {
        if lower[..offset]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let suffix = &lower[offset + 1..];
        let mut matches: Vec<_> = candidates
            .iter()
            .filter_map(|(id, name)| {
                let name = name.trim().to_lowercase();
                if name.is_empty() || !suffix.starts_with(&name) {
                    return None;
                }
                let boundary = suffix[name.len()..].chars().next();
                if boundary.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                    return None;
                }
                Some((*id, name.len()))
            })
            .collect();
        matches.sort_by_key(|(_, len)| std::cmp::Reverse(*len));
        let Some(&(id, length)) = matches.first() else {
            return Err("Unknown @name; select recipient checkboxes explicitly".into());
        };
        if matches.iter().filter(|(_, len)| *len == length).count() != 1 {
            return Err("Ambiguous @name; select recipient checkboxes explicitly".into());
        }
        if !selected.contains(&id) {
            selected.push(id);
        }
    }
    if !selected.is_empty() {
        return Ok(selected);
    }
    if candidates.len() == 1 {
        return Ok(vec![candidates[0].0]);
    }
    Err("Choose recipients, use exact @names, or explicitly ask Jev to choose".into())
}

impl SceneChat {
    pub fn new(root: PathBuf) -> Self {
        Self {
            draft: String::new(),
            recipients: vec![],
            transcript: VecDeque::new(),
            status: "Choose recipients for a scene message".into(),
            partial: String::new(),
            cancellation_revision: 0,
            routing_snapshot: None,
            root,
            cancellation: None,
            events: None,
            active_recipients: vec![],
            speech: VecDeque::new(),
        }
    }
    pub fn is_busy(&self) -> bool {
        self.cancellation.is_some()
    }
    pub fn cancel(&mut self) {
        self.cancellation_revision = self.cancellation_revision.wrapping_add(1);
        if let Some(cancel) = self.cancellation.take() {
            cancel.store(true, Ordering::Release);
        }
        self.events = None;
        self.routing_snapshot = None;
        self.active_recipients.clear();
        self.speech.clear();
        self.partial.clear();
        self.status = "Scene replies cancelled".into();
    }
    /// Routes only after an explicit click; never submits a message.
    pub fn route_draft(
        &mut self,
        config: &crate::recipient_decision::DecisionConfig,
        library: &Library,
        roster: &SceneRoster,
    ) -> Result<(), String> {
        if self.is_busy() || !self.recipients.is_empty() {
            return Err("Clear explicit recipients before asking the decision model".into());
        }
        if self.draft.trim().is_empty() || self.draft.len() > 32_000 {
            return Err("Enter a message of at most 32 KB".into());
        }
        let candidates: Vec<_> = library
            .characters
            .iter()
            .filter(|c| available(library, roster, c.id))
            .map(|c| crate::recipient_decision::RecipientCandidate {
                id: c.id,
                name: c.name.clone(),
            })
            .take(4)
            .collect();
        if candidates.is_empty() {
            return Err("Add a character to the scene first".into());
        }
        let snapshot = RoutingSnapshot {
            draft: self.draft.clone(),
            revision: roster.revision,
            candidates: candidates.iter().map(|c| (c.id, c.name.clone())).collect(),
        };
        let text = self.draft.clone();
        let config = config.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("scene-recipient".into())
            .spawn(move || {
                let result =
                    crate::recipient_decision::choose(&config, &text, &candidates, &|| {
                        worker_cancel.load(Ordering::Acquire)
                    });
                let _ = tx.send(Event::Routed(result));
            })
            .map_err(|_| "Could not start recipient decision worker")?;
        self.routing_snapshot = Some(snapshot);
        self.cancellation = Some(cancel);
        self.events = Some(Mutex::new(rx));
        self.status = "Choosing a scene recipient…".into();
        Ok(())
    }
    pub fn take_speech(&mut self) -> Option<(Uuid, VoiceConfig, String)> {
        self.speech.pop_front()
    }
    pub fn submit(&mut self, library: &Library, roster: &SceneRoster) -> Result<(), String> {
        if self.is_busy() {
            return Err("Scene replies are already running".into());
        }
        let text = self.draft.trim().to_owned();
        if text.is_empty() || text.len() > 32_000 {
            return Err("Scene message must contain 1–32000 bytes".into());
        }
        let requested = if self.recipients.is_empty() {
            let candidates: Vec<_> = library
                .characters
                .iter()
                .filter(|c| available(library, roster, c.id))
                .map(|c| (c.id, c.name.clone()))
                .collect();
            local_recipients(&text, &candidates)?
        } else {
            self.recipients.clone()
        };
        let mut ids = requested.clone();
        ids.sort();
        ids.dedup();
        if ids.is_empty() || ids.len() > 4 {
            return Err("Choose one to four scene recipients".into());
        }
        // Preserve the explicit UI ordering after removing duplicates.
        let mut snapshots = vec![];
        for id in &requested {
            if snapshots.iter().any(|r: &Recipient| r.id == *id) {
                continue;
            }
            if !available(library, roster, *id) {
                return Err("A recipient is no longer in the scene".into());
            }
            let c = library
                .characters
                .iter()
                .find(|c| c.id == *id)
                .ok_or("Missing character")?;
            let focused = library.selected == Some(*id);
            let index = if focused {
                library.provider
            } else {
                c.provider.unwrap_or(library.provider)
            };
            let config = library
                .providers
                .get(index)
                .ok_or("Character connection no longer exists")?
                .clone();
            config.endpoint("chat/completions")?;
            if config.model.trim().is_empty() {
                return Err("Choose a model for each recipient".into());
            }
            snapshots.push(Recipient {
                id: *id,
                name: c.name.clone(),
                persona: c.persona.clone(),
                provider: config,
                voice: if focused {
                    library.voice.clone()
                } else {
                    c.voice.clone().unwrap_or_else(|| library.voice.clone())
                },
            });
        }
        let (tx, rx) = mpsc::sync_channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let root = self.root.clone();
        let worker_text = text.clone();
        std::thread::Builder::new()
            .name("scene-chat".into())
            .spawn(move || {
                let result = run(root, snapshots, worker_text, &worker_cancel, &tx);
                if !worker_cancel.load(Ordering::Acquire) {
                    let _ = tx.send(match result {
                        Ok(()) => Event::Done,
                        Err(e) => Event::Failed(e),
                    });
                }
            })
            .map_err(|_| "Could not start scene chat worker")?;
        self.cancellation = Some(cancel);
        self.events = Some(Mutex::new(rx));
        self.active_recipients = ids;
        self.transcript.push_back(SceneLine {
            character: None,
            name: "You".into(),
            text,
        });
        while self.transcript.len() > 80 {
            self.transcript.pop_front();
        }
        self.draft.clear();
        self.status = "Waiting for scene replies…".into();
        Ok(())
    }
    pub fn poll(&mut self, library: &Library, roster: &SceneRoster) {
        if self
            .active_recipients
            .iter()
            .any(|id| !available(library, roster, *id))
        {
            self.cancel();
            self.status = "Scene changed; pending replies cancelled".into();
            return;
        }
        let events: Vec<_> = self
            .events
            .as_ref()
            .and_then(|rx| rx.lock().ok())
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Routed(result) => {
                    self.cancellation = None;
                    self.events = None;
                    let current: Vec<_> = library
                        .characters
                        .iter()
                        .filter(|c| available(library, roster, c.id))
                        .take(4)
                        .map(|c| (c.id, c.name.clone()))
                        .collect();
                    let fresh = self.routing_snapshot.take().is_some_and(|snapshot| {
                        snapshot.draft == self.draft
                            && snapshot.revision == roster.revision
                            && snapshot.candidates == current
                    }) && self.recipients.is_empty();
                    if !fresh {
                        self.status =
                            "Draft or scene changed; recipient suggestion discarded".into();
                    } else {
                        match result {
                            Ok(Some(id)) if available(library, roster, id) => {
                                self.recipients.push(id);
                                self.status = "Recipient selected. Review and press Send.".into();
                            }
                            Ok(_) => {
                                self.status =
                                    "No recipient selected; choose recipients manually".into()
                            }
                            Err(error) => self.status = error,
                        }
                    }
                }

                Event::Progress(name, text) => {
                    self.status = format!("{name} is replying…");
                    self.partial = text;
                }
                Event::Line(line) => {
                    self.partial.clear();
                    self.status = format!("{} replied", line.name);
                    self.transcript.push_back(line);
                }
                Event::Speech(id, voice, text) => self.speech.push_back((id, voice, text)),
                Event::Done => {
                    self.cancellation = None;
                    self.events = None;
                    self.active_recipients.clear();
                    self.status = "Scene replies finished".into();
                }
                Event::Failed(e) => {
                    self.cancellation = None;
                    self.events = None;
                    self.active_recipients.clear();
                    self.status = e;
                }
            }
        }
        while self.transcript.len() > 80 {
            self.transcript.pop_front();
        }
        while self.speech.len() > 4 {
            self.speech.pop_front();
        }
    }
}
impl Drop for SceneChat {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn request_context(
    persona: &str,
    history: &[Turn],
    text: &str,
    notes: Option<String>,
    evidence: Option<String>,
) -> Vec<Turn> {
    let mut context = vec![Turn {
        role: "system".into(),
        content: persona.into(),
    }];
    if let Some(content) = notes {
        context.push(Turn {
            role: "user".into(),
            content,
        });
    }
    if let Some(content) = evidence {
        context.push(Turn {
            role: "user".into(),
            content,
        });
    }
    context.extend(history.iter().rev().take(38).rev().cloned());
    context.push(Turn {
        role: "user".into(),
        content: text.into(),
    });
    context
}
fn run(
    root: PathBuf,
    recipients: Vec<Recipient>,
    text: String,
    cancel: &AtomicBool,
    tx: &mpsc::SyncSender<Event>,
) -> Result<(), String> {
    let store = Store {
        root: root.join("scene-chat"),
    };
    std::fs::create_dir_all(store.root.join("conversations"))
        .map_err(|_| "Cannot create scene conversation storage")?;
    let mut memory = MemoryState::load(root.join("memory"));
    for recipient in recipients {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut history = store.history(recipient.id)?;
        memory.select_character(Some(recipient.id));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !memory.loaded {
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            memory.poll();
            if memory.error.is_some() || Instant::now() > deadline {
                return Err("Cannot load recipient notes; scene request stopped".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let evidence =
            crate::facts_memory::context_from_disk(&root.join("facts-memory"), recipient.id, &text)
                .map_err(|_| "Cannot load recipient evidence ledger; scene request stopped")?;
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let context = request_context(
            &recipient.persona,
            &history,
            &text,
            memory.context(&text),
            evidence,
        );
        let (provider_tx, provider_rx) = mpsc::channel();
        let job = provider::spawn_chat(recipient.provider, context, provider_tx);
        let mut reply = String::new();
        let mut last_progress = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if cancel.load(Ordering::Acquire) {
                job.cancel();
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("Scene reply timed out".into());
            }
            match provider_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(ProviderEvent::Delta(id, delta)) if id == job.id => {
                    if reply.len() + delta.len() > 64_000 {
                        return Err("Scene reply exceeded 64 KB".into());
                    }
                    reply.push_str(&delta);
                    if last_progress.elapsed() >= Duration::from_millis(50) {
                        let _ = tx.try_send(Event::Progress(recipient.name.clone(), reply.clone()));
                        last_progress = Instant::now();
                    }
                }
                Ok(ProviderEvent::Finished(id)) if id == job.id => break,
                Ok(ProviderEvent::Failed(id, error)) if id == job.id => return Err(error),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Scene provider disconnected".into());
                }
                _ => {}
            }
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        history.push(Turn {
            role: "user".into(),
            content: text.clone(),
        });
        history.push(Turn {
            role: "assistant".into(),
            content: reply.clone(),
        });
        trim_history(&mut history);
        store.save_history(recipient.id, &history)?;
        tx.send(Event::Line(SceneLine {
            character: Some(recipient.id),
            name: recipient.name,
            text: reply.clone(),
        }))
        .map_err(|_| "Scene closed")?;
        if recipient.voice.enabled {
            tx.send(Event::Speech(recipient.id, recipient.voice, reply))
                .map_err(|_| "Scene closed")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_scene_worker_streams_isolated_recipients_over_local_http() {
        use std::io::{Read, Write};
        let root = std::env::temp_dir().join(format!("scene-worker-{}", Uuid::new_v4()));
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut memory = MemoryState::load(root.join("memory"));
        let settle = |memory: &mut MemoryState| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                memory.poll();
                if memory.loaded && !memory.busy {
                    break;
                }
                assert!(Instant::now() < deadline, "memory fixture timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        for (id, note) in [(a, "PRIVATE_ALICE_NOTE"), (b, "PRIVATE_BOB_NOTE")] {
            memory.select_character(Some(id));
            settle(&mut memory);
            memory.set_enabled(true).unwrap();
            settle(&mut memory);
            memory.add_note(note.into()).unwrap();
            settle(&mut memory);
            memory.set_pinned(memory.notes()[0].id, true).unwrap();
            settle(&mut memory);
        }
        drop(memory);
        let mut facts = crate::facts_memory::FactsMemoryState::load(root.join("facts-memory"));
        let settle_facts = |facts: &mut crate::facts_memory::FactsMemoryState| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                facts.poll();
                if facts.loaded && !facts.busy {
                    break;
                }
                assert!(Instant::now() < deadline, "ledger fixture timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        for (id, evidence) in [(a, "PRIVATE_ALICE_EVIDENCE"), (b, "PRIVATE_BOB_EVIDENCE")] {
            facts.select_character(Some(id));
            settle_facts(&mut facts);
            facts.set_enabled(true).unwrap();
            settle_facts(&mut facts);
            facts
                .add_record(
                    crate::facts_memory::RecordKind::Fact,
                    format!("reply {evidence}"),
                    evidence.into(),
                )
                .unwrap();
            settle_facts(&mut facts);
        }
        drop(facts);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = vec![];
            let deadline = Instant::now() + Duration::from_secs(10);
            for response in ["Alice streamed reply", "Bob streamed reply"] {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "scene request never arrived");
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut data = vec![];
                let mut chunk = [0u8; 4096];
                let (header_end, content_length) = loop {
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&chunk[..n]);
                    if let Some(at) = data.windows(4).position(|w| w == [13, 10, 13, 10]) {
                        let header = String::from_utf8_lossy(&data[..at]);
                        assert!(header.starts_with("POST /v1/chat/completions "));
                        let len = header
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (at + 4, len);
                    }
                    assert!(data.len() < 128_000);
                };
                while data.len() < header_end + content_length {
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&chunk[..n]);
                }
                let request: serde_json::Value =
                    serde_json::from_slice(&data[header_end..header_end + content_length]).unwrap();
                assert_eq!(request["stream"], true);
                requests.push(request);
                let crlf = String::from_utf8(vec![13, 10]).unwrap();
                let headers = [
                    "HTTP/1.1 200 OK",
                    "Content-Type: text/event-stream",
                    "Connection: close",
                    "",
                    "",
                ]
                .join(&crlf);
                stream.write_all(headers.as_bytes()).unwrap();
                for word in response.split_inclusive(' ') {
                    let frame = serde_json::json!({"choices":[{"delta":{"content":word}}]});
                    write!(
                        stream,
                        "data: {frame}

"
                    )
                    .unwrap();
                    stream.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(30));
                }
                stream
                    .write_all(
                        b"data: [DONE]

",
                    )
                    .unwrap();
            }
            requests
        });
        let mut library = Library {
            selected: Some(a),
            providers: vec![ProviderConfig {
                base_url: url,
                model: "fake-scene-model".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        library.voice.enabled = true;
        for (id, name) in [(a, "Alice"), (b, "Bob")] {
            library.characters.push(serde_json::from_value(serde_json::json!({
                "id":id, "name":name, "model":"fixture.vrm", "persona":format!("{name} private persona"), "format":"vrm"
            })).unwrap());
        }
        let mut roster = SceneRoster::default();
        roster.add(b, "Bob".into(), "fixture.vrm".into()).unwrap();
        let mut chat = SceneChat::new(root.clone());
        chat.recipients = vec![a, b];
        chat.draft = "Please reply".into();
        chat.submit(&library, &roster).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_partial = false;
        while chat.is_busy() {
            chat.poll(&library, &roster);
            saw_partial |= !chat.partial.is_empty();
            assert!(
                Instant::now() < deadline,
                "scene worker timed out: {}",
                chat.status
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(chat.status, "Scene replies finished");
        assert!(
            saw_partial,
            "actual SSE deltas should reach partial UI state"
        );
        let requests = server.join().unwrap();
        let alice = requests[0].to_string();
        let bob = requests[1].to_string();
        assert!(alice.contains("PRIVATE_ALICE_NOTE"));
        assert!(!alice.contains("PRIVATE_BOB_NOTE"));
        assert!(bob.contains("PRIVATE_BOB_NOTE"));
        assert!(!bob.contains("PRIVATE_ALICE_NOTE"));
        assert!(alice.contains("PRIVATE_ALICE_EVIDENCE"));
        assert!(!alice.contains("PRIVATE_BOB_EVIDENCE"));
        assert!(bob.contains("PRIVATE_BOB_EVIDENCE"));
        assert!(!bob.contains("PRIVATE_ALICE_EVIDENCE"));
        for request in &requests {
            let messages = request["messages"].as_array().unwrap();
            let ledger_turns: Vec<_> = messages
                .iter()
                .filter(|turn| {
                    turn["content"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("Untrusted local memory evidence"))
                })
                .collect();
            assert_eq!(ledger_turns.len(), 1);
            assert_eq!(ledger_turns[0]["role"], "user");
        }
        assert!(chat.draft.is_empty());
        assert!(!bob.contains("Alice streamed reply"));
        assert_eq!(chat.transcript[1].character, Some(a));
        assert_eq!(chat.transcript[2].character, Some(b));
        assert_eq!(chat.take_speech().unwrap().0, a);
        assert_eq!(chat.take_speech().unwrap().0, b);
        let store = Store {
            root: root.join("scene-chat"),
        };
        assert_eq!(store.history(a).unwrap()[1].content, "Alice streamed reply");
        assert_eq!(store.history(b).unwrap()[1].content, "Bob streamed reply");
        assert!(
            !store
                .history(a)
                .unwrap()
                .iter()
                .any(|t| t.content.contains("PRIVATE_"))
        );
        assert!(
            !store
                .history(b)
                .unwrap()
                .iter()
                .any(|t| t.content.contains("PRIVATE_"))
        );
        assert!(!chat.transcript.iter().any(|t| t.text.contains("PRIVATE_")));
        assert!(
            !root.join("conversations").exists(),
            "one-to-one store must remain untouched"
        );
        drop(chat);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_mentions_are_exact_unique_and_preserve_order() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let candidates = vec![
            (a, "Ann".into()),
            (b, "Anna".into()),
            (c, "Ann Smith".into()),
        ];
        assert_eq!(
            local_recipients("@ANNA, then @Ann Smith and @Ann!", &candidates).unwrap(),
            vec![b, c, a]
        );
        assert!(local_recipients("@Annabelle", &candidates).is_err());
        assert!(local_recipients("hello", &candidates).is_err());
        assert!(local_recipients("@Ann", &[(a, "Ann".into()), (b, "ann".into())]).is_err());
        assert_eq!(
            local_recipients("hello", &[(a, "Ann".into())]).unwrap(),
            vec![a]
        );
    }

    #[test]
    fn recipient_histories_and_ephemeral_notes_are_isolated() {
        let root = std::env::temp_dir().join(format!("scene-chat-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("conversations")).unwrap();
        let store = Store { root: root.clone() };
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        store
            .save_history(
                a,
                &[Turn {
                    role: "user".into(),
                    content: "Alice-only history".into(),
                }],
            )
            .unwrap();
        store
            .save_history(
                b,
                &[Turn {
                    role: "user".into(),
                    content: "Bob-only history".into(),
                }],
            )
            .unwrap();
        let context = request_context(
            "Bob persona",
            &store.history(b).unwrap(),
            "shared question",
            Some("Bob note".into()),
            Some("Bob evidence".into()),
        );
        assert!(!context.iter().any(|t| t.content.contains("Alice")));
        assert_eq!(context[0].content, "Bob persona");
        assert_eq!(context[1].content, "Bob note");
        assert_eq!(context.last().unwrap().content, "shared question");
        assert_eq!(
            store.history(b).unwrap().len(),
            1,
            "context is never persisted"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancellation_discards_old_receiver_and_pending_speech() {
        let mut state = SceneChat::new(PathBuf::new());
        let (tx, rx) = mpsc::sync_channel(4);
        let cancel = Arc::new(AtomicBool::new(false));
        state.cancellation = Some(cancel.clone());
        state.events = Some(Mutex::new(rx));
        tx.send(Event::Line(SceneLine {
            character: None,
            name: "stale".into(),
            text: "stale".into(),
        }))
        .unwrap();
        state.cancel();
        state.poll(&Library::default(), &SceneRoster::default());
        assert!(cancel.load(Ordering::Acquire));
        assert!(state.transcript.is_empty());
        assert!(state.take_speech().is_none());
        assert!(!state.is_busy());
    }
    #[test]
    fn edited_draft_discards_recipient_suggestion() {
        let mut state = SceneChat::new(PathBuf::new());
        let (tx, rx) = mpsc::sync_channel(1);
        state.events = Some(Mutex::new(rx));
        state.routing_snapshot = Some(RoutingSnapshot {
            draft: "old draft".into(),
            revision: 0,
            candidates: vec![],
        });
        state.draft = "new draft".into();
        tx.send(Event::Routed(Ok(Some(Uuid::new_v4())))).unwrap();
        state.poll(&Library::default(), &SceneRoster::default());
        assert!(state.recipients.is_empty());
        assert!(state.status.contains("discarded"));
        assert_eq!(state.draft, "new draft");
    }

    #[test]
    fn ordered_events_preserve_reply_and_speech_order() {
        let mut state = SceneChat::new(PathBuf::new());
        let (tx, rx) = mpsc::sync_channel(8);
        state.events = Some(Mutex::new(rx));
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        for (id, name) in [(a, "A"), (b, "B")] {
            tx.send(Event::Line(SceneLine {
                character: Some(id),
                name: name.into(),
                text: name.into(),
            }))
            .unwrap();
            tx.send(Event::Speech(id, VoiceConfig::default(), name.into()))
                .unwrap();
        }
        tx.send(Event::Done).unwrap();
        state.poll(&Library::default(), &SceneRoster::default());
        assert_eq!(state.transcript[0].character, Some(a));
        assert_eq!(state.transcript[1].character, Some(b));
        assert_eq!(state.take_speech().unwrap().0, a);
        assert_eq!(state.take_speech().unwrap().0, b);
    }
}
