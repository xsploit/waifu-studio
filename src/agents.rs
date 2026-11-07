//! Manually reviewed external CLI tasks. No shell, automatic tools, or saved secrets.
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const MAX_PROMPT: usize = 32 * 1024;
const MAX_OUTPUT: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentProvider {
    #[default]
    Codex,
    Claude,
    Pi,
    Custom,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    #[default]
    ReadOnly,
    FileEdits,
}
impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::FileEdits => "Allow file edits",
        }
    }
}
impl AgentProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex CLI",
            Self::Claude => "Claude Code",
            Self::Pi => "Pi",
            Self::Custom => "Custom executable",
        }
    }
    pub fn command(self) -> (&'static str, Vec<String>) {
        self.command_with_access(Access::ReadOnly)
    }
    pub fn command_with_access(self, access: Access) -> (&'static str, Vec<String>) {
        let file_edits = access == Access::FileEdits;
        let (exe, args): (&str, Vec<&str>) = match self {
            Self::Codex => (
                "codex",
                vec![
                    "exec",
                    "--skip-git-repo-check",
                    "--json",
                    "--sandbox",
                    if file_edits {
                        "workspace-write"
                    } else {
                        "read-only"
                    },
                ],
            ),
            Self::Claude => (
                "claude",
                vec![
                    "--safe-mode",
                    "--strict-mcp-config",
                    "--print",
                    "--permission-mode",
                    if file_edits { "acceptEdits" } else { "plan" },
                    "--tools",
                    if file_edits {
                        "Read,Glob,Grep,Edit,Write"
                    } else {
                        "Read,Glob,Grep"
                    },
                    "--no-session-persistence",
                ],
            ),
            Self::Pi => (
                "pi",
                vec![
                    "--print",
                    "--no-session",
                    "--no-extensions",
                    "--no-skills",
                    "--no-mcp",
                    "--tools",
                    if file_edits {
                        "read,grep,find,ls,edit,write"
                    } else {
                        "read,grep,find,ls"
                    },
                    "--exclude-tools",
                    "mcp__*",
                ],
            ),
            Self::Custom => ("", vec![]),
        };
        (exe, args.iter().map(|s| (*s).into()).collect())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    #[default]
    Local,
    Ssh,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: String,
    pub working_dir: String,
}
impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: String::new(),
            port: 22,
            identity_file: String::new(),
            working_dir: String::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Task {
    transport: Transport,
    ssh: SshConfig,
    provider: AgentProvider,
    access: Access,
    executable: String,
    args: Vec<String>,
    directory: PathBuf,
    prompt: String,
    timeout: u64,
}

/// Frozen host configuration for a companion tool request. The model supplies
/// only a prompt; it cannot change executable, arguments, directory or access.
#[derive(Clone)]
pub struct CompanionAgent(Task);
impl CompanionAgent {
    pub fn run(&self, prompt: &str, cancel: Arc<AtomicBool>) -> Result<String, String> {
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT || prompt.contains('\0') {
            return Err("Task must contain 1–32768 UTF-8 bytes".into());
        }
        let mut task = self.0.clone();
        task.prompt = prompt.into();
        let output = Arc::new(Mutex::new(Vec::new()));
        let (status, success) = execute(task, cancel, output.clone());
        Ok(serde_json::json!({"success":success,"status":status,
            "output":bounded_text(&output.lock().unwrap())})
        .to_string())
    }
}
struct Review {
    task: Task,
    display: String,
}
struct Running {
    cancel: Arc<AtomicBool>,
    output: Arc<Mutex<Vec<u8>>>,
    receiver: mpsc::Receiver<(String, bool)>,
    thread: Option<JoinHandle<()>>,
    observed: usize,
    origin: String,
}
#[derive(Serialize, Deserialize)]
struct AgentPreferences {
    transport: Transport,
    ssh: SshConfig,
    provider: AgentProvider,
    access: Access,
    executable: String,
    args: Vec<String>,
    working_dir: String,
    timeout_seconds: u64,
    #[serde(default)]
    codex_model: String,
    #[serde(default)]
    claude_model: String,
}
impl From<&AgentState> for AgentPreferences {
    fn from(state: &AgentState) -> Self {
        Self {
            transport: state.transport,
            ssh: state.ssh.clone(),
            provider: state.provider,
            access: state.access,
            executable: state.executable.clone(),
            args: state.args.clone(),
            working_dir: state.working_dir.clone(),
            timeout_seconds: state.timeout_seconds,
            codex_model: state.codex_model.clone(),
            claude_model: state.claude_model.clone(),
        }
    }
}

