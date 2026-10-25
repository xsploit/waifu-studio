//! Single-flight extension ASR. Dropping a job cancels publication, not its
//! blocking HTTP operation; the worker retains its permit until actual exit.
use crate::{
    extension_audio::{AudioRoute, ReadyAudio},
    listen::ListenConfig,
};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone)]
pub struct AsrPool(Arc<AtomicBool>);
impl Default for AsrPool {
    fn default() -> Self {
        static BUSY: OnceLock<Arc<AtomicBool>> = OnceLock::new();
        Self(
            BUSY.get_or_init(|| Arc::new(AtomicBool::new(false)))
                .clone(),
        )
    }
}
pub struct AsrPermit(Arc<AtomicBool>);
impl Drop for AsrPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl AsrPool {
    /// Reserve before consuming a finished turn, so Busy has no intake effect.
    pub fn reserve(&self) -> Option<AsrPermit> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| AsrPermit(self.0.clone()))
    }
}

pub struct AudioTranscript {
    pub route: AudioRoute,
    pub turn_id: String,
    pub participant_id: String,
    pub participant_name: String,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AsrError {
    Cancelled,
    Failed,
}

pub struct AsrJob {
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<AudioTranscript, AsrError>>,
    terminal: bool,
}
impl AsrPermit {
    /// Run with the existing app route and credential; neither is exposed to a child.
    pub fn start(
        self,
        audio: ReadyAudio,
        config: ListenConfig,
        session_key: String,
    ) -> Result<AsrJob, AsrError> {
        self.start_using(audio, move |wav, cancelled| {
            crate::listen::transcribe_wav(&config, wav, Some(&session_key), &|| {
                cancelled.load(Ordering::Acquire)
            })
            .map_err(|_| AsrError::Failed)
        })
    }

    fn start_using(
        self,
        audio: ReadyAudio,
        transcribe: impl FnOnce(&[u8], &AtomicBool) -> Result<String, AsrError> + Send + 'static,
    ) -> Result<AsrJob, AsrError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("extension-asr".into())
            .spawn(move || {
                let _permit = self;
                if worker_cancelled.load(Ordering::Acquire) {
                    return;
                }
                let text = transcribe(&audio.wav, &worker_cancelled);
                if worker_cancelled.load(Ordering::Acquire) {
                    return;
                }
                let result = text.map(|text| AudioTranscript {
                    route: audio.route,
                    turn_id: audio.turn_id,
                    participant_id: audio.participant_id,
                    participant_name: audio.participant_name,
                    text,
                });
                let _ = sender.send(result);
            })
            .map_err(|_| AsrError::Failed)?;
        Ok(AsrJob {
            cancelled,
            receiver,
            terminal: false,
        })
    }
}
impl AsrJob {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// Consume a result once; final host route/permission checks remain required.
    pub fn poll(&mut self) -> Option<Result<AudioTranscript, AsrError>> {
        if self.terminal {
            return None;
        }
        if self.cancelled.load(Ordering::Acquire) {
            self.terminal = true;
            return Some(Err(AsrError::Cancelled));
        }
        match self.receiver.try_recv() {
            Ok(result) => {
                self.terminal = true;
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.terminal = true;
                Some(Err(AsrError::Failed))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
impl Drop for AsrJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_protocol::{IntegrationOrigin, Target};
    use std::time::{Duration, Instant};
    use uuid::Uuid;
    fn pool() -> AsrPool {
        AsrPool(Arc::new(AtomicBool::new(false)))
    }
    fn audio() -> ReadyAudio {
        ReadyAudio {
            route: AudioRoute {
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
            },
            turn_id: "turn".into(),
            participant_id: "speaker".into(),
            participant_name: "Speaker".into(),
            wav: vec![1, 2, 3],
        }
    }
    fn result(job: &mut AsrJob) -> Result<AudioTranscript, AsrError> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn worker_preserves_route_and_exact_external_text_once() {
        let pool = pool();
        let audio = audio();
        let route = audio.route.clone();
        let mut job = pool
            .reserve()
            .unwrap()
            .start_using(audio, |wav, _| {
                assert_eq!(wav, &[1, 2, 3]);
                Ok("Exact words".into())
            })
            .unwrap();
        let transcript = result(&mut job).unwrap();
        assert_eq!(transcript.route, route);
        assert_eq!(transcript.participant_id, "speaker");
        assert_eq!(transcript.text, "Exact words");
        assert!(job.poll().is_none());
    }
    #[test]
    fn dropped_blocked_worker_keeps_permit_until_it_really_exits() {
        let pool = pool();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let job = pool
            .reserve()
            .unwrap()
            .start_using(audio(), move |_, _| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                Ok("late".into())
            })
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(job);
        assert!(pool.reserve().is_none());
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(permit) = pool.reserve() {
                drop(permit);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn cancellation_discards_an_already_completed_result() {
        let pool = pool();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut job = pool
            .reserve()
            .unwrap()
            .start_using(audio(), move |_, _| {
                tx.send(()).unwrap();
                Ok("late".into())
            })
            .unwrap();
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        // Permit release occurs after publication, proving the result is queued.
        loop {
            if let Some(permit) = pool.reserve() {
                drop(permit);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        job.cancel();
        assert!(matches!(result(&mut job), Err(AsrError::Cancelled)));
        assert!(job.poll().is_none());
    }

    #[test]
    fn provider_failure_is_terminal_and_releases_capacity() {
        let pool = pool();
        let mut job = pool
            .reserve()
            .unwrap()
            .start_using(audio(), |_, _| Err(AsrError::Failed))
            .unwrap();
        assert!(matches!(result(&mut job), Err(AsrError::Failed)));
        assert!(job.poll().is_none());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(permit) = pool.reserve() {
                drop(permit);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
