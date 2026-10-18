//! Incremental speech text, preserving the completed-reply rule that agent
//! proposals must never be spoken. The marker may be split across LLM deltas.
const TASK_MARKER: &str = "```waifu_tasks";
const MAX_CHARACTERS: usize = 12_000;

pub struct ReplySpeechText {
    limit: usize,
    held: String,
    emitted: usize,
    stopped: bool,
}

impl Default for ReplySpeechText {
    fn default() -> Self {
        Self::with_limit(MAX_CHARACTERS)
    }
}
impl ReplySpeechText {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit: limit.min(MAX_CHARACTERS),
            held: String::new(),
            emitted: 0,
            stopped: false,
        }
    }
    pub fn push(&mut self, delta: &str) -> String {
        if self.stopped {
            return String::new();
        }
        let remaining = self.limit.saturating_sub(self.emitted);
        self.held
            .extend(delta.chars().take(remaining + TASK_MARKER.len()));
        if let Some(index) = self.held.find(TASK_MARKER) {
            let text: String = self.held[..index].chars().take(remaining).collect();
            self.emitted += text.chars().count();
            self.held.clear();
            self.stopped = true;
            return text;
        }
        // Retain only a possible marker prefix; never split a UTF-8 scalar.
        let keep = (1..TASK_MARKER.len())
            .rev()
            .find(|&length| self.held.ends_with(&TASK_MARKER[..length]))
            .unwrap_or(0);
        let end = self.held.len() - keep;
        let text: String = self.held[..end].chars().take(remaining).collect();
        self.emitted += text.chars().count();
        self.held.drain(..end);
        if self.emitted >= self.limit {
            self.held.clear();
            self.stopped = true;
        }
        text
    }

    pub fn finish(&mut self) -> String {
        if self.stopped {
            return String::new();
        }
        self.stopped = true;
        let text: String = self
            .held
            .chars()
            .take(self.limit.saturating_sub(self.emitted))
            .collect();
        self.held.clear();
        self.emitted += text.chars().count();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_marker_split_matches_completed_reply_filter() {
        let reply = "Hello, 世界. ```waifu_tasks\n[{\"prompt\":\"not speech\"}]```";
        for split in 0..=reply.len() {
            if !reply.is_char_boundary(split) {
                continue;
            }
            let mut filter = ReplySpeechText::default();
            let mut spoken = filter.push(&reply[..split]);
            spoken.push_str(&filter.push(&reply[split..]));
            spoken.push_str(&filter.finish());
            assert_eq!(spoken, "Hello, 世界. ", "split {split}");
        }
        let mut filter = ReplySpeechText::default();
        let spoken = reply
            .chars()
            .map(|c| filter.push(&c.to_string()))
            .collect::<String>();
        assert_eq!(spoken, "Hello, 世界. ");
    }

    #[test]
    fn ordinary_markdown_and_incomplete_markers_are_not_lost() {
        for text in [
            "Hi!",
            "`code`",
            "```rust\nlet a = 1;```",
            "prefix ```wai",
            "😊",
        ] {
            let mut filter = ReplySpeechText::default();
            let mut spoken = filter.push(text);
            spoken.push_str(&filter.finish());
            assert_eq!(spoken, text);
            assert!(filter.finish().is_empty());
            assert!(filter.push("late").is_empty());
        }
    }

    #[test]
    fn speech_limit_is_in_characters_and_holds_no_unbounded_tail() {
        let mut filter = ReplySpeechText::default();
        let spoken = filter.push(&"語".repeat(MAX_CHARACTERS + 10_000));
        assert_eq!(spoken.chars().count(), MAX_CHARACTERS);
        assert!(filter.held.is_empty());
        assert!(filter.push("more").is_empty());
        assert!(filter.finish().is_empty());
    }
}
