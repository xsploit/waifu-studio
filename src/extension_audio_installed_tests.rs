//! Installed Python -> supervised JSONL -> real ASR adapter -> app queue -> disk.
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    time::{Duration, Instant},
};
use waifu_studio::{
    extension_audio::AudioRoute,
    extension_installed::InstalledRuntime,
    extension_protocol::Capability,
    extension_store::ExtensionStore,
    extensions::PackView,
    listen::{ListeningState, SttBackend},
};

struct Fixture {
    root: PathBuf,
    runtime: ExtensionRuntime,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.runtime.shutdown(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn installed_audio_reaches_external_history_without_local_draft_or_memory_changes() {
    let _guard = audio_intake::AUDIO_TEST_LOCK.lock().unwrap();
    let root = std::env::temp_dir().join(format!("waifu-installed-audio-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let fixture = Fixture {
        root: root.clone(),
        runtime: ExtensionRuntime::default(),
    };
    let character = Uuid::new_v4();
    let target = Target {
        uuid: character,
        generation: 1,
    };
    let (store, mut library) = waifu_studio::data::Store::open(root.join("profile")).unwrap();
    library.characters.push(serde_json::from_value(serde_json::json!({"id":character,"name":"Audio fixture","model":"fixture.vrm","persona":"Offline fixture","format":"VRM 1.0"})).unwrap());
    library.selected = Some(character);
    library.scene.reply_to_external_messages = false;
    library.scene.select_incoming_messages = true;
    library.scene.memory_triage.enabled = true;
    library.voice.enabled = false;
    library.decision.endpoint = "://disabled".into();
    library.decision.model.clear();
    for provider in &mut library.providers {
        provider.base_url = "://disabled".into();
        provider.model.clear();
    }
    let mut studio = Studio::new(store, library);
    studio.model_discovery_enabled = false;
    studio.composer = "Preserve local draft".into();
    studio.facts.enabled = true;
    studio.facts.candidates_enabled = true;
    let mut memory = MemoryState::load(root.join("notes"));
    memory.enabled = true;
    memory.select_character(Some(character));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        studio.facts.poll();
        memory.poll();
        if studio.facts.loaded && !studio.facts.busy && memory.loaded && !memory.busy {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let generation = studio.facts.generation();
    studio.facts.triage_status = "fixture idle".into();

    let package = root.join("package");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(package.join("extension.json"), serde_json::to_vec(&serde_json::json!({"api_version":1,"id":"fixture.installed-audio","version":"1.0.0","name":"Installed audio fixture","description":"Owned synthetic audio fixture","provenance":{"author":"Waifu Studio","license":"MIT"},"process":{"program":"run.py"},"capabilities":["lifecycle.observe","message.deliver","audio.transcribe"]})).unwrap()).unwrap();
    let script=r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
h=json.loads(sys.stdin.readline())
target=TARGET_JSON
sequence=0
results=[]
def send(payload, duplicate=False):
    global sequence
    sequence+=1
    request={'api_version':1,'type':'action','id':str(sequence),'sequence':sequence,'session_id':h['session_id'],'process_generation':h['process_generation'],'target':target,'action':{'type':'audio.input','input':payload}}
    for _ in range(2 if duplicate else 1):
        print(json.dumps(request),flush=True)
        for line in sys.stdin:
            message=json.loads(line)['message']
            if message['type']=='result' and message['id']==request['id']:
                assert message['result']['status']=='ok',message
                results.append(message['result']['status'])
                break
        else: raise RuntimeError('host stopped before receipt')
send({'phase':'start','turn_id':'audio-turn','participant_id':'speaker-42','participant_name':'Audio speaker'})
for frame in range(4):
    send({'phase':'frame','turn_id':'audio-turn','frame_sequence':frame,'pcm':[1]*320},frame==0)
send({'phase':'finish','turn_id':'audio-turn'})
Path(os.environ['WAIFU_EXTENSION_DATA'],'receipts.json').write_text(json.dumps(results))
for line in sys.stdin:
    message=json.loads(line)['message']
    if message['type']=='event' and message['event']['type']=='audio.input.finished':
        Path(os.environ['WAIFU_EXTENSION_DATA'],'feedback.json').write_text(json.dumps(message['event']))
"#.replace("TARGET_JSON",&serde_json::to_string(&target).unwrap());
    std::fs::write(package.join("run.py"), script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        package.join("run.py"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let mut extensions = ExtensionStore::open(root.join("extensions")).unwrap();
    let id = extensions.install_integration(&package).unwrap();
    assert!(!extensions.packs()[0].enabled && extensions.packs()[0].grants.is_empty());
    extensions
        .set_grants(
            id,
            vec![Capability::MessageDeliver, Capability::AudioTranscribe],
        )
        .unwrap();
    extensions.set_enabled(id, true).unwrap();
    let installed = extensions.packs()[0].clone();
    let data = extensions.data_directory(&installed).unwrap();
    let view = PackView {
        content: Some(extensions.resolved(&installed).unwrap()),
        installed,
        data_directory: Some(data.clone()),
        credentials: Default::default(),
        error: None,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut listening = ListeningState::default();
    listening.config.backend = SttBackend::OpenAiCompatible;
    listening.config.endpoint = format!("http://{}/transcriptions", listener.local_addr().unwrap());
    listening.config.model = "fixture-asr".into();
    listening.config.api_key_env.clear();
    listening.session_key = "synthetic-only".into();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buf[..n]);
            assert!(request.len() < 16384);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                let size: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .map(|s| s.trim().parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + size {
                    break;
                }
            }
        }
        let start = request.windows(4).position(|w| w == b"RIFF").unwrap();
        assert_eq!(&request[start + 8..start + 12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(request[start + 40..start + 44].try_into().unwrap()),
            2560
        );
        let body = br#"{"text":"Exact installed participant speech"}"#;
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
        stream.write_all(body).unwrap();
    });
    let mut binder = InstalledRuntime::default();
    let mut incoming = IncomingChat::default();
    let mut picker = ChatPickerState::default();
    let mut audio = AudioIntake::default();
    let mut animation = AnimationState::default();
    let mut behavior = waifu_studio::behavior::BehaviorController::default();
    let revision = Uuid::new_v4();
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut queued = false;
    let mut stale_receipts_rejected = false;
    loop {
        binder.reconcile(std::slice::from_ref(&view), &fixture.runtime);
        assert!(binder.errors.is_empty());
        let identity = fixture.runtime.audio_source_identity(id);
        if !stale_receipts_rejected && let Some((session, process_generation, origin)) = &identity {
            let route = AudioRoute {
                session: *session,
                process_generation: *process_generation,
                origin: origin.clone(),
                target,
                revision,
            };
            use waifu_studio::extension_protocol::AudioInputOutcome;
            let mut stale = route.clone();
            stale.session = Uuid::new_v4();
            assert!(!fixture.runtime.publish_audio_completion(
                &stale,
                "stale".into(),
                AudioInputOutcome::Cancelled
            ));
            stale = route.clone();
            stale.process_generation += 1;
            assert!(!fixture.runtime.publish_audio_completion(
                &stale,
                "stale".into(),
                AudioInputOutcome::Cancelled
            ));
            stale = route;
            stale.origin.integration_id = "different.source".into();
            assert!(!fixture.runtime.publish_audio_completion(
                &stale,
                "stale".into(),
                AudioInputOutcome::Cancelled
            ));
            stale_receipts_rejected = true;
        }
        let allowed: BTreeMap<String, Uuid> = if identity.is_some() {
            [(view.installed.package_id.clone(), revision)].into()
        } else {
            BTreeMap::new()
        };
        let routes = identity
            .map(|(session, process_generation, origin)| {
                vec![AudioRoute {
                    session,
                    process_generation,
                    origin,
                    target,
                    revision,
                }]
            })
            .unwrap_or_default();
        let now = Instant::now();
        audio.reconcile(routes, &listening.config, &listening.session_key, now);
        incoming.reconcile(Some(target), &allowed, now);
        {
            let mut sink = StudioExternalMessages {
                outputs: None,
                audio: Some((&mut audio, &listening)),
                studio: &mut studio,
                memory: &mut memory,
                scene_busy: false,
                target: Some(target),
                incoming: &mut incoming,
                picker: &mut picker,
                allowed: &allowed,
            };
            sink.flush_audio(&fixture.runtime);
            sink.flush_chat(now);
            fixture.runtime.frame(
                &[],
                Some(target),
                false,
                &ActionCatalog::from_packs([], &[]),
                ActionControllers {
                    animation: &mut animation,
                    behavior: &mut behavior,
                    external_messages: Some(&mut sink),
                },
            );
        }
        queued |= incoming.has_pending() && studio.turns.is_empty();
        if !studio.turns.is_empty()
            && data.join("receipts.json").exists()
            && data.join("feedback.json").exists()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "installed audio did not reach application history"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    server.join().unwrap();
    assert!(stale_receipts_rejected);
    assert!(queued);
    assert_eq!(studio.turns.len(), 1);
    let turn = &studio.turns[0];
    assert_eq!(turn.role, "external");
    assert_eq!(
        turn.external.as_ref().unwrap().origin.integration_id,
        "fixture.installed-audio"
    );
    let chunk: serde_json::Value = serde_json::from_str(&turn.content).unwrap();
    let originals: Vec<ChatMessage> = serde_json::from_value(chunk["messages"].clone()).unwrap();
    assert_eq!(originals.len(), 1);
    assert_eq!(originals[0].author_id, "speaker-42");
    assert_eq!(originals[0].author_name, "Audio speaker");
    assert_eq!(originals[0].text, "Exact installed participant speech");
    assert_eq!(studio.store.history(character).unwrap(), studio.turns);
    assert_eq!(studio.composer, "Preserve local draft");
    studio.facts.poll();
    memory.poll();
    assert_eq!(studio.facts.generation(), generation);
    assert_eq!(studio.facts.triage_status, "fixture idle");
    assert!(!studio.facts.busy && !memory.busy);
    assert!(
        studio.facts.candidates().is_empty()
            && studio.facts.records().is_empty()
            && memory.notes().is_empty()
    );
    assert!(
        !studio.has_external_response_work()
            && studio.pending_response.is_empty()
            && picker.pending.is_none()
            && !studio.voice_busy
    );
    let receipts: Vec<String> =
        serde_json::from_slice(&std::fs::read(data.join("receipts.json")).unwrap()).unwrap();
    assert_eq!(receipts, vec!["ok"; 7]);
    let feedback: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data.join("feedback.json")).unwrap()).unwrap();
    assert_eq!(feedback["type"], "audio.input.finished");
    assert_eq!(feedback["turn_id"], "audio-turn");
    assert_eq!(feedback["outcome"], "queued");
    assert_eq!(feedback["target"], serde_json::to_value(target).unwrap());
    assert!(feedback.get("text").is_none());
    assert!(!data.join("credentials.json").exists());
    assert!(fixture.runtime.shutdown(Duration::from_secs(2)));
    drop(fixture);
    assert!(!root.exists());
}
