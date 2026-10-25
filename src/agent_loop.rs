//! Opt-in bounded orchestration. No I/O, worker commands, model calls or credentials.
//! The host owns an immutable worker-config snapshot and must enforce its revision.
use crate::delegation::{Provider, TaskProposal, parse_proposals};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopToken {
    pub generation: u64,
    pub step: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopStatus {
    Off,
    AwaitingAssistant,
    WorkerRunning,
    Cooldown,
    Stopped(StopReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Cancelled,
    CharacterChanged,
    NoProposals,
    InvalidProposal,
    ProviderMismatch,
    WorkerFailed,
    StepLimit,
    AssistantFailed,
}

#[derive(Clone, Debug)]
pub struct WorkerStep {
    pub token: LoopToken,
    pub character_id: Uuid,
    pub worker_config_revision: u64,
    pub proposal: TaskProposal,
}

#[derive(Debug)]
pub struct AgentLoop {
    character_id: Option<Uuid>,
    generation: u64,
    goal: String,
    provider: Provider,
    worker_config_revision: u64,
    max_steps: u8,
    steps: u8,
    cooldown: Duration,
    ready_at: Duration,
    status: LoopStatus,
}
impl Default for AgentLoop {
    fn default() -> Self {
        Self {
            character_id: None,
            generation: 0,
            goal: String::new(),
            provider: Provider::Codex,
            worker_config_revision: 0,
            max_steps: 1,
            steps: 0,
            cooldown: Duration::from_secs(5),
            ready_at: Duration::ZERO,
            status: LoopStatus::Off,
        }
    }
}
impl AgentLoop {
    pub fn status(&self) -> &LoopStatus {
        &self.status
    }
    pub fn goal(&self) -> &str {
        &self.goal
    }
    pub fn steps(&self) -> u8 {
        self.steps
    }
    pub fn max_steps(&self) -> u8 {
        self.max_steps
    }
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            LoopStatus::AwaitingAssistant | LoopStatus::WorkerRunning | LoopStatus::Cooldown
        )
    }
    fn token(&self) -> LoopToken {
        LoopToken {
            generation: self.generation,
            step: self.steps,
        }
    }
    fn bump_generation(&mut self) -> Result<(), String> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Loop generation exhausted")?;
        Ok(())
    }
    /// Call only from explicit Start UI after the user selects goal and worker/access/cwd.
    /// `now` is monotonic elapsed time from a single host clock, never model supplied.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        character_id: Uuid,
        goal: String,
        provider: Provider,
        worker_config_revision: u64,
        max_steps: u8,
        cooldown: Duration,
        now: Duration,
    ) -> Result<LoopToken, String> {
        if self.is_active() {
            return Err("Stop the active loop before starting another".into());
        }
        if goal.trim().is_empty() || goal.len() > 8192 {
            return Err("Loop goal must contain 1–8192 bytes".into());
        }
        self.bump_generation()?;
        self.character_id = Some(character_id);
        self.goal = goal;
        self.provider = provider;
        self.worker_config_revision = worker_config_revision;
        self.max_steps = max_steps.clamp(1, 8);
        self.steps = 0;
        self.cooldown = cooldown.clamp(Duration::from_secs(1), Duration::from_secs(3600));
        self.ready_at = now;
        self.status = LoopStatus::AwaitingAssistant;
        Ok(self.token())
    }
    /// Accept only an assistant completion for the outstanding loop request.
    /// Multiple proposals are not a queue: only the first can run; extras are dropped.
    /// Every supplied provider must match the explicit Start selection.
    pub fn accept_assistant(
        &mut self,
        token: LoopToken,
        response: &str,
    ) -> Result<Option<WorkerStep>, String> {
        self.require(token, LoopStatus::AwaitingAssistant)?;
        let proposals = match parse_proposals(response) {
            Ok(tasks) => tasks,
            Err(error) => {
                self.stop(StopReason::InvalidProposal);
                return Err(error);
            }
        };
        if proposals.iter().any(|task| task.provider != self.provider) {
            self.stop(StopReason::ProviderMismatch);
            return Err("Proposal cannot change the selected worker provider".into());
        }
        let Some(proposal) = proposals.into_iter().next() else {
            self.stop(StopReason::NoProposals);
            return Ok(None);
        };
        if self.steps >= self.max_steps {
            self.stop(StopReason::StepLimit);
            return Err(
                "Task budget exhausted; companion must provide a final answer without tools".into(),
            );
        }
        self.steps += 1;
        self.status = LoopStatus::WorkerRunning;
        Ok(Some(WorkerStep {
            token: self.token(),
            character_id: self.character_id.expect("Start establishes character"),
            worker_config_revision: self.worker_config_revision,
            proposal,
        }))
    }
    /// Host first captures bounded untrusted feedback, then marks the worker result.
    /// A failed/cancelled/timed-out worker must pass success=false.
    pub fn complete_worker(
        &mut self,
        token: LoopToken,
        success: bool,
        now: Duration,
    ) -> Result<(), String> {
        self.require(token, LoopStatus::WorkerRunning)?;
        if !success {
            self.stop(StopReason::WorkerFailed);
        } else {
            // Even the final worker result gets one companion summary request.
            // accept_assistant still rejects any task beyond the fixed budget.
            self.ready_at = now.saturating_add(self.cooldown);
            self.status = LoopStatus::Cooldown;
        }
        Ok(())
    }
    /// Returns once per cooldown; host sends goal plus explicitly quoted feedback
    /// back to the companion and retains the token for that request's completion.
    pub fn next_chat(&mut self, now: Duration) -> Option<LoopToken> {
        if self.status != LoopStatus::Cooldown || now < self.ready_at {
            return None;
        }
        self.status = LoopStatus::AwaitingAssistant;
        Some(self.token())
    }
    pub fn assistant_failed(&mut self, token: LoopToken) -> Result<(), String> {
        self.require(token, LoopStatus::AwaitingAssistant)?;
        self.stop(StopReason::AssistantFailed);
        Ok(())
    }
    /// Host must also cancel/reap any actual worker and outstanding model request.
    pub fn cancel(&mut self) {
        self.stop(StopReason::Cancelled);
    }
    /// Call on character selection changes; never transfer an active loop's consent.
    pub fn reset_for_character(&mut self, character_id: Uuid) {
        if self.character_id != Some(character_id) {
            self.stop(StopReason::CharacterChanged);
            self.character_id = Some(character_id);
            self.goal.clear();
            self.steps = 0;
        }
    }
    fn require(&self, token: LoopToken, status: LoopStatus) -> Result<(), String> {
        if self.token() != token || self.status != status {
            Err("Stale or out-of-order loop event".into())
        } else {
            Ok(())
        }
    }
    fn stop(&mut self, reason: StopReason) {
        // A terminal state rejects outstanding tokens even if generation is exhausted.
        let _ = self.bump_generation();
        self.status = LoopStatus::Stopped(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(provider: &str) -> String {
        format!(
            "```waifu_tasks\n{{\"tasks\":[{{\"title\":\"Inspect\",\"provider\":\"{provider}\",\"prompt\":\"Read source\"}}]}}\n```"
        )
    }
    fn start(c: &mut AgentLoop, limit: u8) -> LoopToken {
        c.start(
            Uuid::nil(),
            "Review project".into(),
            Provider::Codex,
            42,
            limit,
            Duration::from_secs(5),
            Duration::ZERO,
        )
        .unwrap()
    }
    #[test]
    fn default_off_and_single_step_budget() {
        let mut c = AgentLoop::default();
        assert_eq!(c.status(), &LoopStatus::Off);
        assert!(
            c.accept_assistant(
                LoopToken {
                    generation: 0,
                    step: 0
                },
                &response("codex")
            )
            .is_err()
        );
        let chat = start(&mut c, 0);
        assert_eq!(c.max_steps(), 1);
        let worker = c
            .accept_assistant(chat, &response("codex"))
            .unwrap()
            .unwrap();
        assert_eq!(worker.worker_config_revision, 42);
        assert!(c.accept_assistant(chat, &response("codex")).is_err());
        c.complete_worker(worker.token, true, Duration::ZERO)
            .unwrap();
        assert_eq!(c.status(), &LoopStatus::Cooldown);
        let summary = c.next_chat(Duration::from_secs(999)).unwrap();
        assert!(
            c.accept_assistant(summary, "The requested check passed.")
                .unwrap()
                .is_none()
        );
        assert!(!c.is_active());
        assert_eq!(c.steps(), 1);
    }
    #[test]
    fn final_summary_cannot_start_an_extra_worker() {
        let mut c = AgentLoop::default();
        let chat = start(&mut c, 1);
        let worker = c
            .accept_assistant(chat, &response("codex"))
            .unwrap()
            .unwrap();
        c.complete_worker(worker.token, true, Duration::ZERO)
            .unwrap();
        let final_chat = c.next_chat(Duration::from_secs(5)).unwrap();
        assert!(c.accept_assistant(final_chat, &response("codex")).is_err());
        assert_eq!(c.steps(), 1);
        assert_eq!(c.status(), &LoopStatus::Stopped(StopReason::StepLimit));
    }
    #[test]
    fn cooldown_feedback_then_next_proposal() {
        let mut c = AgentLoop::default();
        let chat = start(&mut c, 2);
        let worker = c
            .accept_assistant(chat, &response("codex"))
            .unwrap()
            .unwrap();
        assert!(c.next_chat(Duration::from_secs(999)).is_none());
        c.complete_worker(worker.token, true, Duration::from_secs(10))
            .unwrap();
        assert!(c.next_chat(Duration::from_secs(14)).is_none());
        let next = c.next_chat(Duration::from_secs(15)).unwrap();
        assert!(c.next_chat(Duration::from_secs(20)).is_none());
        assert!(
            c.complete_worker(worker.token, true, Duration::from_secs(20))
                .is_err()
        );
        assert!(c.accept_assistant(next, "All done.").unwrap().is_none());
        assert_eq!(c.status(), &LoopStatus::Stopped(StopReason::NoProposals));
    }
    #[test]
    fn mismatch_malformed_failure_and_cancel_stop() {
        for bad in [response("custom"), "```waifu_tasks\nnot json\n```".into()] {
            let mut c = AgentLoop::default();
            let token = start(&mut c, 8);
            assert!(c.accept_assistant(token, &bad).is_err());
            assert!(!c.is_active());
        }
        let mut c = AgentLoop::default();
        let chat = start(&mut c, 255);
        assert_eq!(c.max_steps(), 8);
        let worker = c
            .accept_assistant(chat, &response("codex"))
            .unwrap()
            .unwrap();
        c.complete_worker(worker.token, false, Duration::ZERO)
            .unwrap();
        assert!(!c.is_active());
        let chat = start(&mut c, 2);
        c.cancel();
        assert!(c.accept_assistant(chat, &response("codex")).is_err());
    }
    #[test]
    fn character_change_and_restart_invalidate_old_results() {
        let mut c = AgentLoop::default();
        let old = start(&mut c, 2);
        c.reset_for_character(Uuid::from_u128(1));
        assert!(!c.is_active());
        assert!(c.goal().is_empty());
        let current = start(&mut c, 2);
        assert_ne!(old, current);
        assert!(c.accept_assistant(old, &response("codex")).is_err());
        c.assistant_failed(current).unwrap();
        assert_eq!(
            c.status(),
            &LoopStatus::Stopped(StopReason::AssistantFailed)
        );
    }
}