// Windows npm shims are shell scripts. Resolve the actual native CLI (or its
// JavaScript entry point) so reviewed prompts remain literal argv, without cmd.
fn local_command(name: &str, args: Vec<String>) -> (String, Vec<String>) {
    let paths: Vec<_> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    resolve_local_command(name, args, &paths)
}
fn resolve_local_command(
    name: &str,
    args: Vec<String>,
    paths: &[PathBuf],
) -> (String, Vec<String>) {
    #[cfg(windows)]
    let node = paths
        .iter()
        .map(|p| p.join("node.exe"))
        .find(|p| p.is_file());
    for folder in paths {
        let path = folder.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        if path.is_file() {
            return (path.to_string_lossy().into_owned(), args);
        }
        #[cfg(windows)]
        {
            if name == "codex" {
                for relative in [
                    "node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe",
                    "node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
                    "node_modules/@openai/codex/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
                ] {
                    let path = folder.join(relative);
                    if path.is_file() {
                        return (path.to_string_lossy().into_owned(), args);
                    }
                }
            }
            let entry = match name {
                "codex" => Some("node_modules/@openai/codex/bin/codex.js"),
                "pi" => Some("node_modules/@mariozechner/pi-coding-agent/dist/cli.js"),
                _ => None,
            };
            if let (Some(node), Some(entry)) = (&node, entry) {
                let script = folder.join(entry);
                if script.is_file() {
                    let mut prefixed = vec![script.to_string_lossy().into_owned()];
                    prefixed.extend(args);
                    return (node.to_string_lossy().into_owned(), prefixed);
                }
            }
        }
    }
    (name.into(), args)
}

