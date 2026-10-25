//! Bounded stdio process transport. Host protocol/authorization is separate.
//! External programs execute local code; this is supervision, not an OS sandbox.
#[cfg(all(test, windows))]
#[path = "extension_process_windows_tests.rs"]
mod windows_tests;
#[cfg(not(windows))]
use std::process::Command;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const MAX_LINE: usize = 64 * 1024;
const QUEUE: usize = 32;
#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub data_directory: PathBuf,
    /// One-shot hooks have a lifetime bound; persistent services use None.
    pub lifetime: Option<Duration>,
}
#[derive(Debug)]
pub enum ProcessEvent {
    Started { pid: u32 },
    Line(Vec<u8>),
    Log(String),
    Exited { code: Option<i32>, reason: String },
}
#[derive(Clone, Debug)]
pub struct ProcessTermination {
    /// Publication after completed supervision/reaping, not initial OS-exit observation.
    pub observed_at: Instant,
    pub code: Option<i32>,
    pub reason: String,
}
/// Actual OS-spawn evidence, retained independently of bounded event delivery.
#[derive(Clone, Copy, Debug)]
pub struct ProcessStarted {
    pub pid: u32,
    pub observed_at: Instant,
}
struct GracefulRequest {
    line: Option<Vec<u8>>,
    deadline: Instant,
}
#[derive(Default)]
struct TransportControl {
    quiescing: bool,
    graceful: Option<GracefulRequest>,
    written_records: u64,
    partial_record: bool,
}
pub struct ProcessHandle {
    input: SyncSender<Vec<u8>>,
    events: Mutex<Receiver<ProcessEvent>>,
    terminal: Arc<Mutex<Option<ProcessTermination>>>,
    started: Arc<Mutex<Option<ProcessStarted>>>,
    stop: Arc<AtomicBool>,
    control: Arc<Mutex<TransportControl>>,
    worker: Option<JoinHandle<()>>,
}
impl ProcessHandle {
    /// Spawning, writes, reads and waits all occur off the caller's thread.
    pub fn spawn(spec: ProcessSpec) -> Result<Self, String> {
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = spec;
            return Err(
                "Extension process supervision is not implemented on this platform yet".into(),
            );
        }
        #[cfg(any(target_os = "linux", windows))]
        {
            let (input, commands) = mpsc::sync_channel(QUEUE);
            let (output, events) = mpsc::sync_channel(QUEUE);
            let stop = Arc::new(AtomicBool::new(false));
            let cancelled = stop.clone();
            let control = Arc::new(Mutex::new(TransportControl::default()));
            let worker_control = control.clone();
            let terminal = Arc::new(Mutex::new(None));
            let terminal_result = terminal.clone();
            let started = Arc::new(Mutex::new(None));
            let worker_started = started.clone();
            let worker = std::thread::Builder::new()
                .name("waifu-extension-process".into())
                .spawn(move || {
                    let mut termination = match supervise(
                        spec,
                        commands,
                        &output,
                        &cancelled,
                        &worker_control,
                        &worker_started,
                    ) {
                        Ok((code, reason)) => ProcessTermination {
                            code,
                            reason,
                            observed_at: Instant::now(),
                        },
                        Err(reason) => ProcessTermination {
                            code: None,
                            reason,
                            observed_at: Instant::now(),
                        },
                    };
                    {
                        let mut stored = terminal_result
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        // Completed supervision is the terminal boundary. Host
                        // stop requests use this same lock and monotonic clock.
                        termination.observed_at = Instant::now();
                        *stored = Some(termination.clone());
                    }
                    let _ = output.try_send(ProcessEvent::Exited {
                        code: termination.code,
                        reason: termination.reason,
                    });
                })
                .map_err(|e| format!("Cannot start extension supervisor: {e}"))?;
            Ok(Self {
                input,
                events: Mutex::new(events),
                terminal,
                started,
                stop,
                control,
                worker: Some(worker),
            })
        }
    }
    /// Validate and frame a host protocol record exactly once. The protocol
    /// encoder includes LF; the raw transport API accepts an unframed line.
    pub fn send_host(
        &self,
        message: &crate::extension_protocol::HostEnvelope,
    ) -> Result<(), String> {
        let encoded = crate::extension_protocol::encode_host_line(message)
            .map_err(|error| error.to_string())?;
        let line = encoded
            .strip_suffix(b"\n")
            .ok_or("Host encoder omitted its line terminator")?;
        self.send_line(line)
    }

    pub fn send_line(&self, line: &[u8]) -> Result<(), String> {
        if line.is_empty()
            || line.len() > MAX_LINE
            || line.contains(&b'\n')
            || line.contains(&b'\r')
        {
            return Err("Extension messages must be one nonempty line of at most 64 KiB".into());
        }
        // Serialize admission and native writes with the quiescing transition.
        let control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if control.quiescing || self.finished() || self.stop.load(Ordering::Acquire) {
            return Err("Extension is stopped".into());
        }
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line);
        bytes.push(b'\n');
        self.input.try_send(bytes).map_err(|e| match e {
            TrySendError::Full(_) => "Extension input queue is full".into(),
            TrySendError::Disconnected(_) => "Extension process is unavailable".into(),
        })
    }
    pub fn poll(&self) -> Option<ProcessEvent> {
        self.events.lock().ok()?.try_recv().ok()
    }
    /// Authoritative terminal receipt, retained even when output flooded its queue.
    pub(crate) fn stop_request_boundary(&self) -> Option<Instant> {
        let terminal = self
            .terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if terminal.is_some() || self.finished() {
            None
        } else {
            Some(Instant::now())
        }
    }
    pub fn termination(&self) -> Option<ProcessTermination> {
        self.terminal.lock().ok()?.clone()
    }
    #[cfg(test)]
    pub(crate) fn discard_terminal_for_test(&self) {
        self.terminal.lock().unwrap().take();
    }
    pub fn started(&self) -> Option<ProcessStarted> {
        *self
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn stop(&self) {
        let _control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.stop.store(true, Ordering::Release);
    }
    pub(crate) fn has_written_record(&self) -> bool {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .written_records
            != 0
    }

    /// Close ordinary admission and revoke all untouched queued input. A partial
    /// frame cannot be recalled safely, so cancellation immediately wins there.
    pub(crate) fn quiesce_input(&self) {
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        control.quiescing = true;
        if control.partial_record {
            self.stop.store(true, Ordering::Release);
        }
    }

    /// Best-effort final record, not a receipt from the child. Replaces untouched
    /// queued input; a partial previous record is force-stopped, never spliced.
    /// The deadline is shared by all processes and never extended here.
    pub(crate) fn request_graceful(
        &self,
        message: &crate::extension_protocol::HostEnvelope,
        deadline: Instant,
    ) -> Result<(), String> {
        let line = crate::extension_protocol::encode_host_line(message)
            .map_err(|error| error.to_string())?;
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if control.graceful.is_some() || self.stop.load(Ordering::Acquire) || self.finished() {
            return Err("Extension is stopped or quiescing".into());
        }
        if control.partial_record || Instant::now() >= deadline {
            self.stop.store(true, Ordering::Release);
            return Err("Cannot safely flush shutdown record".into());
        }
        control.quiescing = true;
        control.graceful = Some(GracefulRequest {
            line: Some(line),
            deadline,
        });
        Ok(())
    }

    /// Join only if already finished; this never waits for an active supervisor.
    pub(crate) fn reap_finished(&mut self) -> bool {
        if !self.finished() {
            return false;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        true
    }
    /// Explicit app-shutdown path, outside render-frame teardown. Waiting is
    /// bounded; ordinary Drop only signals cancellation and never blocks a frame.
    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.stop();
        let deadline = Instant::now() + timeout;
        while !self.finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.finished() {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            true
        } else {
            false
        }
    }
    pub fn finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(|worker| worker.is_finished())
    }
}
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "linux")]
fn nonblocking(fd: std::os::fd::RawFd) -> Result<(), String> {
    // SAFETY: caller owns the live pipe; fcntl does not take ownership of it.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(format!(
            "Cannot configure extension pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
#[cfg(any(target_os = "linux", windows))]
struct Running {
    child: Child,
    group_killed: bool,
    #[cfg(windows)]
    job: crate::windows_process::Job,
}
#[cfg(any(target_os = "linux", windows))]
impl Running {
    fn kill_group(&mut self) {
        if !self.group_killed {
            // Must be called before reaping the group leader, whose PID anchors
            // the group identity against reuse by an unrelated process.
            #[cfg(target_os = "linux")]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            self.job.kill();
            self.group_killed = true;
        }
    }
}
#[cfg(any(target_os = "linux", windows))]
impl Drop for Running {
    fn drop(&mut self) {
        self.kill_group();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(any(target_os = "linux", windows))]
fn supervise(
    spec: ProcessSpec,
    input: Receiver<Vec<u8>>,
    output: &SyncSender<ProcessEvent>,
    stop: &AtomicBool,
    control: &Mutex<TransportControl>,
    spawn_evidence: &Mutex<Option<ProcessStarted>>,
) -> Result<(Option<i32>, String), String> {
    use std::io::ErrorKind;
    #[cfg(target_os = "linux")]
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    if stop.load(Ordering::Acquire) {
        return Err("Stopped before startup".into());
    }
    std::fs::create_dir_all(&spec.data_directory)
        .map_err(|e| format!("Cannot create extension data: {e}"))?;
    #[cfg(not(windows))]
    let mut command = Command::new(&spec.program);
    #[cfg(windows)]
    let mut command = crate::windows_process::command(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    #[cfg(target_os = "linux")]
    command.process_group(0);
    #[cfg(windows)]
    let windows_stdin = {
        let (writer, child_stdin) = crate::windows_process::stdin_pipe()
            .map_err(|error| format!("Cannot configure extension stdin: {error}"))?;
        command.stdin(child_stdin);
        writer
    };
    // Provider keys and unrelated application credentials never enter the child
    // environment. Preserve the user's selected network route explicitly.
    for key in [
        "PATH",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SystemRoot",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("WAIFU_EXTENSION_DATA", &spec.data_directory);
    if let Ok(executable) = std::env::current_exe() {
        command.env("WAIFU_STUDIO_EXECUTABLE", executable);
    }
    #[cfg(target_os = "linux")]
    let parent_pid = std::process::id() as libc::pid_t;
    // SAFETY: only async-signal-safe Linux syscalls run between fork and exec.
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent_pid {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    #[cfg(target_os = "linux")]
    let mut process = Running {
        child: command
            .spawn()
            .map_err(|e| format!("Cannot start extension: {e}"))?,
        group_killed: false,
    };
    #[cfg(windows)]
    let mut process = {
        let (child, job) = crate::windows_process::Job::spawn(&mut command)
            .map_err(|error| format!("Cannot start extension: {error}"))?;
        Running {
            child,
            job,
            group_killed: false,
        }
    };
    *spawn_evidence
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ProcessStarted {
        pid: process.child.id(),
        observed_at: Instant::now(),
    });
    #[cfg(target_os = "linux")]
    let stdin = process
        .child
        .stdin
        .take()
        .ok_or("Extension stdin is unavailable")?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or("Extension stdout is unavailable")?;
    let stderr = process
        .child
        .stderr
        .take()
        .ok_or("Extension stderr is unavailable")?;
    #[cfg(target_os = "linux")]
    for fd in [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()] {
        nonblocking(fd)?;
    }
    #[cfg(windows)]
    let (stdin, stdout, stderr) = (
        windows_stdin,
        crate::windows_process::Reader(stdout),
        crate::windows_process::Reader(stderr),
    );
    let (mut stdout, mut stderr) = (stdout, stderr);
    let mut stdin = Some(stdin);
    output
        .try_send(ProcessEvent::Started {
            pid: process.child.id(),
        })
        .map_err(|_| "Extension event queue unavailable")?;
    let started = Instant::now();
    let mut pending: Option<(Vec<u8>, usize)> = None;
    let mut stdout_buffer = Vec::new();
    let mut stderr_buffer = VecDeque::new();
    let mut logs = 0usize;
    let mut stdout_closed = false;
    let mut exited = None;
    let mut drain_deadline = None;
    let mut stdin_closed = false;
    let mut quiescing = false;
    let reason;
    let code;
    loop {
        if stop.load(Ordering::Acquire) {
            reason = "Stopped".into();
            code = None;
            break;
        }
        if spec
            .lifetime
            .is_some_and(|limit| started.elapsed() >= limit)
        {
            reason = "Hook timed out".into();
            code = None;
            break;
        }
        // Bounded byte work per iteration prevents stdout/stderr from starving
        // cancellation, writes or child-reaping even under a sustained flood.
        for _ in 0..16 {
            let mut chunk = [0u8; 4096];
            match stdout.read(&mut chunk) {
                Ok(0) => {
                    stdout_closed = true;
                    break;
                }
                Ok(count) => {
                    for byte in &chunk[..count] {
                        if *byte == b'\n' {
                            if stdout_buffer.last() == Some(&b'\r') {
                                stdout_buffer.pop();
                            }
                            if stdout_buffer.is_empty()
                                || std::str::from_utf8(&stdout_buffer).is_err()
                            {
                                return Err(
                                    "Extension emitted an empty or non-UTF8 protocol line".into()
                                );
                            }
                            output
                                .try_send(ProcessEvent::Line(std::mem::take(&mut stdout_buffer)))
                                .map_err(|_| {
                                    "Extension output queue overflow or disconnected".to_owned()
                                })?;
                        } else {
                            if stdout_buffer.len() >= MAX_LINE {
                                return Err("Extension output line exceeds 64 KiB".into());
                            }
                            stdout_buffer.push(*byte);
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err("Cannot read extension stdout".into()),
            }
        }
        for _ in 0..4 {
            let mut chunk = [0u8; 1024];
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    for byte in &chunk[..count] {
                        if *byte == b'\n' {
                            if logs < 8 {
                                let line: Vec<_> = stderr_buffer.drain(..).collect();
                                let _ = output.try_send(ProcessEvent::Log(
                                    String::from_utf8_lossy(&line).into_owned(),
                                ));
                                logs += 1;
                            } else {
                                stderr_buffer.clear();
                            }
                        } else {
                            if stderr_buffer.len() >= 2048 {
                                stderr_buffer.pop_front();
                            }
                            stderr_buffer.push_back(*byte);
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        // No ordinary record may cross the host's quiescing boundary. This
        // lock also covers each nonblocking write, so revocation cannot race a
        // continuation of private data already staged in `pending`.
        let mut control = control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stop.load(Ordering::Acquire) {
            reason = "Stopped".into();
            code = None;
            break;
        }
        if control.quiescing && !quiescing {
            if pending.as_ref().is_some_and(|(_, offset)| *offset != 0) {
                reason = "Shutdown interrupted partial input record".into();
                code = None;
                break;
            }
            pending = None;
            // Admission is now closed; this queue cannot refill.
            for _ in 0..QUEUE {
                if input.try_recv().is_err() {
                    break;
                }
            }
            quiescing = true;
        }
        if let Some(request) = &mut control.graceful {
            if Instant::now() >= request.deadline {
                reason = "Shutdown grace expired".into();
                code = None;
                break;
            }
            if request.line.is_some() {
                pending = request.line.take().map(|line| (line, 0));
            }
        }
        if !quiescing && !stdin_closed && pending.is_none() {
            match input.try_recv() {
                Ok(line) => pending = Some((line, 0)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    reason = "Host disconnected".into();
                    code = None;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some((bytes, offset)) = &mut pending {
            match stdin
                .as_mut()
                .ok_or("Extension stdin is closed")?
                .write(&bytes[*offset..])
            {
                Ok(0) => {
                    stdin_closed = true;
                    pending = None;
                }
                Ok(count) => {
                    *offset += count;
                    if *offset == bytes.len() {
                        pending = None;
                        control.written_records = control.written_records.saturating_add(1);
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => (),
                Err(e) if e.kind() == ErrorKind::BrokenPipe => {
                    stdin_closed = true;
                    pending = None;
                }
                Err(_) => return Err("Cannot write extension stdin".into()),
            }
        }
        control.partial_record = pending.as_ref().is_some_and(|(_, offset)| *offset != 0);
        if quiescing && control.graceful.is_some() && pending.is_none() {
            stdin_closed = true;
            stdin.take(); // EOF follows the bounded final record; no new input.
        }
        drop(control);
        #[cfg(target_os = "linux")]
        if exited.is_none() {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            // WNOWAIT observes exit without freeing the numeric process-group
            // identity. Kill ordinary descendants before reaping the leader.
            let observed = unsafe {
                libc::waitid(
                    libc::P_PID,
                    process.child.id(),
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if observed != 0 {
                return Err("Cannot inspect extension process".into());
            }
            if unsafe { info.assume_init().si_pid() } != 0 {
                process.kill_group();
                exited = process
                    .child
                    .try_wait()
                    .map_err(|_| "Cannot reap extension process")?;
                drain_deadline = Some(Instant::now() + Duration::from_millis(500));
            }
        }
        #[cfg(windows)]
        if exited.is_none() {
            exited = process
                .child
                .try_wait()
                .map_err(|_| "Cannot inspect extension process")?;
            if exited.is_some() {
                process.kill_group();
                drain_deadline = Some(Instant::now() + Duration::from_millis(500));
            }
        }
        if drain_deadline.is_some_and(|deadline| Instant::now() >= deadline) && !stdout_closed {
            code = exited.and_then(|status| status.code());
            reason = "Process exited; descendant retained stdout beyond drain deadline".into();
            break;
        }
        if let Some(status) = exited
            && stdout_closed
        {
            if !stdout_buffer.is_empty() {
                return Err("Extension exited with an incomplete protocol line".into());
            }
            reason = if status.success() {
                "Completed"
            } else {
                "Process failed"
            }
            .into();
            code = status.code();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(process); // kill/reap before reporting termination
    Ok((code, reason))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn retained_os_spawn_evidence_survives_fast_exit_flood_and_event_drain_but_not_spawn_failure() {
        for script in [
            "pass",
            "import sys\nfor i in range(10000): print('{}', flush=True)",
        ] {
            let (process, root) = fixture(script, None);
            let mut fixture = ShutdownProcess { process, root };
            let deadline = Instant::now() + Duration::from_secs(3);
            while !fixture.process.finished() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            let evidence = fixture.process.started().expect("actual OS spawn was lost");
            assert!(evidence.pid > 0);
            while fixture.process.poll().is_some() {}
            assert_eq!(fixture.process.started().unwrap().pid, evidence.pid);
            assert!(fixture.process.reap_finished());
            assert!(fixture.process.termination().is_some());
        }
        let root = std::env::temp_dir().join(format!("failed-spawn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let process = ProcessHandle::spawn(ProcessSpec {
            program: root.join("missing-executable"),
            args: vec![],
            working_directory: root.clone(),
            data_directory: root.join("data"),
            lifetime: None,
        })
        .unwrap();
        let mut fixture = ShutdownProcess { process, root };
        let deadline = Instant::now() + Duration::from_secs(3);
        while !fixture.process.finished() {
            assert!(Instant::now() < deadline, "failed spawn did not terminate");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture.process.reap_finished());
        assert!(fixture.process.started().is_none());
    }
    fn stopping() -> crate::extension_protocol::HostEnvelope {
        use crate::extension_protocol::*;
        HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: uuid::Uuid::new_v4(),
            sequence: 99,
            process_generation: 1,
            message: HostMessage::Event {
                event: HostEvent::AppStopping {
                    app_run_id: uuid::Uuid::new_v4(),
                },
            },
        }
    }

    struct ShutdownProcess {
        process: ProcessHandle,
        root: PathBuf,
    }
    impl Drop for ShutdownProcess {
        fn drop(&mut self) {
            self.process.shutdown(Duration::from_secs(1));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn graceful_transport_purges_untouched_input_preserves_final_jsonl_and_reports_real_exit() {
        let (process, root) = fixture(
            r#"
import pathlib, sys
with pathlib.Path('received.jsonl').open('w') as output:
    for line in sys.stdin:
        output.write(line)
        output.flush()
"#,
            None,
        );
        let mut fixture = ShutdownProcess { process, root };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.root.join("received.jsonl").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let final_record = stopping();
        {
            // Hold the same native-write lock to put the supervisor in a
            // deterministic full-queue/untouched-input state before quiescence.
            let mut control = fixture.process.control.lock().unwrap();
            for _ in 0..QUEUE {
                fixture
                    .process
                    .input
                    .try_send(b"PRIVATE_DO_NOT_FLUSH\n".to_vec())
                    .unwrap();
            }
            control.quiescing = true;
        }
        fixture
            .process
            .request_graceful(&final_record, Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert!(fixture.process.send_line(b"NEW_INPUT").is_err());
        while !fixture.process.finished() {
            assert!(
                Instant::now() < deadline,
                "cooperative transport did not exit"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture.process.reap_finished());
        let bytes = std::fs::read(fixture.root.join("received.jsonl")).unwrap();
        assert_eq!(
            bytes,
            crate::extension_protocol::encode_host_line(&final_record).unwrap()
        );
        assert_eq!(fixture.process.termination().unwrap().code, Some(0));
    }

    #[test]
    fn graceful_transport_force_stops_partial_old_record_instead_of_completing_or_splicing() {
        let (process, root) = fixture("import time; time.sleep(30)", None);
        let mut fixture = ShutdownProcess { process, root };
        fixture.process.send_line(&vec![b'x'; MAX_LINE]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if fixture.process.control.lock().unwrap().partial_record {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "blocked pipe never retained a partial record"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            fixture
                .process
                .request_graceful(&stopping(), deadline)
                .is_err()
        );
        assert!(fixture.process.shutdown(Duration::from_secs(1)));
        assert_eq!(fixture.process.control.lock().unwrap().written_records, 0);
        assert!(fixture.process.termination().is_some());
    }

    #[test]
    fn graceful_transport_deadline_bounds_a_blocked_reader_without_host_polling() {
        let (process, root) = fixture("import time; time.sleep(30)", None);
        let mut fixture = ShutdownProcess { process, root };
        fixture.process.quiesce_input();
        let start = Instant::now();
        fixture
            .process
            .request_graceful(&stopping(), start + Duration::from_millis(30))
            .unwrap();
        while !fixture.process.finished() {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture.process.reap_finished());
        assert_eq!(
            fixture.process.termination().unwrap().reason,
            "Shutdown grace expired"
        );
    }
    fn fixture(code: &str, lifetime: Option<Duration>) -> (ProcessHandle, PathBuf) {
        let root = std::env::temp_dir().join(format!("waifu-process-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let process = ProcessHandle::spawn(ProcessSpec {
            program: "/usr/bin/python3".into(),
            args: vec!["-u".into(), "-c".into(), code.into()],
            working_directory: root.clone(),
            data_directory: root.join("data"),
            lifetime,
        })
        .unwrap();
        (process, root)
    }
    fn collect(process: &ProcessHandle) -> Vec<ProcessEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut result = vec![];
        loop {
            while let Some(event) = process.poll() {
                result.push(event);
            }
            if process.finished() {
                while let Some(event) = process.poll() {
                    result.push(event);
                }
                return result;
            }
            assert!(Instant::now() < deadline, "supervisor hung");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn actual_json_line_exchange_and_exit_are_observable() {
        let (process, root) = fixture(
            "import sys\nprint(sys.stdin.readline().strip(), flush=True)",
            None,
        );
        process.send_line(br#"{"ping":1}"#).unwrap();
        let events = collect(&process);
        assert!(matches!(events.first(), Some(ProcessEvent::Started { .. })));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessEvent::Line(line) if line == br#"{"ping":1}"#))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessEvent::Exited { code: Some(0), .. }))
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn typed_host_record_round_trips_through_real_child() {
        use crate::extension_protocol::{
            API_VERSION, HostEnvelope, HostEnvelopeType, HostMessage, IntegrationOrigin,
            parse_host_line,
        };
        let (process, root) = fixture(
            "import sys\nprint(sys.stdin.readline().strip(), flush=True)",
            None,
        );
        let message = HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: uuid::Uuid::new_v4(),
            sequence: 1,
            process_generation: 1,
            message: HostMessage::Handshake {
                settings: Default::default(),
                origin: IntegrationOrigin {
                    integration_id: "org.example.fixture".into(),
                    display_name: "Fixture".into(),
                },
                capabilities: vec![],
            },
        };
        process.send_host(&message).unwrap();
        let events = collect(&process);
        let lines: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ProcessEvent::Line(line) => Some(line),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(parse_host_line(lines[0]).unwrap(), message);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn maximum_line_survives_process_exit_and_overflow_reason_survives_full_queue() {
        let (process, root) = fixture("print('x'*65536, flush=True)", None);
        let events = collect(&process);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessEvent::Line(line) if line.len() == MAX_LINE))
        );
        assert_eq!(process.termination().unwrap().code, Some(0));
        let _ = std::fs::remove_dir_all(root);
        let (process, root) = fixture(
            "import sys,time\nsys.stdout.write('{}\\n'*200);sys.stdout.flush();time.sleep(30)",
            None,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !process.finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(process.termination().unwrap().reason.contains("overflow"));
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn overlong_partial_line_and_stdout_flood_are_bounded() {
        for code in [
            "import sys,time\nsys.stdout.write('x'*70000);sys.stdout.flush();time.sleep(30)",
            "import sys,time\nsys.stdout.write('{}\\n'*200);sys.stdout.flush();time.sleep(30)",
        ] {
            let (process, root) = fixture(code, None);
            // Do not drain until the supervisor stops: queue overflow must end it.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !process.finished() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(collect(&process).len() <= QUEUE);
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn unread_stdin_never_blocks_stop_and_hooks_time_out() {
        for lifetime in [None, Some(Duration::from_millis(100))] {
            let (process, root) = fixture("import time\ntime.sleep(30)", lifetime);
            for _ in 0..QUEUE {
                let _ = process.send_line(&vec![b'x'; MAX_LINE]);
            }
            if lifetime.is_none() {
                process.stop();
            }
            assert!(
                collect(&process)
                    .iter()
                    .any(|e| matches!(e, ProcessEvent::Exited { .. }))
            );
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn closed_stdin_preserves_exit_status_and_shutdown_is_bounded() {
        let (mut process, root) = fixture(
            "import os,time\nos.close(0);print('{}',flush=True);time.sleep(.05)",
            None,
        );
        for _ in 0..QUEUE {
            let _ = process.send_line(&vec![b'x'; MAX_LINE]);
        }
        collect(&process);
        assert_eq!(process.termination().unwrap().code, Some(0));
        assert!(process.shutdown(Duration::from_millis(100)));
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn escaped_descendant_cannot_hold_supervisor_open_after_leader_exit() {
        // Deliberately leave the managed group, retaining stdout. This is not
        // sandboxed; the test cleans up the escaped process by its emitted PID.
        let script = "import os,time\npid=os.fork()\nif pid==0:\n os.setsid();time.sleep(30);os._exit(0)\nprint(pid,flush=True)";
        let (process, root) = fixture(script, None);
        let events = collect(&process);
        let pid: i32 = events
            .iter()
            .find_map(|e| match e {
                ProcessEvent::Line(line) => std::str::from_utf8(line).ok()?.parse().ok(),
                _ => None,
            })
            .unwrap();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let terminal = process.termination().unwrap();
        assert_eq!(terminal.code, Some(0));
        assert!(terminal.reason.contains("drain deadline"), "{terminal:?}");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn malformed_utf8_and_nonzero_exit_report_failure() {
        for code in [
            "import sys\nsys.stdout.buffer.write(b'\\xff\\n')",
            "raise SystemExit(7)",
            "import sys\nsys.stdout.write('partial')",
        ] {
            let (process, root) = fixture(code, None);
            let events = collect(&process);
            assert!(events.iter().any(
                |e| matches!(e, ProcessEvent::Exited { reason, .. } if reason != "Completed")
            ));
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
