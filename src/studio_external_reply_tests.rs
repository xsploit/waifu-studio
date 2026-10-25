// Included inside studio::tests; all controller names/helpers come from that scope.
// Production admits only two provider workers globally. Keep these transport
// fixtures sequential; other transport test modules need --test-threads=1.
static EXTERNAL_REPLY_TRANSPORT: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct ExternalReplyDirectory(PathBuf);
impl Drop for ExternalReplyDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ready_external_reply(
    studio: &mut Studio,
    directory: &std::path::Path,
) -> waifu_studio::memory::MemoryState {
    let mut memory = waifu_studio::memory::MemoryState::load(directory.join("notes"));
    let mut semantic =
        waifu_studio::semantic_memory::SemanticMemory::load(directory.join("semantic"));
    memory.select_character(studio.library.selected);
    settle_input_fixture(studio, &mut memory, &mut semantic);
    // Never inherit a pinned remote route/session from the controller fixture,
    // including negative tests which mutate model/endpoint before admission.
    studio.library.providers[studio.library.provider] = waifu_studio::provider::ProviderConfig {
        base_url: "://invalid".into(),
        ..Default::default()
    };
    studio.library.voice.enabled = false;
    studio.library.scene.automatic_reflection.enabled = false;
    memory
}

// Real production provider transport, bounded even when admission unexpectedly fails.
fn external_reply_server() -> (String, std::thread::JoinHandle<serde_json::Value>) {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) => panic!("loopback accept: {error}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "loopback accept deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap()).take(80_000);
        let mut length = None;
        let mut header_bytes = 0;
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            header_bytes += line.len();
            assert!(header_bytes <= 16_000);
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let length = length.unwrap();
        assert!(length <= 64_000);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let body = serde_json::from_slice(&body).unwrap();
        let response = "data: {\"choices\":[{\"delta\":{\"content\":\"External reply fixture.\"}}]}\n\ndata: [DONE]\n\n";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        body
    });
    (url, worker)
}

