//! Application-owned media state. No UI or credential serialization.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use waifu_studio::{
    extension_audio::{AudioRoute, AudioTurns},
    extension_audio_job::{AsrJob, AsrPool},
    extension_protocol::{AudioInput, AudioInputOutcome, ChatMessage, ErrorCode},
    listen::ListenConfig,
};
#[cfg(test)]
pub(super) static AUDIO_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Pending {
    route: AudioRoute,
    turn_id: String,
    started: Instant,
    job: AsrJob,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    };
    use uuid::Uuid;
    use waifu_studio::{
        extension_protocol::{IntegrationOrigin, Target},
        listen::SttBackend,
    };
    fn route() -> AudioRoute {
        AudioRoute {
            session: Uuid::new_v4(),
            process_generation: 1,
            target: Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            },
            origin: IntegrationOrigin {
                integration_id: "fixture".into(),
                display_name: "Fixture".into(),
            },
            revision: Uuid::new_v4(),
        }
    }
    fn start() -> AudioInput {
        AudioInput::Start {
            turn_id: "turn".into(),
            participant_id: "speaker".into(),
            participant_name: "Speaker".into(),
        }
    }
    fn frame() -> AudioInput {
        AudioInput::Frame {
            turn_id: "turn".into(),
            frame_sequence: 0,
            pcm: vec![1; 320],
        }
    }
    fn finish() -> AudioInput {
        AudioInput::Finish {
            turn_id: "turn".into(),
        }
    }
    #[test]
    fn service_key_mute_and_source_changes_discard_unfinished_turns() {
        let _guard = AUDIO_TEST_LOCK.lock().unwrap();
        for case in 0..5 {
            let mut intake = AudioIntake::default();
            let route = route();
            let now = Instant::now();
            let mut config = ListenConfig {
                endpoint: "://never-connect".into(),
                ..Default::default()
            };
            intake.reconcile(vec![route.clone()], &config, "synthetic", now);
            intake
                .accept(&route, &start(), &config, "synthetic", now)
                .unwrap();
            intake
                .accept(&route, &frame(), &config, "synthetic", now)
                .unwrap();
            let mut routes = vec![route.clone()];
            let mut key = "synthetic";
            match case {
                0 => config.model = "changed".into(),
                1 => key = "replacement",
                2 => config.enabled = false,
                3 => routes.clear(),
                _ => routes[0].revision = Uuid::new_v4(),
            }
            intake.reconcile(routes, &config, key, now);
            assert!(intake.accept(&route, &finish(), &config, key, now).is_err());
            assert!(intake.pending.is_none());
        }
    }

    #[test]
    fn full_completion_budget_refuses_finish_without_consuming_audio() {
        let _guard = AUDIO_TEST_LOCK.lock().unwrap();
        let mut intake = AudioIntake::default();
        let route = route();
        let now = Instant::now();
        let config = ListenConfig::default();
        intake.reconcile(vec![route.clone()], &config, "synthetic", now);
        intake
            .accept(&route, &start(), &config, "synthetic", now)
            .unwrap();
        intake
            .accept(&route, &frame(), &config, "synthetic", now)
            .unwrap();
        for index in 0..8 {
            intake.completed.push_back(AudioCompletion {
                route: route.clone(),
                turn_id: format!("old-{index}"),
                result: Err(AudioInputOutcome::Cancelled),
            });
        }
        assert_eq!(
            intake
                .accept(&route, &finish(), &config, "synthetic", now)
                .unwrap_err()
                .0,
            ErrorCode::Busy
        );
        assert!(intake.pending.is_none());
        for _ in 0..8 {
            assert!(intake.poll().is_some());
        }
        assert!(intake.poll().is_none());
        let ready = intake
            .turns
            .accept(&route, &finish(), now)
            .unwrap()
            .unwrap();
        assert_eq!(ready.wav.len(), 684);
    }

    #[test]
    fn actual_asr_adapter_produces_external_candidate_and_discards_revoked_job() {
        let _guard = AUDIO_TEST_LOCK.lock().unwrap();
        for case in 0..5 {
            let revoke = case == 1;
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let config = ListenConfig {
                backend: SttBackend::OpenAiCompatible,
                endpoint: format!("http://{}/transcriptions", listener.local_addr().unwrap()),
                model: "fixture-asr".into(),
                api_key_env: String::new(),
                ..Default::default()
            };
            let (entered_tx, entered_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let server = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
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
                    let count = stream.read(&mut buf).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buf[..count]);
                    assert!(request.len() < 16384);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let size: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|s| s.trim().parse().unwrap())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + size {
                            break;
                        }
                    }
                }
                assert!(request.windows(4).any(|w| w == b"RIFF"));
                assert!(String::from_utf8_lossy(&request).contains("Bearer synthetic"));
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let text = if case == 3 {
                    "x".repeat(2049)
                } else {
                    "Exact participant speech".into()
                };
                let body = serde_json::to_vec(&serde_json::json!({"text":text})).unwrap();
                let status = if case == 2 { "500 Error" } else { "200 OK" };
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                let _ = stream.write_all(&body);
            });
            let mut intake = AudioIntake::default();
            let route = route();
            let now = Instant::now();
            intake.reconcile(vec![route.clone()], &config, "synthetic", now);
            for input in [start(), frame(), finish()] {
                intake
                    .accept(&route, &input, &config, "synthetic", now)
                    .unwrap();
            }
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if revoke {
                intake.reconcile(Vec::new(), &config, "synthetic", Instant::now());
                assert!(intake.pending.is_none());
            }
            if case == 4 {
                intake
                    .accept(
                        &route,
                        &AudioInput::Cancel {
                            turn_id: "turn".into(),
                        },
                        &config,
                        "synthetic",
                        Instant::now(),
                    )
                    .unwrap();
            }
            release_tx.send(()).unwrap();
            server.join().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let completion = loop {
                if let Some(result) = intake.poll() {
                    break result;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            };
            assert_eq!(completion.route, route);
            assert_eq!(completion.turn_id, "turn");
            match case {
                1 | 4 => assert!(matches!(
                    completion.result,
                    Err(AudioInputOutcome::Cancelled)
                )),
                2 => assert!(matches!(completion.result, Err(AudioInputOutcome::Failed))),
                3 => assert!(matches!(
                    completion.result,
                    Err(AudioInputOutcome::Rejected)
                )),
                _ => {
                    let message = completion.result.unwrap();
                    assert_eq!(message.author_id, "speaker");
                    assert_eq!(message.author_name, "Speaker");
                    assert_eq!(message.text, "Exact participant speech");
                    assert!(message.source_id.starts_with("audio:"));
                }
            }
            assert!(intake.poll().is_none());
            // A revoked HTTP worker must actually exit before the next fixture.
            loop {
                if let Some(permit) = intake.pool.reserve() {
                    drop(permit);
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
pub(super) struct AudioCompletion {
    pub route: AudioRoute,
    pub turn_id: String,
    pub result: Result<ChatMessage, AudioInputOutcome>,
}

#[derive(Default)]
pub(super) struct AudioIntake {
    completed: VecDeque<AudioCompletion>,
    turns: AudioTurns,
    pool: AsrPool,
    pending: Option<Pending>,
    routes: Vec<AudioRoute>,
    service: Option<(String, String)>,
}
impl AudioIntake {
    pub fn reconcile(
        &mut self,
        routes: Vec<AudioRoute>,
        config: &ListenConfig,
        key: &str,
        now: Instant,
    ) {
        if routes.is_empty() || !config.enabled {
            self.turns.reconcile(&[], now);
            self.cancel_pending();
            self.routes.clear();
            self.service = None;
            return;
        }
        // Memory-only snapshot includes credential changes; never log or persist it.
        let service = (
            serde_json::to_string(config).unwrap_or_default(),
            key.to_owned(),
        );
        if self.service.as_ref() != Some(&service) {
            self.turns.reconcile(&[], now);
            self.cancel_pending();
            self.service = Some(service);
        }
        self.routes = routes;
        self.turns.reconcile(&self.routes, now);
        if self.pending.as_ref().is_some_and(|p| {
            !self.routes.contains(&p.route)
                || now.saturating_duration_since(p.started) >= Duration::from_secs(45)
        }) {
            self.cancel_pending();
        }
    }
    pub fn accept(
        &mut self,
        route: &AudioRoute,
        input: &AudioInput,
        config: &ListenConfig,
        key: &str,
        now: Instant,
    ) -> Result<(), (ErrorCode, String)> {
        if !self.routes.contains(route) {
            return Err((
                ErrorCode::PermissionDenied,
                "Audio source is not enabled".into(),
            ));
        }
        if let AudioInput::Cancel { turn_id } = input
            && self
                .pending
                .as_ref()
                .is_some_and(|p| p.route == *route && p.turn_id == *turn_id)
        {
            self.cancel_pending();
            return Ok(());
        }
        let permit = if matches!(input, AudioInput::Finish { .. }) {
            if self.pending.is_some() || self.completed.len() >= 8 {
                return Err((ErrorCode::Busy, "Speech recognition is busy".into()));
            }
            Some(
                self.pool
                    .reserve()
                    .ok_or((ErrorCode::Busy, "Speech recognition is busy".into()))?,
            )
        } else {
            None
        };
        if let Some(audio) = self.turns.accept(route, input, now)? {
            let turn_id = audio.turn_id.clone();
            let job = permit
                .unwrap()
                .start(audio, config.clone(), key.to_owned())
                .map_err(|_| {
                    (
                        ErrorCode::Unavailable,
                        "Speech recognition could not start".into(),
                    )
                })?;
            self.pending = Some(Pending {
                route: route.clone(),
                turn_id,
                started: now,
                job,
            });
        }
        Ok(())
    }
    /// Caller offers this through the existing attributed external-chat queue.
    /// Reconcile current authority immediately before polling every frame.
    fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.completed.push_back(AudioCompletion {
                route: pending.route,
                turn_id: pending.turn_id,
                result: Err(AudioInputOutcome::Cancelled),
            });
        }
    }
    pub fn poll(&mut self) -> Option<AudioCompletion> {
        if let Some(completed) = self.completed.pop_front() {
            return Some(completed);
        }
        let pending = self.pending.as_mut()?;
        let result = pending.job.poll()?;
        let pending = self.pending.take().unwrap();
        let result = result
            .map_err(|_| AudioInputOutcome::Failed)
            .and_then(|result| {
                if !self.routes.contains(&result.route) {
                    return Err(AudioInputOutcome::Cancelled);
                }
                let message = ChatMessage {
                    source_id: format!("audio:{}", uuid::Uuid::new_v4()),
                    author_id: result.participant_id,
                    author_name: result.participant_name,
                    text: result.text,
                };
                message
                    .validate()
                    .map_err(|_| AudioInputOutcome::Rejected)?;
                Ok(message)
            });
        Some(AudioCompletion {
            route: pending.route,
            turn_id: pending.turn_id,
            result,
        })
    }
}
