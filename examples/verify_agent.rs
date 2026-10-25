//! Small, explicit authenticated acceptance task through the production CLI runner.
use std::time::{Duration, Instant};
use waifu_studio::agents::{AgentProvider, AgentState};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let provider = match args.next().as_deref() {
        Some("codex") => AgentProvider::Codex,
        Some("claude") => AgentProvider::Claude,
        _ => return Err("Usage: verify_agent codex|claude WORKSPACE_WITH_readiness.txt".into()),
    };
    let directory = args.next().ok_or("Supply the fixture workspace")?;
    let mut state = AgentState::default();
    state.provider = provider;
    if provider == AgentProvider::Claude {
        state.claude_model = "sonnet".into();
    }
    state.working_dir = directory;
    state.timeout_seconds = 120;
    state.prompt = "Read readiness.txt in the current working directory and reply with its single verification word. Do not edit any files or run any other task.".into();
    state.review()?;
    state.run()?;
    let started = Instant::now();
    while state.is_busy() && started.elapsed() < Duration::from_secs(140) {
        state.poll();
        std::thread::sleep(Duration::from_millis(30));
    }
    if state.is_busy() {
        state.cancel();
        return Err("Agent did not finish within its acceptance deadline".into());
    }
    if state.completion_success() != Some(true) || !state.output.contains("WAIFU_AGENT_READY") {
        if let Some(path) = std::env::var_os("WAIFU_AGENT_VERIFY_LOG") {
            std::fs::write(path, &state.output).map_err(|e| e.to_string())?;
        }
        return Err(format!("Agent acceptance failed: {}", state.status));
    }
    println!(
        "{}",
        serde_json::json!({"provider": provider.label(), "authenticated_read_task": "ok", "seconds": started.elapsed().as_secs_f64()})
    );
    Ok(())
}