#[test]
fn external_reply_production_request_preserves_attribution_draft_and_lifecycle() {
    let _transport = EXTERNAL_REPLY_TRANSPORT.lock().unwrap();
    let (mut studio, directory) = studio();
    let _cleanup = ExternalReplyDirectory(directory.clone());
    let mut memory = ready_external_reply(&mut studio, &directory);
    let (target, origin) = external_request(&studio);
    let (url, server) = external_reply_server();
    studio.library.providers[studio.library.provider] = waifu_studio::provider::ProviderConfig {
        base_url: url,
        model: "offline-external-reply".into(),
        key_env: "WAIFU_STUDIO_EXTERNAL_REPLY_TEST_UNUSED".into(),
        ..Default::default()
    };
    studio.library.scene.reply_to_external_messages = true;
    studio.library.scene.affect.enabled = true;
    studio.library.scene.affect.source = waifu_studio::reply_affect::AffectSource::UserMessageJev;
    studio.library.scene.memory_triage.enabled = true;
    studio.facts.enabled = true;
    studio.facts.candidates_enabled = true;
    studio.facts.triage_status = "unchanged external triage".into();
    studio.composer = "PRIVATE LOCAL DRAFT".into();
    studio
        .attachments
        .push(waifu_studio::attachments::ImageAttachment {
            name: "draft.png".into(),
            data_url: "data:image/png;base64,PRIVATE_ATTACHMENT".into(),
        });
    let facts_generation = studio.facts.generation();
    let text = "I like jasmine tea. Remember that I live in a lighthouse.";
    let message_id = studio.deliver_external(target, &origin, text).unwrap();
    let job = studio
        .respond_to_external(target, message_id, &mut memory)
        .unwrap();
    assert!(!job.is_nil());
    assert_ne!(job, message_id);
    assert_eq!(studio.job.as_ref().unwrap().id, job);
    assert_eq!(studio.speech_reply_id, Some(job));
    assert_eq!(studio.chat_character, Some(target.uuid));
    assert!(studio.appraisal.is_none());
    assert!(memory.notes().is_empty());
    assert_eq!(studio.turns.len(), 1);
    assert_eq!(studio.composer, "PRIVATE LOCAL DRAFT");
    assert_eq!(studio.attachments.len(), 1);
    assert_eq!(
        studio.attachments[0].data_url,
        "data:image/png;base64,PRIVATE_ATTACHMENT"
    );
    let body = server.join().unwrap();
    let messages = body["messages"].as_array().unwrap();
    let users: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .collect();
    assert_eq!(users.len(), 1);
    let attributed: serde_json::Value =
        serde_json::from_str(users[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(attributed["kind"], "external_message");
    assert_eq!(attributed["message_id"], message_id.to_string());
    assert_eq!(attributed["origin"], serde_json::to_value(&origin).unwrap());
    assert_eq!(attributed["text"], text);
    assert!(!body.to_string().contains("PRIVATE_"));
    assert!(!body.to_string().contains("PRIVATE LOCAL"));
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(studio)
        .init_resource::<waifu_studio::animation::AnimationState>()
        .add_message::<LoadAvatar>()
        .add_message::<UnloadAvatar>()
        .add_systems(Update, poll_studio);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.update();
        if app.world().resource::<Studio>().job.is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    {
        let mut studio = app.world_mut().resource_mut::<Studio>();
        assert_eq!(studio.turns.len(), 2);
        assert_eq!(studio.turns[1].role, "assistant");
        assert_eq!(studio.turns[1].content, "External reply fixture.");
        assert_eq!(studio.store.history(target.uuid).unwrap(), studio.turns);
        assert_eq!(studio.facts.generation(), facts_generation);
        assert!(studio.facts.candidates().is_empty() && studio.facts.records().is_empty());
        assert_eq!(studio.facts.triage_status, "unchanged external triage");
        assert!(studio.lifecycle.drain().into_iter().any(|event| matches!(event.event,
        LifecycleEvent::ReplyFinished { reply_id, character, outcome: ReplyOutcome::Completed { .. } }
        if reply_id == job && character == Some(target.uuid))));
    }
    drop(app);
    drop(memory);
}

#[test]
fn external_reply_guards_preserve_saved_delivery_and_never_charge_cooldown() {
    let (mut studio, directory) = studio();
    let _cleanup = ExternalReplyDirectory(directory.clone());
    let mut memory = ready_external_reply(&mut studio, &directory);
    let (target, origin) = external_request(&studio);
    studio.composer = "unsent draft".into();
    let first = studio
        .deliver_external(target, &origin, "First incoming message")
        .unwrap();
    assert!(
        studio
            .respond_to_external(target, first, &mut memory)
            .is_err()
    );
    studio.library.scene.reply_to_external_messages = true;
    let latest = studio
        .deliver_external(target, &origin, "Latest incoming message")
        .unwrap();
    for id in [first, Uuid::new_v4()] {
        assert!(studio.respond_to_external(target, id, &mut memory).is_err());
    }
    let mut invalid = target;
    invalid.uuid = Uuid::nil();
    assert!(
        studio
            .respond_to_external(invalid, latest, &mut memory)
            .is_err()
    );
    studio.chat_character = Some(Uuid::new_v4());
    assert!(
        studio
            .respond_to_external(target, latest, &mut memory)
            .is_err()
    );
    studio.chat_character = Some(target.uuid);
    let saved = studio.store.history(target.uuid).unwrap();
    // Synchronous provider validation must not start transport or charge cooldown.
    studio.library.providers[studio.library.provider]
        .model
        .clear();
    assert!(
        studio
            .respond_to_external(target, latest, &mut memory)
            .is_err()
    );
    assert!(studio.job.is_none() && studio.appraisal.is_none());
    assert!(studio.last_external_reply_started.is_none());
    assert_eq!(studio.store.history(target.uuid).unwrap(), saved);
    assert_eq!(studio.turns, saved);
    studio.library.providers[studio.library.provider] = waifu_studio::provider::ProviderConfig {
        base_url: "://invalid".into(),
        model: "offline-invalid-endpoint".into(),
        ..Default::default()
    };
    assert!(
        studio
            .respond_to_external(target, latest, &mut memory)
            .is_err()
    );
    assert!(studio.job.is_none() && studio.last_external_reply_started.is_none());
    assert_eq!(studio.store.history(target.uuid).unwrap(), saved);
    // A different committed snapshot must never be overwritten by stale UI state.
    let mut committed = saved.clone();
    committed.push(
        Turn::external(
            "Committed elsewhere",
            ExternalMessage {
                message_id: Uuid::new_v4(),
                origin,
            },
        )
        .unwrap(),
    );
    studio.store.save_history(target.uuid, &committed).unwrap();
    assert!(
        studio
            .respond_to_external(target, latest, &mut memory)
            .is_err()
    );
    assert!(studio.external_history_unconfirmed);
    assert!(studio.job.is_none());
    assert_eq!(studio.store.history(target.uuid).unwrap(), committed);
    assert_eq!(studio.turns, saved);
    assert_eq!(studio.composer, "unsent draft");
    drop(studio);
    drop(memory);
}

#[test]
fn external_reply_cooldown_survives_cancel_and_expires_without_sleep() {
    let _transport = EXTERNAL_REPLY_TRANSPORT.lock().unwrap();
    let (mut studio, directory) = studio();
    let _cleanup = ExternalReplyDirectory(directory.clone());
    let mut memory = ready_external_reply(&mut studio, &directory);
    let (target, origin) = external_request(&studio);
    studio.library.scene.reply_to_external_messages = true;
    for attempt in 0..2 {
        let (url, server) = external_reply_server();
        studio.library.providers[studio.library.provider] =
            waifu_studio::provider::ProviderConfig {
                base_url: url,
                model: "offline-cooldown".into(),
                key_env: "WAIFU_STUDIO_EXTERNAL_REPLY_TEST_UNUSED".into(),
                ..Default::default()
            };
        let id = studio
            .deliver_external(target, &origin, "Another incoming message")
            .unwrap();
        let job = studio.respond_to_external(target, id, &mut memory).unwrap();
        // Consume the real terminal event before moving to another transport.
        // A worker-limit/error failure is reported directly rather than hidden
        // behind the server's accept deadline. Do not retry admission.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let event = studio
                .provider_rx
                .lock()
                .unwrap()
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("cooldown fixture provider terminal deadline");
            match event {
                ProviderEvent::Finished(id) if id == job => break,
                ProviderEvent::Failed(id, error) if id == job => {
                    panic!("cooldown fixture failed: {error}")
                }
                _ => (),
            }
        }
        server.join().unwrap();
        studio.cancel_chat();
        let next = studio
            .deliver_external(target, &origin, "Saved during cooldown")
            .unwrap();
        let saved = studio.store.history(target.uuid).unwrap();
        assert!(
            studio
                .respond_to_external(target, next, &mut memory)
                .unwrap_err()
                .contains("cooling down")
        );
        assert!(studio.job.is_none());
        assert_eq!(studio.store.history(target.uuid).unwrap(), saved);
        assert_eq!(
            saved.last().unwrap().external.as_ref().unwrap().message_id,
            next
        );
        if attempt == 0 {
            // A recently admitted reply owned by a different character still
            // consumes the runtime-wide cooldown (switching A/B/A cannot evade it).
            let started = studio.last_external_reply_started.unwrap().1;
            studio.last_external_reply_started = Some((Uuid::new_v4(), started));
            assert!(
                studio
                    .respond_to_external(target, next, &mut memory)
                    .unwrap_err()
                    .contains("cooling down")
            );
            assert!(studio.job.is_none());
            assert_eq!(studio.store.history(target.uuid).unwrap(), saved);
            studio.last_external_reply_started = Some((
                target.uuid,
                std::time::Instant::now() - std::time::Duration::from_secs(11),
            ));
        }
    }
    drop(studio);
    drop(memory);
}

#[test]
fn capture_discovery_guard_skips_automatic_and_explicit_provider_lookup() {
    let (mut studio, directory) = studio();
    let _cleanup = ExternalReplyDirectory(directory);
    studio.model_discovery_enabled = false;
    let generation = studio.model_generation;
    studio.ensure_models();
    studio.refresh_models();
    assert!(studio.model_discovery.is_empty());
    assert_eq!(studio.model_generation, generation);
    assert!(studio.provider_rx.lock().unwrap().try_recv().is_err());
}
