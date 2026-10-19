//! Latest-turn timings only; no prompts, response text, keys or disk writes.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    Prepared,
    Worker,
    Request,
    Headers,
    RawText,
    Consumed,
    Layout,
    Returned,
    SpeechConnect,
    SpeechQueued,
    AudioQueued,
}
impl Stage {
    pub const ALL: [(Self, &'static str); 11] = [
        (Self::Prepared, "Request prepared"),
        (Self::Worker, "Worker started"),
        (Self::Request, "HTTP send started"),
        (Self::Headers, "Response headers"),
        (Self::RawText, "First provider text"),
        (Self::Consumed, "First text consumed"),
        (Self::Layout, "First text laid out"),
        (Self::Returned, "Send handler returned"),
        (Self::SpeechConnect, "Speech connection queued (WebSocket)"),
        (Self::SpeechQueued, "First speech text queued"),
        (Self::AudioQueued, "First audio queued for playback"),
    ];
}
struct Data {
    started: Instant,
    stages: [Option<Duration>; 11],
    history_save: Option<Duration>,
    cancelled: bool,
}
#[derive(Clone)]
pub struct ReplyTiming(Arc<Mutex<Data>>);
impl ReplyTiming {
    pub fn new(started: Instant) -> Self {
        Self(Arc::new(Mutex::new(Data {
            started,
            stages: [None; 11],
            history_save: None,
            cancelled: false,
        })))
    }
    pub fn mark(&self, stage: Stage) {
        self.mark_at(stage, Instant::now());
    }
    fn mark_at(&self, stage: Stage, now: Instant) {
        if let Ok(mut data) = self.0.lock()
            && !data.cancelled
        {
            let elapsed = now.saturating_duration_since(data.started);
            data.stages[stage as usize].get_or_insert(elapsed);
        }
    }
    pub fn history_save(&self, duration: Duration) {
        if let Ok(mut data) = self.0.lock() {
            data.history_save = Some(duration);
        }
    }
    pub fn cancel(&self) {
        if let Ok(mut data) = self.0.lock() {
            data.cancelled = true;
        }
    }
    pub fn snapshot(&self) -> Option<([Option<Duration>; 11], Option<Duration>, bool)> {
        self.0
            .lock()
            .ok()
            .map(|data| (data.stages, data.history_save, data.cancelled))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_marks_win_and_cancel_freezes_late_workers() {
        let start = Instant::now();
        let timing = ReplyTiming::new(start);
        let worker = timing.clone();
        worker.mark_at(Stage::RawText, start + Duration::from_millis(80));
        worker.mark_at(Stage::RawText, start + Duration::from_millis(90));
        timing.cancel();
        worker.mark_at(Stage::Headers, start + Duration::from_millis(100));
        let (stages, history, cancelled) = timing.snapshot().unwrap();
        assert_eq!(
            stages[Stage::RawText as usize],
            Some(Duration::from_millis(80))
        );
        assert_eq!(stages[Stage::Headers as usize], None);
        assert_eq!(history, None);
        assert!(cancelled);
    }
    #[test]
    fn new_turn_is_isolated_and_history_duration_is_separate() {
        let start = Instant::now();
        let old = ReplyTiming::new(start);
        let new = ReplyTiming::new(start);
        old.mark_at(Stage::RawText, start + Duration::from_secs(1));
        new.history_save(Duration::from_millis(4));
        let (stages, history, _) = new.snapshot().unwrap();
        assert!(stages.iter().all(Option::is_none));
        assert_eq!(history, Some(Duration::from_millis(4)));
    }
}
