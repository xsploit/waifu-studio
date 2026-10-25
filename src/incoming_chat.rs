//! Deterministic bounded ingress; caller supplies time and reconciles host authority.
//! No networking, model selection, or application side effects.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::extension_protocol::{ChatMessage, ErrorCode, IntegrationOrigin, Target};

pub const MAX_PENDING: usize = 16;
pub const MAX_CHUNK_BYTES: usize = 4096;
pub const MAX_RECENT_IDS: usize = 256;
pub const CHUNK_WINDOW: Duration = Duration::from_secs(4);
pub const MESSAGE_TTL: Duration = Duration::from_secs(15);
pub const DEDUP_TTL: Duration = Duration::from_secs(60);
type OfferResult = Result<(), (ErrorCode, String)>;

#[derive(Debug)]
struct Original {
    message: ChatMessage,
    received: Instant,
    valid: Arc<AtomicBool>,
}
#[derive(Debug)]
struct Pending {
    target: Target,
    origin: IntegrationOrigin,
    revision: Uuid,
    original: Original,
}
#[derive(Debug)]
struct Seen {
    integration_id: String,
    source_id: String,
    target: Target,
    revision: Uuid,
    received: Instant,
    valid: Arc<AtomicBool>,
}

/// Owns original arrival times and cancellation tickets, including while Busy.
/// Deliberately not Clone: retry consumes a chunk exactly once.
#[derive(Debug)]
pub struct ChatChunk {
    pub target: Target,
    pub origin: IntegrationOrigin,
    pub revision: Uuid,
    originals: Vec<Original>,
}
impl ChatChunk {
    pub fn target(&self) -> Target {
        self.target
    }
    pub fn origin(&self) -> &IntegrationOrigin {
        &self.origin
    }
    pub fn revision(&self) -> Uuid {
        self.revision
    }
    pub fn originals(&self) -> impl ExactSizeIterator<Item = &ChatMessage> {
        self.originals.iter().map(|o| &o.message)
    }
    /// Observe liveness without changing the offered-ID set while a worker runs.
    pub fn has_live(&self, now: Instant) -> bool {
        self.originals
            .iter()
            .any(|o| o.valid.load(Ordering::Relaxed) && younger(o.received, now, MESSAGE_TTL))
    }
    /// Recheck original deadlines and cancellation tickets after worker latency.
    pub fn retain_live(&mut self, now: Instant) -> bool {
        self.originals
            .retain(|o| o.valid.load(Ordering::Relaxed) && younger(o.received, now, MESSAGE_TTL));
        !self.originals.is_empty()
    }
    /// Keep only offered originals. Unknown/duplicate IDs reject the whole result.
    pub fn select(&mut self, ids: &[String]) -> Result<(), String> {
        if ids.len() > 2
            || ids.iter().enumerate().any(|(i, id)| {
                ids[..i].contains(id) || !self.originals.iter().any(|o| &o.message.source_id == id)
            })
        {
            return Err("Invalid selected chat identifiers".into());
        }
        self.originals
            .retain(|o| ids.contains(&o.message.source_id));
        Ok(())
    }
    /// UTF-8 JSON bytes, including escaping and envelope, are budgeted at offer/take.
    pub fn encode(&self) -> String {
        encode(self.originals.iter().map(|o| &o.message))
    }
}
fn encode<'a>(messages: impl Iterator<Item = &'a ChatMessage>) -> String {
    let messages: Vec<_> = messages
        .map(|m| {
            serde_json::json!({
                "source_id": m.source_id, "author_id": m.author_id,
                "author_name": m.author_name, "text": m.text,
            })
        })
        .collect();
    serde_json::json!({"kind": "chat_chunk", "messages": messages}).to_string()
}
fn younger(received: Instant, now: Instant, ttl: Duration) -> bool {
    now.saturating_duration_since(received) < ttl
}
fn busy(message: &str) -> OfferResult {
    Err((ErrorCode::Busy, message.into()))
}

