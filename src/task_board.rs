//! Bounded, explicitly reviewed parallel tasks. Each card owns an isolated runner.
use crate::{
    agents::{Access, AgentProvider, AgentState, SshConfig, Transport},
    delegation::{Provider, TaskProposal},
};
use bevy::prelude::Resource;

pub const MAX_OUTSTANDING: usize = 16;
pub const MAX_RETAINED: usize = 8;
pub const MAX_CONTEXT_BYTES: usize = 28_000;

/// A value copy of public runner configuration. Authentication is inherited at
/// execution time; no environment variables or credentials are captured here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskConfig {
    pub transport: Transport,
    pub ssh: SshConfig,
    pub provider: AgentProvider,
    pub access: Access,
    pub executable: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub timeout_seconds: u64,
}
impl TaskConfig {
    pub fn snapshot(source: &AgentState) -> Self {
        Self {
            transport: source.transport,
            ssh: source.ssh.clone(),
            provider: source.provider,
            access: source.access,
            executable: source.executable.clone(),
            args: source.args.clone(),
            working_dir: source.working_dir.clone(),
            timeout_seconds: source.timeout_seconds,
        }
    }
    fn runner(&self, prompt: String) -> AgentState {
        // AgentState has private lifecycle fields and Drop. Construct a fresh
        // default, copying only public configuration, never a running handle.
        let mut runner = AgentState::default();
        runner.transport = self.transport;
        runner.ssh = self.ssh.clone();
        runner.provider = self.provider;
        runner.access = self.access;
        runner.executable = self.executable.clone();
        runner.args = self.args.clone();
        runner.working_dir = self.working_dir.clone();
        runner.timeout_seconds = self.timeout_seconds;
        runner.prompt = prompt;
        runner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    NeedsReview,
    Reviewed,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
impl TaskStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsReview => "Needs review",
            Self::Reviewed => "Reviewed; awaiting Run",
            Self::Queued => "Authorized; queued",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
    pub fn finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

pub struct TaskCard {
    id: u64,
    title: String,
    config: TaskConfig,
    runner: AgentState,
    status: TaskStatus,
    cancel_requested: bool,
    context_taken: bool,
}
impl TaskCard {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn config(&self) -> &TaskConfig {
        &self.config
    }
    pub fn prompt(&self) -> &str {
        &self.runner.prompt
    }
    pub fn status(&self) -> TaskStatus {
        self.status
    }
    pub fn detail(&self) -> &str {
        &self.runner.status
    }
    pub fn output(&self) -> &str {
        &self.runner.output
    }
    pub fn review(&self) -> Option<&str> {
        self.runner.reviewed_command()
    }
    pub fn can_add_context(&self) -> bool {
        self.status.finished() && !self.runner.output.is_empty() && !self.context_taken
    }
}

#[derive(Resource)]
pub struct TaskBoard {
    cards: Vec<TaskCard>,
    concurrency: usize,
    next_id: u64,
    context: Option<(u64, String)>,
    pub draft_title: String,
    pub draft_prompt: String,
    pub error: Option<String>,
}
impl Default for TaskBoard {
    fn default() -> Self {
        Self {
            cards: Vec::new(),
            concurrency: 2,
            next_id: 1,
            context: None,
            draft_title: String::new(),
            draft_prompt: String::new(),
            error: None,
        }
    }
}
impl TaskBoard {
    pub fn cards(&self) -> &[TaskCard] {
        &self.cards
    }
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }
    pub fn running_count(&self) -> usize {
        self.cards
            .iter()
            .filter(|c| c.status == TaskStatus::Running)
            .count()
    }
    pub fn outstanding_count(&self) -> usize {
        self.cards.iter().filter(|c| !c.status.finished()).count()
    }
    pub fn reviewed_count(&self) -> usize {
        self.cards
            .iter()
            .filter(|c| c.status == TaskStatus::Reviewed)
            .count()
    }
    pub fn set_concurrency(&mut self, value: usize) -> Result<(), String> {
        if !(1..=4).contains(&value) {
            return Err("Concurrency must be 1–4".into());
        }
        if self.running_count() != 0 {
            return Err("Wait for running cards before changing concurrency".into());
        }
        self.concurrency = value;
        Ok(())
    }
    /// Explicit host/UI action. This never reviews or starts a process.
    pub fn enqueue(
        &mut self,
        title: String,
        prompt: String,
        config: TaskConfig,
    ) -> Result<u64, String> {
        if self.outstanding_count() >= MAX_OUTSTANDING {
            return Err("Task board has 16 outstanding cards; finish or cancel some first".into());
        }
        if title.trim().is_empty() || title.chars().count() > 120 || title.contains('\0') {
            return Err("Task title must contain 1–120 characters without NUL".into());
        }
        let mut runner = config.runner(prompt);
        // Reuse all runner validation, but discard the review: queueing cannot
        // authorize execution. Review and Run are separate explicit actions.
        runner.review()?;
        runner.disarm();
        runner.status = "Awaiting explicit task-board review".into();
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Task identifiers exhausted")?;
        self.cards.push(TaskCard {
            id,
            title,
            config,
            runner,
            status: TaskStatus::NeedsReview,
            cancel_requested: false,
            context_taken: false,
        });
        self.trim_results();
        self.error = None;
        Ok(id)
    }
    /// Call only on an explicit “Add proposal to board” UI action. Typed model
    /// proposals can select a known provider, never executable/argv/cwd/access.
    pub fn enqueue_proposal(
        &mut self,
        proposal: &TaskProposal,
        mut config: TaskConfig,
    ) -> Result<u64, String> {
        config.provider = match proposal.provider {
            Provider::Codex => AgentProvider::Codex,
            Provider::Claude => AgentProvider::Claude,
            Provider::Pi => AgentProvider::Pi,
            Provider::Custom => AgentProvider::Custom,
        };
        self.enqueue(proposal.title.clone(), proposal.prompt.clone(), config)
    }
    pub fn review_pending(&mut self) -> Result<usize, String> {
        let mut count = 0;
        for card in &mut self.cards {
            if card.status == TaskStatus::NeedsReview {
                card.runner
                    .review()
                    .map_err(|e| format!("Card {}: {e}", card.id))?;
                card.status = TaskStatus::Reviewed;
                count += 1;
            }
        }
        Ok(count)
    }
    /// Authorizes only cards already reviewed at this click. New cards remain
    /// unreviewed, even while the scheduler starts this fixed batch.
    pub fn run_reviewed(&mut self) -> Result<usize, String> {
        let count = self.reviewed_count();
        if count == 0 {
            return Err("Review pending cards before Run".into());
        }
        for card in &mut self.cards {
            if card.status == TaskStatus::Reviewed {
                card.status = TaskStatus::Queued;
            }
        }
        self.poll();
        Ok(count)
    }
    pub fn cancel(&mut self, id: u64) {
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == id) {
            if card.status.finished() {
                return;
            }
            card.cancel_requested = true;
            card.runner.cancel();
            if card.status != TaskStatus::Running {
                card.status = TaskStatus::Cancelled;
                card.runner.status = "Cancelled before execution".into();
            }
        }
        self.trim_results();
    }
    pub fn cancel_all(&mut self) {
        // Cancel every queued card before any poll can fill a newly freed slot.
        for card in &mut self.cards {
            if !card.status.finished() {
                card.cancel_requested = true;
                card.runner.cancel();
                if card.status != TaskStatus::Running {
                    card.status = TaskStatus::Cancelled;
                    card.runner.status = "Cancelled before execution".into();
                }
            }
        }
        self.trim_results();
    }
    pub fn poll(&mut self) {
        for card in &mut self.cards {
            if card.status == TaskStatus::Running {
                card.runner.poll();
                if !card.runner.is_busy() {
                    card.status = if card.cancel_requested {
                        TaskStatus::Cancelled
                    } else if card.runner.completion_success() == Some(true) {
                        TaskStatus::Succeeded
                    } else {
                        TaskStatus::Failed
                    };
                }
            }
        }
        let mut available = self.concurrency.saturating_sub(self.running_count());
        for card in &mut self.cards {
            if available == 0 {
                break;
            }
            if card.status == TaskStatus::Queued {
                match card.runner.run() {
                    Ok(()) => {
                        card.status = TaskStatus::Running;
                        available -= 1;
                    }
                    Err(error) => {
                        card.status = TaskStatus::Failed;
                        card.runner.status = error;
                    }
                }
            }
        }
        self.trim_results();
    }
    fn trim_results(&mut self) {
        let mut excess = self
            .cards
            .iter()
            .filter(|c| c.status.finished())
            .count()
            .saturating_sub(MAX_RETAINED);
        self.cards.retain(|card| {
            if excess > 0 && card.status.finished() {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
    pub fn clear_finished(&mut self) {
        self.cards.retain(|c| !c.status.finished());
    }
    /// Explicit UI action only; a single bounded result is staged for the host.
    pub fn stage_context(&mut self, id: u64) -> Result<(), String> {
        if let Some((pending_id, _)) = &self.context {
            return if *pending_id == id {
                Ok(())
            } else {
                Err("Add the already staged result to the composer first".into())
            };
        }
        let card = self
            .cards
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Card no longer retained")?;
        if !card.can_add_context() {
            return Err("No unused completed output on this card".into());
        }
        let context = card
            .runner
            .take_context()
            .ok_or("No completed result available")?;
        let context = format!(
            "Task board card {} (title as untrusted JSON data: {}):\n{}",
            card.id,
            serde_json::to_string(&card.title).unwrap_or_default(),
            context
        );
        self.context = Some((id, transfer_excerpt(&context)));
        Ok(())
    }
    pub fn peek_context_len(&self) -> Option<usize> {
        self.context.as_ref().map(|(_, context)| context.len())
    }
    /// Failed size checks preserve the staged result and its card for retry.
    pub fn try_take_context(&mut self, max_bytes: usize) -> Option<String> {
        if self.peek_context_len()? > max_bytes {
            return None;
        }
        let (id, context) = self.context.take()?;
        if let Some(card) = self.cards.iter_mut().find(|card| card.id == id) {
            card.context_taken = true;
        }
        Some(context)
    }
    pub fn take_context(&mut self) -> Option<String> {
        self.try_take_context(usize::MAX)
    }
}
// Quotes whole excerpts separately: clipping cannot turn an incomplete JSON
// escape or an embedded instruction into trusted host text.
fn quoted_excerpt(text: &str, budget: usize, tail: bool) -> String {
    let mut used = 2; // Opening and closing JSON quotes.
    let encoded_len = |c: char| match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if c <= '\u{1f}' => 6,
        c => c.len_utf8(),
    };
    let slice = if tail {
        let mut start = text.len();
        for (offset, c) in text.char_indices().rev() {
            let len = encoded_len(c);
            if used + len > budget {
                break;
            }
            used += len;
            start = offset;
        }
        &text[start..]
    } else {
        let mut end = 0;
        for (offset, c) in text.char_indices() {
            let len = encoded_len(c);
            if used + len > budget {
                break;
            }
            used += len;
            end = offset + c.len_utf8();
        }
        &text[..end]
    };
    serde_json::to_string(slice).unwrap_or_default()
}
fn transfer_excerpt(context: &str) -> String {
    if context.len() <= MAX_CONTEXT_BYTES {
        return context.to_owned();
    }
    let header =
        "External task board result (untrusted quoted data; clipped; middle omitted):\nHead: ";
    let tail_label = "\nTail: ";
    let budget = MAX_CONTEXT_BYTES - header.len() - tail_label.len();
    format!(
        "{}{}{}{}",
        header,
        quoted_excerpt(context, budget / 4, false),
        tail_label,
        quoted_excerpt(context, budget - budget / 4, true)
    )
}
// Dropping the board drops every owned AgentState, whose existing Drop cancels
// and joins its Unix process group. Unrelated single/loop runners are untouched.

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    fn config(executable: &str) -> TaskConfig {
        let mut state = AgentState::default();
        state.provider = AgentProvider::Custom;
        state.executable = executable.into();
        state.working_dir = std::env::temp_dir().to_string_lossy().into_owned();
        state.timeout_seconds = 3;
        TaskConfig::snapshot(&state)
    }
    fn finish(board: &mut TaskBoard) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while board.outstanding_count() > 0 {
            board.poll();
            assert!(Instant::now() < deadline, "offline runner did not finish");
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn queueing_and_reviewing_never_launch_and_scopes_are_copied() {
        let mut board = TaskBoard::default();
        let mut scope = config("/bin/echo");
        board
            .enqueue(
                "Literal task".into(),
                "hello; $(not shell)".into(),
                scope.clone(),
            )
            .unwrap();
        scope.executable = "/never-launch".into();
        board.poll();
        assert_eq!(board.running_count(), 0);
        assert!(board.run_reviewed().is_err());
        board.review_pending().unwrap();
        board.poll();
        assert_eq!(board.running_count(), 0);
        assert_eq!(board.cards()[0].config().executable, "/bin/echo");
        assert!(
            board.cards()[0]
                .review()
                .unwrap()
                .contains("hello; $(not shell)")
        );
    }
    #[cfg(unix)]
    #[test]
    fn echo_results_require_explicit_transfer_and_new_cards_are_not_authorized() {
        let mut board = TaskBoard::default();
        let id = board
            .enqueue(
                "Echo".into(),
                "literal; $(no shell)".into(),
                config("/bin/echo"),
            )
            .unwrap();
        board.review_pending().unwrap();
        board
            .enqueue("Later".into(), "later".into(), config("/bin/echo"))
            .unwrap();
        assert_eq!(board.run_reviewed().unwrap(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while board.running_count() != 0 {
            board.poll();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(board.cards()[0].status(), TaskStatus::Succeeded);
        assert_eq!(board.cards()[1].status(), TaskStatus::NeedsReview);
        assert!(board.take_context().is_none());
        board.stage_context(id).unwrap();
        assert!(
            board
                .take_context()
                .unwrap()
                .contains("literal; $(no shell)")
        );
        assert!(board.stage_context(id).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn independent_runners_overlap_and_cancel_all_never_starts_queued_work() {
        let mut board = TaskBoard::default();
        for n in 0..4 {
            board
                .enqueue(format!("Sleep {n}"), "2".into(), config("/bin/sleep"))
                .unwrap();
        }
        board.review_pending().unwrap();
        board.run_reviewed().unwrap();
        assert_eq!(board.running_count(), 2);
        assert_eq!(board.cards.iter().filter(|c| c.runner.is_busy()).count(), 2);
        assert_eq!(
            board
                .cards
                .iter()
                .filter(|c| c.status == TaskStatus::Queued)
                .count(),
            2
        );
        assert!(board.set_concurrency(1).is_err());
        let running = board.cards[0].id;
        board.cancel(running);
        assert_eq!(board.cards[1].status, TaskStatus::Running);
        board.cancel_all();
        finish(&mut board);
        assert!(
            board
                .cards
                .iter()
                .all(|c| c.status == TaskStatus::Cancelled)
        );
        assert!(
            board.cards[2..]
                .iter()
                .all(|c| c.runner.execution_id() == 0)
        );
    }
    #[cfg(unix)]
    #[test]
    fn full_echo_batch_refills_slots_and_retains_only_eight_results() {
        let mut board = TaskBoard::default();
        board.set_concurrency(4).unwrap();
        for n in 0..MAX_OUTSTANDING {
            board
                .enqueue(
                    format!("Echo {n}"),
                    format!("literal result {n}"),
                    config("/bin/echo"),
                )
                .unwrap();
        }
        board.review_pending().unwrap();
        assert_eq!(board.run_reviewed().unwrap(), MAX_OUTSTANDING);
        let deadline = Instant::now() + Duration::from_secs(5);
        while board.outstanding_count() > 0 {
            board.poll();
            assert!(board.running_count() <= 4);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(board.cards.len(), MAX_RETAINED);
        assert!(
            board
                .cards
                .iter()
                .all(|c| c.status == TaskStatus::Succeeded)
        );
        assert!(
            board
                .cards
                .iter()
                .any(|c| c.output().contains("literal result 15"))
        );
    }
    #[cfg(unix)]
    #[test]
    fn dropping_board_cancels_and_joins_owned_workers() {
        let start = Instant::now();
        {
            let mut board = TaskBoard::default();
            for n in 0..2 {
                board
                    .enqueue(format!("Cleanup {n}"), "30".into(), config("/bin/sleep"))
                    .unwrap();
            }
            board.review_pending().unwrap();
            board.run_reviewed().unwrap();
            assert_eq!(board.running_count(), 2);
            thread::sleep(Duration::from_millis(50));
        }
        // Fixture timeout is 3 seconds. Dropping must actively cancel instead
        // of waiting for that timeout or leaving workers alive in background.
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn excerpts_keep_utf8_and_control_data_quoted_with_head_and_tail() {
        let raw = format!(
            "START provenance {} END findings",
            "猫\0\n\\\"".repeat(20000)
        );
        let clipped = transfer_excerpt(&raw);
        assert!(clipped.len() <= MAX_CONTEXT_BYTES);
        assert!(clipped.contains("untrusted quoted data; clipped; middle omitted"));
        let (head, tail) = clipped
            .split_once("Head: ")
            .unwrap()
            .1
            .split_once("\nTail: ")
            .unwrap();
        let head: String = serde_json::from_str(head).unwrap();
        let tail: String = serde_json::from_str(tail).unwrap();
        assert!(raw.starts_with(&head) && head.contains("START provenance"));
        assert!(raw.ends_with(&tail) && tail.contains("END findings"));
    }
    #[cfg(unix)]
    #[test]
    fn oversized_result_rejects_small_draft_capacity_then_transfers_once() {
        let mut scope = config("/usr/bin/python3");
        scope.args = vec![
            "-c".into(),
            "print('START provenance ' + '猫' * 30000 + ' END findings')".into(),
        ];
        let mut board = TaskBoard::default();
        let id = board
            .enqueue("Large output".into(), "literal task".into(), scope)
            .unwrap();
        board.review_pending().unwrap();
        board.run_reviewed().unwrap();
        finish(&mut board);
        board.stage_context(id).unwrap();
        let len = board.peek_context_len().unwrap();
        assert!(len <= MAX_CONTEXT_BYTES);
        assert!(board.try_take_context(len - 1).is_none());
        assert_eq!(board.peek_context_len(), Some(len));
        assert!(board.cards[0].can_add_context());
        board.stage_context(id).unwrap(); // Same card retries without losing data.
        let transferred = board.try_take_context(MAX_CONTEXT_BYTES).unwrap();
        assert!(transferred.contains("clipped; middle omitted"));
        assert!(transferred.contains("END findings"));
        assert!(board.peek_context_len().is_none());
        assert!(!board.cards[0].can_add_context());
        assert!(board.try_take_context(MAX_CONTEXT_BYTES).is_none());
    }
    #[test]
    fn queue_and_retention_are_bounded_and_provider_proposals_do_not_execute() {
        let mut board = TaskBoard::default();
        assert!(board.set_concurrency(0).is_err());
        assert!(board.set_concurrency(5).is_err());
        for n in 0..MAX_OUTSTANDING {
            board
                .enqueue(format!("Task {n}"), "echo".into(), config("/bin/echo"))
                .unwrap();
        }
        assert!(
            board
                .enqueue("Overflow".into(), "x".into(), config("/bin/echo"))
                .is_err()
        );
        board.cancel_all();
        assert_eq!(board.cards.len(), MAX_RETAINED);
        board.clear_finished();
        assert!(board.cards.is_empty());
        board
            .enqueue_proposal(
                &TaskProposal {
                    title: "Suggestion".into(),
                    provider: Provider::Custom,
                    prompt: "hello".into(),
                },
                config("/bin/echo"),
            )
            .unwrap();
        board.poll();
        assert_eq!(board.cards[0].runner.execution_id(), 0);
        assert_eq!(board.cards[0].status, TaskStatus::NeedsReview);
    }
}
