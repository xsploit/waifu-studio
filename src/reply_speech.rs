//! Shared bounded reply speech scheduling.
//! Callers remove affect metadata before `push`; task blocks are filtered here.
//! The caller owns playback and cancellation of the VoiceWorker turn.
use crate::{
    speech_text::ReplySpeechText,
    voice::{HTTP_SLOTS, VoiceConfig, VoiceEvent, VoiceWorker},
};
use std::collections::VecDeque;

/// Sentence-at-a-time scheduling for SSE, whose worker invalidates the previous
/// audio generation on each request. Never advance until playback has drained.
pub struct SseReplySpeech {
    id: u64,
    config: VoiceConfig,
    key: Option<String>,
    filter: ReplySpeechText,
    text: String,
    finished: bool,
    in_flight: bool,
}
impl SseReplySpeech {
    pub fn new(id: u64, config: VoiceConfig, key: Option<String>) -> Self {
        Self {
            id,
            config,
            key,
            filter: Default::default(),
            text: String::new(),
            finished: false,
            in_flight: false,
        }
    }
    pub fn push(&mut self, text: &str) {
        if !self.finished {
            self.text.push_str(&self.filter.push(text));
        }
    }
    pub fn finish(&mut self) {
        if !self.finished {
            self.text.push_str(&self.filter.finish());
            self.finished = true;
        }
    }
    pub fn is_done(&self) -> bool {
        self.finished && self.text.trim().is_empty() && !self.in_flight
    }
    fn next_text(&mut self, playing: bool) -> Option<String> {
        if playing || self.in_flight {
            return None;
        }
        let text = take_speech_chunk(&mut self.text, self.finished)?;
        self.in_flight = true;
        Some(text)
    }
    pub fn pump(
        &mut self,
        fish: &mut crate::fish_live::FishLiveState,
        playing: bool,
    ) -> Result<(), String> {
        if let Some(text) = self.next_text(playing) {
            fish.speak(self.id, &self.config, text, self.key.clone())?;
        }
        Ok(())
    }
    /// Returns false for events from another speech turn.
    pub fn receive(&mut self, event: &crate::fish_live::LiveEvent) -> Result<bool, String> {
        use crate::fish_live::LiveEvent;
        if event.id() != self.id {
            return Ok(false);
        }
        match event {
            LiveEvent::Finished { .. } => self.in_flight = false,
            LiveEvent::Failed { message, .. } => return Err(message.clone()),
            _ => {}
        }
        Ok(true)
    }
}

pub struct HttpReplySpeech {
    id: u64,
    config: VoiceConfig,
    key: Option<String>,
    filter: ReplySpeechText,
    text: String,
    ready: VecDeque<VoiceEvent>,
    outstanding: usize,
    finished: bool,
}
impl HttpReplySpeech {
    pub fn new(id: u64, config: VoiceConfig, key: Option<String>) -> Self {
        Self {
            id,
            config,
            key,
            filter: Default::default(),
            text: String::new(),
            ready: VecDeque::new(),
            outstanding: 0,
            finished: false,
        }
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }
    pub fn ready_count(&self) -> usize {
        self.ready.len()
    }
    pub fn push(&mut self, text: &str) {
        if !self.finished {
            self.text.push_str(&self.filter.push(text));
        }
    }
    pub fn finish(&mut self) {
        if !self.finished {
            self.text.push_str(&self.filter.finish());
            self.finished = true;
        }
    }
    pub fn is_done(&self) -> bool {
        self.finished
            && self.text.trim().is_empty()
            && self.outstanding == 0
            && self.ready.is_empty()
    }
    /// False means a stale event; it cannot affect this turn's accounting.
    pub fn receive(&mut self, event: VoiceEvent) -> Result<bool, String> {
        if event.id() != self.id {
            return Ok(false);
        }
        self.outstanding = self.outstanding.saturating_sub(1);
        match event {
            VoiceEvent::Ready { .. } | VoiceEvent::Streaming { .. } => self.ready.push_back(event),
            VoiceEvent::Failed { message, .. } => return Err(message),
        }
        Ok(true)
    }
    /// Prefetch at most two chunks ahead and yield one ordered event if playback is idle.
    pub fn pump(
        &mut self,
        worker: &mut VoiceWorker,
        playing: bool,
    ) -> Result<Option<VoiceEvent>, String> {
        let next = if playing {
            None
        } else {
            self.ready.pop_front()
        };
        while self.outstanding + self.ready.len() < HTTP_SLOTS
            && worker.can_prefetch()
            && let Some(text) = take_speech_chunk(&mut self.text, self.finished)
        {
            worker.enqueue(self.id, self.config.clone(), text, self.key.clone())?;
            self.outstanding += 1;
        }
        Ok(next)
    }
}