#[derive(Debug, Default)]
pub struct IncomingChat {
    pending: VecDeque<Pending>,
    recent: VecDeque<Seen>,
}
impl IncomingChat {
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    fn expire(&mut self, now: Instant) {
        self.pending.retain(|p| {
            p.original.valid.load(Ordering::Relaxed)
                && younger(p.original.received, now, MESSAGE_TTL)
        });
        self.recent.retain(|s| younger(s.received, now, DEDUP_TTL));
    }
    pub fn offer(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        revision: Uuid,
        message: &ChatMessage,
        now: Instant,
    ) -> OfferResult {
        self.expire(now);
        // Duplicate acknowledgement must succeed even when either bound is full.
        if self
            .recent
            .iter()
            .any(|s| s.integration_id == origin.integration_id && s.source_id == message.source_id)
        {
            return Ok(());
        }
        if self.pending.len() == MAX_PENDING {
            return busy("incoming chat queue is full");
        }
        if self.recent.len() == MAX_RECENT_IDS {
            return busy("incoming chat dedup cache is full");
        }
        origin
            .validate()
            .map_err(|e| (ErrorCode::InvalidRequest, e.to_string()))?;
        if message.text.len() > 2048 {
            return Err((
                ErrorCode::InvalidRequest,
                "chat text exceeds 2048 bytes".into(),
            ));
        }
        if encode(std::iter::once(message)).len() > MAX_CHUNK_BYTES {
            return Err((
                ErrorCode::InvalidRequest,
                "chat message exceeds 4096-byte JSON chunk budget".into(),
            ));
        }
        let valid = Arc::new(AtomicBool::new(true));
        self.recent.push_back(Seen {
            integration_id: origin.integration_id.clone(),
            source_id: message.source_id.clone(),
            target,
            revision,
            received: now,
            valid: valid.clone(),
        });
        self.pending.push_back(Pending {
            target,
            origin: origin.clone(),
            revision,
            original: Original {
                message: message.clone(),
                received: now,
                valid,
            },
        });
        Ok(())
    }
    /// Also cancels originals held outside the buffer by an outstanding chunk.
    /// Call before offer/take/retry when host target or integration revisions change.
    pub fn reconcile(
        &mut self,
        target: Option<Target>,
        allowed: &BTreeMap<String, Uuid>,
        now: Instant,
    ) {
        for s in &self.recent {
            if target != Some(s.target) || allowed.get(&s.integration_id) != Some(&s.revision) {
                s.valid.store(false, Ordering::Relaxed);
            }
        }
        self.expire(now);
    }
    /// Clears pending and outstanding chunks, retaining replay protection.
    pub fn clear(&mut self) {
        for s in &self.recent {
            s.valid.store(false, Ordering::Relaxed);
        }
        self.pending.clear();
    }
    pub fn take_ready(&mut self, now: Instant) -> Option<ChatChunk> {
        self.expire(now);
        let first = self.pending.front()?;
        if self.pending.len() < MAX_PENDING
            && now.saturating_duration_since(first.original.received) < CHUNK_WINDOW
        {
            return None;
        }
        let mut chunk = ChatChunk {
            target: first.target,
            origin: first.origin.clone(),
            revision: first.revision,
            originals: Vec::new(),
        };
        // Stop at an intervening origin: no cross-origin FIFO leapfrogging.
        while let Some(p) = self.pending.front() {
            if p.target != chunk.target || p.origin != chunk.origin || p.revision != chunk.revision
            {
                break;
            }
            if encode(
                chunk
                    .originals
                    .iter()
                    .map(|o| &o.message)
                    .chain(std::iter::once(&p.original.message)),
            )
            .len()
                > MAX_CHUNK_BYTES
            {
                break;
            }
            chunk
                .originals
                .push(self.pending.pop_front().unwrap().original);
        }
        Some(chunk)
    }
    /// Returns the number restored. Expired/cancelled originals and overflow are
    /// discarded; accepted originals retain arrival times and their original order.
    pub fn retry_busy(&mut self, chunk: ChatChunk, now: Instant) -> usize {
        self.expire(now);
        let mut originals: Vec<_> = chunk
            .originals
            .into_iter()
            .filter(|o| o.valid.load(Ordering::Relaxed) && younger(o.received, now, MESSAGE_TTL))
            .take(MAX_PENDING - self.pending.len())
            .collect();
        let restored = originals.len();
        while let Some(original) = originals.pop() {
            self.pending.push_front(Pending {
                target: chunk.target,
                origin: chunk.origin.clone(),
                revision: chunk.revision,
                original,
            });
        }
        restored
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(n: u128) -> Target {
        Target {
            uuid: Uuid::from_u128(n),
            generation: 1,
        }
    }
    fn origin(id: &str) -> IntegrationOrigin {
        IntegrationOrigin {
            integration_id: id.into(),
            display_name: id.into(),
        }
    }
    fn message(id: usize, text: &str) -> ChatMessage {
        ChatMessage {
            source_id: id.to_string(),
            author_id: "author".into(),
            author_name: "Name".into(),
            text: text.into(),
        }
    }
    fn offer(b: &mut IncomingChat, id: usize, now: Instant) {
        b.offer(
            target(1),
            &origin("a"),
            Uuid::from_u128(1),
            &message(id, "hello"),
            now,
        )
        .unwrap();
    }
    #[test]
    fn window_and_capacity() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        offer(&mut b, 0, t);
        assert!(
            b.take_ready(t + CHUNK_WINDOW - Duration::from_nanos(1))
                .is_none()
        );
        assert_eq!(
            b.take_ready(t + CHUNK_WINDOW).unwrap().originals().count(),
            1
        );
        for i in 1..=16 {
            offer(&mut b, i, t);
        }
        assert_eq!(
            b.offer(target(1), &origin("a"), Uuid::nil(), &message(17, "x"), t)
                .unwrap_err()
                .0,
            ErrorCode::Busy
        );
        assert_eq!(b.take_ready(t).unwrap().originals().count(), 16);
    }
    #[test]
    fn dedup_saturation_and_retention() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        for i in 0..256 {
            offer(&mut b, i, t);
            b.clear();
        }
        offer(&mut b, 0, t);
        assert!(b.is_empty());
        assert_eq!(
            b.offer(target(2), &origin("a"), Uuid::nil(), &message(256, "x"), t)
                .unwrap_err()
                .0,
            ErrorCode::Busy
        );
        offer(&mut b, 256, t + DEDUP_TTL);
        assert_eq!(b.len(), 1);
    }
    #[test]
    fn escaping_budget_and_whole_messages() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        let mut m = message(1, "\"\\\n雪🙂");
        let overhead = encode(std::iter::once(&message(1, ""))).len();
        let budget = MAX_CHUNK_BYTES - overhead;
        m.text = "\"".repeat(budget / 2) + &"x".repeat(budget % 2);
        b.offer(target(1), &origin("a"), Uuid::nil(), &m, t)
            .unwrap();
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        assert_eq!(c.encode().len(), MAX_CHUNK_BYTES);
        m.source_id = "2".into();
        m.text.push('x');
        assert_eq!(
            b.offer(target(1), &origin("a"), Uuid::nil(), &m, t)
                .unwrap_err()
                .0,
            ErrorCode::InvalidRequest
        );
        m.text = "\"\\\n雪🙂".repeat(120);
        for i in 2..6 {
            m.source_id = i.to_string();
            b.offer(target(1), &origin("a"), Uuid::nil(), &m, t)
                .unwrap();
        }
        let mut count = 0;
        while let Some(c) = b.take_ready(t + CHUNK_WINDOW) {
            assert!(c.encode().len() <= MAX_CHUNK_BYTES);
            let v: serde_json::Value = serde_json::from_str(&c.encode()).unwrap();
            assert_eq!(v["kind"], "chat_chunk");
            for value in v["messages"].as_array().unwrap() {
                assert_eq!(value["text"], m.text);
                assert_eq!(value["author_id"], m.author_id);
                assert_eq!(value["author_name"], m.author_name);
            }
            count += c.originals().count();
        }
        assert_eq!(count, 4);
    }
    #[test]
    fn fifo_and_busy_capacity() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        offer(&mut b, 0, t);
        b.offer(
            target(1),
            &origin("b"),
            Uuid::from_u128(1),
            &message(1, "b"),
            t,
        )
        .unwrap();
        offer(&mut b, 2, t);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        assert_eq!(c.originals().next().unwrap().source_id, "0");
        assert_eq!(b.retry_busy(c, t + CHUNK_WINDOW), 1);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        for i in 3..17 {
            offer(&mut b, i, t);
        }
        assert_eq!(b.len(), 16);
        assert_eq!(b.retry_busy(c, t), 0);
        assert_eq!(b.take_ready(t).unwrap().origin.integration_id, "b");
    }
    #[test]
    fn ttl_includes_busy_time() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        offer(&mut b, 0, t);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        assert_eq!(b.retry_busy(c, t + MESSAGE_TTL), 0);
        offer(&mut b, 1, t);
        assert!(b.take_ready(t + MESSAGE_TTL).is_none());
    }
    #[test]
    fn duplicates_are_origin_scoped_and_do_not_refresh_ttl() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        offer(&mut b, 0, t);
        offer(&mut b, 0, t + Duration::from_secs(3));
        b.offer(
            target(1),
            &origin("b"),
            Uuid::nil(),
            &message(0, "other"),
            t,
        )
        .unwrap();
        assert_eq!(b.len(), 2);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        assert_eq!(c.target(), target(1));
        assert_eq!(c.origin(), &origin("a"));
        assert_eq!(c.revision(), Uuid::from_u128(1));
        assert_eq!(b.retry_busy(c, t + MESSAGE_TTL), 0);
    }
    #[test]
    fn escaped_2048_bytes_rejected_without_poisoning_id() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        let m = message(0, &"\\".repeat(2048));
        assert_eq!(
            b.offer(target(1), &origin("a"), Uuid::nil(), &m, t)
                .unwrap_err()
                .0,
            ErrorCode::InvalidRequest
        );
        assert!(b.is_empty());
        offer(&mut b, 0, t);
        assert_eq!(b.len(), 1);
    }
    #[test]
    fn retry_retains_fifo_and_per_original_deadlines() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        offer(&mut b, 0, t);
        offer(&mut b, 1, t + Duration::from_secs(1));
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        offer(&mut b, 2, t + Duration::from_secs(5));
        assert_eq!(b.retry_busy(c, t + MESSAGE_TTL), 1);
        let c = b.take_ready(t + MESSAGE_TTL).unwrap();
        assert_eq!(
            c.originals()
                .map(|m| m.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );
        assert_eq!(b.retry_busy(c, t + Duration::from_secs(16)), 1);
    }
    #[test]
    fn cancellation_covers_outstanding_and_dedup() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        offer(&mut b, 0, t);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        let allowed = BTreeMap::from([("a".into(), Uuid::from_u128(1))]);
        b.reconcile(Some(target(2)), &allowed, t);
        assert_eq!(b.retry_busy(c, t), 0);
        offer(&mut b, 0, t);
        assert!(b.is_empty());
        offer(&mut b, 1, t);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        b.reconcile(
            Some(target(1)),
            &BTreeMap::from([("a".into(), Uuid::from_u128(2))]),
            t,
        );
        assert_eq!(b.retry_busy(c, t), 0);
        offer(&mut b, 2, t);
        b.reconcile(None, &allowed, t);
        assert!(b.is_empty());
        offer(&mut b, 3, t);
        let c = b.take_ready(t + CHUNK_WINDOW).unwrap();
        b.clear();
        assert_eq!(b.retry_busy(c, t), 0);
    }
    #[test]
    fn selection_keeps_exact_originals_and_rechecks_expiry_and_cancellation() {
        let t = Instant::now();
        let mut b = IncomingChat::default();
        for i in 0..3 {
            offer(&mut b, i, t);
        }
        let mut chunk = b.take_ready(t + CHUNK_WINDOW).unwrap();
        assert!(chunk.select(&["unknown".into()]).is_err());
        assert_eq!(chunk.originals().len(), 3);
        assert!(chunk.select(&["1".into(), "1".into()]).is_err());
        chunk.select(&["2".into(), "0".into()]).unwrap();
        assert_eq!(
            chunk
                .originals()
                .map(|m| m.source_id.as_str())
                .collect::<Vec<_>>(),
            ["0", "2"]
        );
        assert!(chunk.retain_live(t + CHUNK_WINDOW));
        assert!(!chunk.retain_live(t + MESSAGE_TTL));
        offer(&mut b, 4, t);
        let mut chunk = b.take_ready(t + CHUNK_WINDOW).unwrap();
        b.clear();
        assert!(!chunk.retain_live(t + CHUNK_WINDOW));
    }
}
