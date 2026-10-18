//! Shared bounded HTTP reply speech scheduling for Studio and its benchmark.
//! Callers remove affect metadata before `push`; task blocks are filtered here.
//! The caller owns playback and cancellation of the VoiceWorker turn.
use crate::{
    speech_text::ReplySpeechText,
    voice::{HTTP_SLOTS, VoiceConfig, VoiceEvent, VoiceWorker},
};
use std::collections::VecDeque;

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