const SPEECH_CHUNK_MIN: usize = 40;
pub const SPEECH_CHUNK_MAX: usize = 300;
/// Takes the next sentence group (40–300 characters). Western terminators split
/// only before whitespace, so decimals, URLs and a still-streaming "…" survive.
pub fn take_speech_chunk(text: &mut String, finished: bool) -> Option<String> {
    loop {
        let mut end = None;
        let mut last_space = None;
        let mut chars = text.char_indices().enumerate().peekable();
        while let Some((count, (index, c))) = chars.next() {
            let next = index + c.len_utf8();
            if c.is_whitespace() {
                last_space = Some(next);
            }
            let boundary = match c {
                '。' | '！' | '？' | '\n' => true,
                '.' | '!' | '?' | '…' => {
                    chars.peek().is_some_and(|(_, (_, n))| n.is_whitespace())
                }
                _ => false,
            };
            if boundary && count + 1 >= SPEECH_CHUNK_MIN {
                end = Some(next);
                break;
            }
            if count + 1 >= SPEECH_CHUNK_MAX {
                end = Some(last_space.unwrap_or(next));
                break;
            }
        }
        let end = end.or_else(|| finished.then_some(text.len()))?;
        let chunk: String = text.drain(..end).collect();
        let chunk = chunk.trim();
        if !chunk.is_empty() {
            return Some(chunk.to_owned());
        }
        if text.is_empty() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::{VoiceAdapter, VoiceAudio};

    fn ready(id: u64) -> VoiceEvent {
        VoiceEvent::Ready {
            id,
            audio: VoiceAudio {
                bytes: vec![],
                mime: "audio/wav".into(),
                format: "wav".into(),
            },
        }
    }
    #[test]
    fn sse_starts_before_finish_but_never_interrupts_network_or_playback() {
        use crate::fish_live::{LiveEvent, LiveStats};
        let mut reply = SseReplySpeech::new(7, VoiceConfig::default(), None);
        let first = "This first sentence is long enough to start speaking.";
        let second = "This second sentence must wait for the first to drain.";
        reply.push(&format!("{first} {second} Tail ```wai"));
        reply.push("fu_tasks\nsecret\n```");
        assert_eq!(reply.next_text(false).as_deref(), Some(first));
        assert!(
            !reply.finished,
            "first sentence is available before LLM completion"
        );
        assert!(
            reply.next_text(false).is_none(),
            "network still owns the generation"
        );
        assert!(
            !reply
                .receive(&LiveEvent::Finished {
                    id: 6,
                    stats: LiveStats::default()
                })
                .unwrap()
        );
        assert!(reply.in_flight);
        reply
            .receive(&LiveEvent::Finished {
                id: 7,
                stats: LiveStats::default(),
            })
            .unwrap();
        assert!(
            reply.next_text(true).is_none(),
            "network completion does not mean playback drained"
        );
        assert_eq!(reply.next_text(false).as_deref(), Some(second));
        reply
            .receive(&LiveEvent::Finished {
                id: 7,
                stats: LiveStats::default(),
            })
            .unwrap();
        assert!(
            reply.next_text(false).is_none(),
            "unfinished short tail waits"
        );
        reply.finish();
        reply.finish();
        reply.push("Must not speak late text.");
        assert_eq!(reply.next_text(false).as_deref(), Some("Tail"));
        assert!(!reply.is_done());
        reply
            .receive(&LiveEvent::Finished {
                id: 7,
                stats: LiveStats::default(),
            })
            .unwrap();
        assert!(reply.is_done());
        assert!(reply.next_text(false).is_none());
    }

    #[test]
    fn sse_failure_is_reported_only_for_the_owned_turn() {
        use crate::fish_live::LiveEvent;
        let mut reply = SseReplySpeech::new(3, VoiceConfig::default(), None);
        assert!(
            !reply
                .receive(&LiveEvent::Failed {
                    id: 2,
                    message: "old".into()
                })
                .unwrap()
        );
        assert_eq!(
            reply.receive(&LiveEvent::Failed {
                id: 3,
                message: "failed".into()
            }),
            Err("failed".into())
        );
    }

    #[test]
    fn stale_events_do_not_change_accounting_and_playback_holds_ready_chunks() {
        let mut reply = HttpReplySpeech::new(7, VoiceConfig::preset(VoiceAdapter::FishLocal), None);
        reply.outstanding = 2;
        reply.finish();
        assert!(!reply.receive(ready(6)).unwrap());
        assert_eq!(reply.outstanding(), 2);
        reply.receive(ready(7)).unwrap();
        reply.receive(ready(7)).unwrap();
        let mut worker = VoiceWorker::new();
        assert!(reply.pump(&mut worker, true).unwrap().is_none());
        assert_eq!(reply.ready_count(), 2);
        assert!(!reply.is_done());
        assert!(reply.pump(&mut worker, false).unwrap().is_some());
        assert!(!reply.is_done());
        assert!(reply.pump(&mut worker, false).unwrap().is_some());
        assert!(reply.is_done());
    }
    #[test]
    fn finish_is_idempotent_and_late_text_cannot_restart_a_reply() {
        let mut reply = HttpReplySpeech::new(1, VoiceConfig::preset(VoiceAdapter::FishLocal), None);
        reply.push("```waifu_tasks\nsecret\n``` ");
        reply.finish();
        reply.finish();
        reply.push("Must not be spoken after finish.");
        assert!(reply.is_done());
        assert!(
            reply
                .pump(&mut VoiceWorker::new(), false)
                .unwrap()
                .is_none()
        );
    }
}