#[derive(Resource)]
pub struct AgentState {
    pub transport: Transport,
    pub ssh: SshConfig,
    pub provider: AgentProvider,
    pub access: Access,
    pub executable: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub prompt: String,
    pub codex_model: String,
    pub claude_model: String,
    pub output: String,
    pub status: String,
    pub error: Option<String>,
    pub timeout_seconds: u64,
    review: Option<Review>,
    running: Mutex<Option<Running>>,
    completed_context: Option<String>,
    completion: Option<bool>,
    run_serial: u64,
    preferences_path: Option<PathBuf>,
}
impl Default for AgentState {
    fn default() -> Self {
        Self {
            transport: Transport::Local,
            ssh: SshConfig::default(),
            provider: AgentProvider::Codex,
            access: Access::ReadOnly,
            executable: String::new(),
            args: Vec::new(),
            working_dir: String::new(),
            prompt: String::new(),
            codex_model: String::new(),
            claude_model: String::new(),
            output: String::new(),
            status: "No agent task running".into(),
            error: None,
            timeout_seconds: 120,
            review: None,
            running: Mutex::new(None),
            completed_context: None,
            completion: None,
            run_serial: 0,
            preferences_path: None,
        }
    }
}
impl AgentState {
    /// Automatic delegation is restricted to the built-in local read-only
    /// adapters. Editing and SSH retain the existing reviewed task workflow.
    pub fn companion_adapter(&self, provider: AgentProvider) -> Result<CompanionAgent, String> {
        if !matches!(provider, AgentProvider::Codex | AgentProvider::Claude) {
            return Err("Choose Codex or Claude for companion delegation".into());
        }
        let mut state = Self::default();
        state.provider = provider;
        state.working_dir = self.working_dir.clone();
        state.timeout_seconds = self.timeout_seconds;
        state.codex_model = self.codex_model.clone();
        state.claude_model = self.claude_model.clone();
        state.task_config("companion task").map(CompanionAgent)
    }
    pub fn load_preferences(path: PathBuf) -> Self {
        let mut state = Self::default();
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata.is_file()
            && metadata.len() <= 64 * 1024
            && let Ok(bytes) = std::fs::read(&path)
            && let Ok(saved) = serde_json::from_slice::<AgentPreferences>(&bytes)
        {
            state.transport = saved.transport;
            state.ssh = saved.ssh;
            state.provider = saved.provider;
            state.access = saved.access;
            state.executable = saved.executable;
            state.args = saved.args;
            state.working_dir = saved.working_dir;
            state.timeout_seconds = saved.timeout_seconds.clamp(1, 600);
            state.codex_model = saved.codex_model;
            state.claude_model = saved.claude_model;
        }
        if state.working_dir.is_empty()
            && let Some(parent) = path.parent()
        {
            let workspace = parent.join("agent-workspace");
            match std::fs::create_dir_all(&workspace) {
                Ok(()) => state.working_dir = workspace.to_string_lossy().into_owned(),
                Err(error) => {
                    state.error = Some(format!("Cannot prepare agent workspace: {error}"))
                }
            }
        }
        state.preferences_path = Some(path);
        state
    }
    pub fn save_preferences(&mut self) {
        let Some(path) = &self.preferences_path else {
            return;
        };
        let result =
            crate::durable_file::write_json(path, &AgentPreferences::from(&*self), 64 * 1024);
        if let Err(error) = result {
            self.error = Some(format!("Could not save agent settings: {error}"));
        }
    }
    pub fn is_busy(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }
    pub fn disarm(&mut self) {
        self.review = None;
    }
    pub fn execution_scope(&self) -> Result<String, String> {
        Ok(format!("{:?}", self.task_config("scope-validation")?))
    }
    pub fn execution_id(&self) -> u64 {
        self.run_serial
    }
    pub fn completion_success(&self) -> Option<bool> {
        self.completion
    }
    fn task(&self) -> Result<Task, String> {
        self.task_config(&self.prompt)
    }
    fn task_config(&self, prompt: &str) -> Result<Task, String> {
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT || prompt.contains('\0') {
            return Err("Task must contain 1–32768 UTF-8 bytes".into());
        }
        if !(1..=600).contains(&self.timeout_seconds) {
            return Err("Timeout must be 1–600 seconds".into());
        }
        let directory = if self.transport == Transport::Ssh {
            validate_ssh(&self.ssh)?;
            PathBuf::from(&self.ssh.working_dir)
        } else {
            if self.working_dir.trim().is_empty() || self.working_dir.len() > 4096 {
                return Err("Choose an existing working directory".into());
            }
            let directory = std::fs::canonicalize(&self.working_dir)
                .map_err(|_| "Working directory does not exist")?;
            if !directory.is_dir() {
                return Err("Working directory must be a directory".into());
            }
            directory
        };
        let (executable, args) = if self.provider == AgentProvider::Custom {
            (self.executable.clone(), self.args.clone())
        } else {
            let (e, mut a) = self.provider.command_with_access(self.access);
            let model = match self.provider {
                AgentProvider::Codex => self.codex_model.trim(),
                AgentProvider::Claude => self.claude_model.trim(),
                _ => "",
            };
            if model.len() > 256 || model.chars().any(char::is_control) {
                return Err(
                    "Agent model must be at most 256 characters without control characters".into(),
                );
            }
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            if self.transport == Transport::Local {
                local_command(e, a)
            } else {
                (e.into(), a)
            }
        };
        if executable.trim().is_empty()
            || executable.len() > 4096
            || executable.chars().any(char::is_control)
        {
            return Err("Choose an executable name or path".into());
        }
        if args.len() > 64
            || args.iter().any(|a| a.len() > 4096 || a.contains('\0'))
            || args.iter().map(String::len).sum::<usize>() > MAX_PROMPT
        {
            return Err("Arguments exceed size limits".into());
        }
        Ok(Task {
            transport: self.transport,
            ssh: self.ssh.clone(),
            provider: self.provider,
            access: self.access,
            executable,
            args,
            directory,
            prompt: prompt.to_owned(),
            timeout: self.timeout_seconds,
        })
    }
    pub fn review(&mut self) -> Result<(), String> {
        if self.is_busy() {
            return Err("Wait for the current task to stop".into());
        }
        self.review = None;
        let task = self.task()?;
        let mut display = format!(
            "Working directory: {}\nTimeout: {} seconds\nAccess: {}\nExecutable: {}\nArguments (JSON array; final argument is your task): {}\n\nNo local shell is used. Local tasks inherit the standard environment/CLI authentication; SSH tasks use the remote installed CLI/authentication.",
            task.directory.display(),
            task.timeout,
            if task.provider == AgentProvider::Custom {
                "Custom supplied arguments (access preset not enforced)"
            } else {
                task.access.label()
            },
            task.executable,
            serde_json::to_string(
                &task
                    .args
                    .iter()
                    .cloned()
                    .chain((task.provider != AgentProvider::Custom).then_some("--".to_owned()))
                    .chain(std::iter::once(task.prompt.clone()))
                    .collect::<Vec<_>>()
            )
            .map_err(|e| e.to_string())?
        );
        if task.transport == Transport::Ssh {
            let (_, argv) = launch_spec(&task);
            display.push_str(&format!("\n\nSSH transport (no local shell; remote POSIX shell uses individually quoted arguments): ssh {}\nRequires remote GNU timeout and installed agent/auth. Local SSH watchdog allows 20 additional seconds for connection setup and remote termination grace. Host-key verification stays enabled. Local cancellation stops SSH; remote GNU timeout bounds ordinary remote descendants, but independently detached descendants may escape and file changes cannot be rolled back.",serde_json::to_string(&argv).map_err(|e|e.to_string())?));
        }
        self.review = Some(Review { task, display });
        self.error = None;
        self.status = "Review the exact task, then Start".into();
        Ok(())
    }
    pub fn reviewed_command(&self) -> Option<&str> {
        self.review.as_ref().map(|r| r.display.as_str())
    }
    pub fn run(&mut self) -> Result<(), String> {
        if !cfg!(any(unix, windows)) {
            return Err("Coding-agent execution is unavailable on this platform.".into());
        }
        if self.is_busy() {
            return Err("One agent task can run at a time".into());
        }
        let current = self.task()?;
        let review = self
            .review
            .take()
            .ok_or("Review this exact task before Start")?;
        if current != review.task {
            return Err("Task changed; review again before Start".into());
        }
        let origin = format!(
            "{} / transport {:?} {} / executable {} / directory {}",
            current.provider.label(),
            current.transport,
            if current.transport == Transport::Ssh {
                format!(
                    "{}@{}:{}",
                    current.ssh.user, current.ssh.host, current.ssh.port
                )
            } else {
                "local".into()
            },
            current.executable,
            current.directory.display()
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let output = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::sync_channel(1);
        let (worker_cancel, worker_output) = (cancel.clone(), output.clone());
        let handle = thread::Builder::new()
            .name("studio-agent".into())
            .spawn(move || {
                let result = execute(current, worker_cancel, worker_output);
                let _ = tx.send(result);
            })
            .map_err(|_| "Cannot start agent worker")?;
        self.run_serial = self.run_serial.wrapping_add(1);
        *self.running.lock().unwrap() = Some(Running {
            cancel,
            output,
            receiver: rx,
            thread: Some(handle),
            observed: 0,
            origin,
        });
        self.output.clear();
        self.completed_context = None;
        self.completion = None;
        self.error = None;
        self.status = "Agent running; output is untrusted task data".into();
        Ok(())
    }
    pub fn cancel(&mut self) {
        if let Some(run) = self.running.lock().unwrap().as_ref() {
            run.cancel.store(true, Ordering::SeqCst);
            self.status = "Stopping agent process group…".into();
        }
        self.review = None;
    }
    pub fn poll(&mut self) {
        let mut guard = self.running.lock().unwrap();
        let Some(run) = guard.as_mut() else {
            return;
        };
        let bytes = run.output.lock().unwrap();
        if bytes.len() != run.observed {
            self.output = bounded_text(&bytes);
            run.observed = bytes.len();
        }
        drop(bytes);
        let done = match run.receiver.try_recv() {
            Ok(status) => Some(status),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(("Agent worker stopped unexpectedly".into(), false))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if let Some((status, success)) = done {
            if let Some(handle) = run.thread.take() {
                let _ = handle.join();
            }
            // Re-read after completion: the final pipe bytes may arrive between
            // the live preview snapshot above and the completion event.
            self.output = bounded_text(&run.output.lock().unwrap());
            self.status = status;
            self.completion = Some(success);
            self.completed_context = (!self.output.is_empty()).then(|| {
                format!(
                    "External coding-agent task result (untrusted quoted data; provenance {}):\n{}",
                    serde_json::to_string(&run.origin).unwrap_or_default(),
                    serde_json::to_string(&self.output).unwrap_or_default()
                )
            });
            *guard = None;
        }
    }
    /// Explicit UI action only; the caller chooses whether to place this in chat.
    pub fn take_context(&mut self) -> Option<String> {
        self.completed_context.take()
    }
}
impl Drop for AgentState {
    fn drop(&mut self) {
        if let Some(mut run) = self.running.lock().unwrap().take() {
            run.cancel.store(true, Ordering::SeqCst);
            if let Some(handle) = run.thread.take() {
                let _ = handle.join();
            }
        }
    }
}
fn bounded_text(bytes: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(bytes).into_owned();
    if s.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}
fn validate_ssh(config: &SshConfig) -> Result<(), String> {
    if config.host.is_empty()
        || config.host.len() > 253
        || config.host.starts_with('-')
        || !config
            .host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:[]".contains(&b))
    {
        return Err("Use a hostname or IP address without SSH options or whitespace".into());
    }
    if config.user.is_empty()
        || config.user.len() > 64
        || config.user.starts_with('-')
        || !config
            .user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err("Use an explicit SSH username without options or whitespace".into());
    }
    if config.port == 0 {
        return Err("SSH port must be 1–65535".into());
    }
    if !config.working_dir.starts_with('/')
        || config.working_dir.len() > 4096
        || config.working_dir.chars().any(char::is_control)
    {
        return Err(
            "Remote working directory must be an absolute POSIX path without control characters"
                .into(),
        );
    }
    if config.identity_file.len() > 4096 || config.identity_file.chars().any(char::is_control) {
        return Err("Identity path contains control characters or exceeds the limit".into());
    }
    Ok(())
}
fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn launch_spec(task: &Task) -> (String, Vec<String>) {
    let mut agent_args = task.args.clone();
    if task.provider != AgentProvider::Custom {
        agent_args.push("--".into());
    }
    agent_args.push(task.prompt.clone());
    if task.transport == Transport::Local {
        return (task.executable.clone(), agent_args);
    }
    let remote_args = [
        "timeout".to_owned(),
        "--signal=TERM".into(),
        "--kill-after=5s".into(),
        "--".into(),
        format!("{}s", task.timeout),
        task.executable.clone(),
    ]
    .into_iter()
    .chain(agent_args)
    .map(|a| posix_quote(&a))
    .collect::<Vec<_>>()
    .join(" ");
    let remote = format!(
        "cd {} && exec {}",
        posix_quote(&task.ssh.working_dir),
        remote_args
    );
    let mut argv = vec![
        "-T".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-p".into(),
        task.ssh.port.to_string(),
        "-l".into(),
        task.ssh.user.clone(),
    ];
    if !task.ssh.identity_file.is_empty() {
        argv.extend(["-i".into(), task.ssh.identity_file.clone()]);
    }
    argv.extend(["--".into(), task.ssh.host.clone(), remote]);
    ("ssh".into(), argv)
}
struct ProcessGroup {
    child: Child,
    cleaned: bool,
    #[cfg(windows)]
    job: crate::windows_process::Job,
}
impl ProcessGroup {
    fn kill(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        #[cfg(windows)]
        self.job.kill();
        #[cfg(unix)]
        // SAFETY: process_group(0) creates a fresh group whose ID is this child PID.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}
fn drain(
    mut reader: impl Read,
    output: Arc<Mutex<Vec<u8>>>,
    overflow: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
            Ok(n) => {
                let mut data = output.lock().unwrap();
                let available = MAX_OUTPUT.saturating_sub(data.len());
                data.extend_from_slice(&chunk[..n.min(available)]);
                if n > available {
                    overflow.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    }
}
fn execute(task: Task, cancel: Arc<AtomicBool>, output: Arc<Mutex<Vec<u8>>>) -> (String, bool) {
    if cancel.load(Ordering::SeqCst) {
        return ("Agent cancelled before launch".into(), false);
    }
    let (executable, arguments) = launch_spec(&task);
    let mut command = Command::new(executable);
    command.args(arguments);
    if task.transport == Transport::Local {
        command.current_dir(&task.directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(windows))]
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return (format!("Cannot start selected CLI: {e}"), false),
    };
    #[cfg(windows)]
    let (child, job) = match crate::windows_process::Job::spawn(&mut command) {
        Ok(result) => result,
        Err(error) => return (format!("Cannot start selected CLI: {error}"), false),
    };
    let mut process = ProcessGroup {
        child,
        cleaned: false,
        #[cfg(windows)]
        job,
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = process.child.stdout.take().unwrap();
    let stderr = process.child.stderr.take().unwrap();
    #[cfg(windows)]
    let (stdout, stderr) = (
        crate::windows_process::Reader(stdout),
        crate::windows_process::Reader(stderr),
    );
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // SAFETY: descriptors are owned live pipes; preserve existing flags.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return ("Cannot configure bounded output pipes".into(), false);
            }
        }
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let (o, e, stop) = (output.clone(), overflow.clone(), stopped.clone());
    let out_reader = thread::spawn(move || drain(stdout, o, e, stop));
    let (o, e, stop) = (output, overflow.clone(), stopped.clone());
    let err_reader = thread::spawn(move || drain(stderr, o, e, stop));
    let start = Instant::now();
    // Allow SSH connection setup + GNU timeout's kill grace before local teardown.
    let local_limit = task.timeout
        + if task.transport == Transport::Ssh {
            20
        } else {
            0
        };
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            break ("Agent cancelled; processes stopped".into(), false);
        }
        if overflow.load(Ordering::SeqCst) {
            break ("Agent stopped: 256 KiB combined output limit".into(), false);
        }
        if start.elapsed() >= Duration::from_secs(local_limit) {
            break ("Agent stopped: wall-clock timeout".into(), false);
        }
        match process.child.try_wait() {
            Ok(Some(status)) => break (format!("Agent exited: {status}"), status.success()),
            Ok(None) => {}
            Err(e) => break (format!("Cannot wait for agent: {e}"), false),
        }
        thread::sleep(Duration::from_millis(20));
    };
    // Also kill descendants after a parent exits, so inherited output pipes close.
    process.kill();
    stopped.store(true, Ordering::SeqCst);
    let _ = out_reader.join();
    let _ = err_reader.join();
    if overflow.load(Ordering::SeqCst) {
        ("Agent stopped: 256 KiB combined output limit".into(), false)
    } else {
        status
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_delegation_freezes_local_read_only_scope_and_keeps_model_overrides() {
        let mut state = AgentState::default();
        state.working_dir = std::env::temp_dir().to_string_lossy().into_owned();
        state.transport = Transport::Ssh;
        state.access = Access::FileEdits;
        state.executable = "arbitrary-command".into();
        state.claude_model = "sonnet".into();
        let adapter = state.companion_adapter(AgentProvider::Claude).unwrap();
        assert_eq!(adapter.0.transport, Transport::Local);
        assert_eq!(adapter.0.access, Access::ReadOnly);
        assert!(
            !adapter
                .0
                .args
                .iter()
                .any(|arg| arg.contains("Edit") || arg.contains("Write"))
        );
        assert!(
            adapter
                .0
                .args
                .windows(2)
                .any(|args| args == ["--model", "sonnet"])
        );
        assert!(state.companion_adapter(AgentProvider::Custom).is_err());
    }

    #[test]
    fn agent_preferences_restore_scope_without_saving_task_or_output() {
        let root =
            std::env::temp_dir().join(format!("waifu-agent-preferences-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("agents.json");
        let mut state = AgentState::load_preferences(path.clone());
        assert!(std::path::Path::new(&state.working_dir).is_dir());
        state.provider = AgentProvider::Claude;
        state.claude_model = "sonnet".into();
        state.codex_model = "test-codex-model".into();
        state.timeout_seconds = 90;
        state.prompt = "private pending task".into();
        state.output = "private tool output".into();
        state.save_preferences();
        assert!(state.error.is_none());
        let restored = AgentState::load_preferences(path.clone());
        assert_eq!(restored.provider, AgentProvider::Claude);
        assert_eq!(restored.working_dir, state.working_dir);
        assert_eq!(restored.claude_model, "sonnet");
        assert_eq!(restored.codex_model, "test-codex-model");
        let task = restored.task_config("read fixture").unwrap();
        assert!(task.args.windows(2).any(|p| p == ["--model", "sonnet"]));
        assert!(task.args.contains(&"--safe-mode".into()) && !task.args.contains(&"--bare".into()));
        assert_eq!(restored.timeout_seconds, 90);
        assert!(restored.prompt.is_empty() && restored.output.is_empty());
        assert!(!std::fs::read_to_string(path).unwrap().contains("private"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn windows_codex_npm_install_resolves_native_binary_and_literal_arguments() {
        let root =
            std::env::temp_dir().join(format!("waifu-cli-resolution-{}", uuid::Uuid::new_v4()));
        let executable = root.join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"fixture only").unwrap();
        let args = vec!["exec".into(), "literal & $(input)".into()];
        let later = root.join("later-on-path");
        std::fs::create_dir(&later).unwrap();
        std::fs::write(later.join("codex.exe"), b"later desktop launcher").unwrap();
        let (resolved, preserved) =
            resolve_local_command("codex", args.clone(), &[root.clone(), later]);
        assert_eq!(PathBuf::from(resolved), executable);
        assert_eq!(preserved, args);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[allow(clippy::field_reassign_with_default)] // Drop prevents struct-update syntax.
    fn state() -> AgentState {
        let mut state = AgentState::default();
        state.provider = AgentProvider::Custom;
        state.executable = "/usr/bin/printf".into();
        state.args = vec!["%s".into()];
        state.working_dir = std::env::temp_dir().to_string_lossy().into_owned();
        state.prompt = "literal $(command); 🐈".into();
        state
    }
    fn finish(s: &mut AgentState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.is_busy() {
            s.poll();
            assert!(Instant::now() < deadline, "worker timeout");
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    #[cfg(unix)]
    fn requires_exact_review_and_no_shell() {
        let mut s = state();
        assert!(s.run().is_err());
        s.review().unwrap();
        s.prompt.push('!');
        assert!(s.run().is_err());
        s.review().unwrap();
        s.run().unwrap();
        finish(&mut s);
        assert_eq!(s.output, s.prompt);
        assert_eq!(s.completion_success(), Some(true));
        assert!(s.take_context().is_some());
        assert!(s.take_context().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn changed_access_invalidates_exact_review() {
        let mut s = state();
        s.review().unwrap();
        s.access = Access::FileEdits;
        assert!(s.run().is_err());
        assert!(s.reviewed_command().is_none());
    }
    #[test]
    fn permission_presets_keep_readonly_default_and_enable_only_explicit_edits() {
        assert_eq!(AgentState::default().access, Access::ReadOnly);
        for provider in [
            AgentProvider::Codex,
            AgentProvider::Claude,
            AgentProvider::Pi,
        ] {
            let (_, read) = provider.command();
            let (_, edit) = provider.command_with_access(Access::FileEdits);
            assert_ne!(read, edit);
            assert!(
                !edit
                    .iter()
                    .any(|arg| arg.contains("bypass") || arg.contains("yolo"))
            );
        }
        let (_, codex) = AgentProvider::Codex.command_with_access(Access::FileEdits);
        assert!(codex.iter().any(|arg| arg == "workspace-write"));
        let (_, claude) = AgentProvider::Claude.command_with_access(Access::FileEdits);
        assert!(claude.iter().any(|arg| arg == "acceptEdits"));
        assert!(!claude.iter().any(|arg| arg.contains("Bash")));
        let (_, pi) = AgentProvider::Pi.command_with_access(Access::FileEdits);
        assert!(pi.iter().any(|arg| arg == "read,grep,find,ls,edit,write"));
        assert!(pi.iter().any(|arg| arg == "--no-mcp"));
    }
    #[test]
    fn ssh_arguments_quote_adversarial_task_and_keep_strict_host_verification() {
        let mut s = state();
        s.transport = Transport::Ssh;
        s.ssh = SshConfig {
            host: "example.invalid".into(),
            user: "worker".into(),
            port: 2222,
            identity_file: "/keys/a key".into(),
            working_dir: "/remote/project's folder".into(),
        };
        s.prompt = "say 'hello'; $(touch forbidden)\nnext line".into();
        s.working_dir = "/not/local/and/not/validated".into();
        let task = s.task().unwrap();
        let (exe, args) = launch_spec(&task);
        assert_eq!(exe, "ssh");
        assert!(args.contains(&"BatchMode=yes".into()));
        assert!(args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"ConnectTimeout=10".into()));
        assert!(args.contains(&"/keys/a key".into()));
        let remote = args.last().unwrap();
        assert!(remote.contains(&posix_quote(&s.prompt)));
        assert!(remote.starts_with(&format!("cd {} && exec ", posix_quote(&s.ssh.working_dir))));
        assert!(remote.contains("'timeout' '--signal=TERM' '--kill-after=5s' '--' '120s'"));
        assert_eq!(posix_quote("a'b"), "'a'\"'\"'b'");
        s.review().unwrap();
        s.ssh.port = 22;
        assert!(s.task().unwrap() != s.review.as_ref().unwrap().task);
    }
    #[test]
    fn ssh_rejects_option_hosts_and_control_paths() {
        let mut ssh = SshConfig {
            host: "-oProxyCommand=bad".into(),
            user: "worker".into(),
            working_dir: "/remote".into(),
            ..Default::default()
        };
        assert!(validate_ssh(&ssh).is_err());
        ssh.host = "example.invalid".into();
        ssh.user = "worker\ncommand".into();
        assert!(validate_ssh(&ssh).is_err());
        ssh.user = "worker".into();
        ssh.working_dir = "/remote\ncommand".into();
        assert!(validate_ssh(&ssh).is_err());
        ssh.working_dir = "/remote".into();
        assert!(validate_ssh(&ssh).is_ok());
    }
    #[test]
    fn execution_scope_excludes_task_but_binds_permissions_and_transport() {
        let mut s = state();
        let scope = s.execution_scope().unwrap();
        s.prompt.clear();
        assert_eq!(scope, s.execution_scope().unwrap());
        s.access = Access::FileEdits;
        assert_ne!(scope, s.execution_scope().unwrap());
        s.transport = Transport::Ssh;
        s.ssh = SshConfig {
            host: "example.invalid".into(),
            user: "worker".into(),
            working_dir: "/remote".into(),
            ..Default::default()
        };
        assert_ne!(scope, s.execution_scope().unwrap());
        assert_eq!(s.completion_success(), None);
    }
    #[test]
    fn rejects_non_directory_and_oversized_prompt() {
        let mut s = state();
        s.working_dir = "/definitely/no/such/working/directory".into();
        assert!(s.review().is_err());
        s = state();
        s.prompt = "a".repeat(MAX_PROMPT + 1);
        assert!(s.review().is_err());
    }
    #[test]
    fn bounded_utf8_output() {
        let text = vec![0xff; MAX_OUTPUT];
        let result = bounded_text(&text);
        assert!(result.len() <= MAX_OUTPUT);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn simultaneous_stdout_stderr_are_drained_and_combined_output_is_bounded() {
        let mut s = state();
        s.executable = "/usr/bin/python3".into();
        s.args = vec![
            "-c".into(),
            "import os; os.write(1, b'a'*200000); os.write(2, b'b'*200000)".into(),
        ];
        s.review().unwrap();
        s.run().unwrap();
        finish(&mut s);
        assert!(s.output.len() <= MAX_OUTPUT);
        assert!(s.status.contains("output limit"));
    }
    #[cfg(unix)]
    #[test]
    fn cancel_and_timeout_finish_workers() {
        for cancel in [true, false] {
            let mut s = state();
            s.executable = "/usr/bin/sleep".into();
            s.args = vec![];
            s.prompt = "30".into();
            s.timeout_seconds = 1;
            s.review().unwrap();
            s.run().unwrap();
            if cancel {
                s.cancel();
            }
            finish(&mut s);
            assert_eq!(s.completion_success(), Some(false));
            assert!(
                s.status
                    .contains(if cancel { "cancelled" } else { "timeout" })
            );
        }
    }
}
