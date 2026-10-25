#![cfg(target_os = "linux")]
//! Actual supervisor termination of production HTTP/TLS helpers stalled locally.
//! The child is an exact Rust test harness, not a production endpoint override.
use super::*;
use crate::extension_process::{ProcessHandle, ProcessSpec};
use std::{
    fs,
    net::{TcpListener, TcpStream},
    path::PathBuf,
};

fn child_fixture(data: &std::path::Path) {
    let value: Value =
        serde_json::from_slice(&fs::read(data.join("blocked-startup.json")).unwrap()).unwrap();
    let url = reqwest::Url::parse(value["url"].as_str().unwrap()).unwrap();
    assert_eq!(
        url.host_str(),
        Some(if value["stage"] == "resolver" {
            "discord-startup.invalid"
        } else {
            "127.0.0.1"
        })
    );
    assert!(url.port().is_some());
    match value["stage"].as_str().unwrap() {
        "http" => {
            assert_eq!(url.scheme(), "http");
            let client = discovery_client_builder().no_proxy().build().unwrap();
            let _ = request_discovery(
                &client,
                url.as_str(),
                "SyntheticPrivateBotToken",
                Duration::from_secs(8),
            );
        }
        "resolver" => {
            struct PendingResolver(PathBuf);
            impl reqwest::dns::Resolve for PendingResolver {
                fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
                    assert_eq!(name.as_str(), "discord-startup.invalid");
                    fs::write(&self.0, b"resolver entered").unwrap();
                    Box::pin(std::future::pending())
                }
            }
            assert_eq!(url.scheme(), "http");
            let client = discovery_client_builder()
                .no_proxy()
                .dns_resolver(Arc::new(PendingResolver(data.join("resolver-entered"))))
                .build()
                .unwrap();
            let _ = request_discovery(
                &client,
                url.as_str(),
                "SyntheticPrivateBotToken",
                Duration::from_secs(8),
            );
        }
        "tls" => {
            assert_eq!(url.scheme(), "wss");
            let _ = crate::fish_ws::observation_socket(&url);
        }
        _ => panic!("unknown local fixture stage"),
    }
    // Parent must terminate the blocked child before a startup continuation.
    fs::write(data.join("continued"), b"startup operation returned").unwrap();
    panic!("blocked operation completed before supervisor stop");
}
fn receive_header(stream: &mut TcpStream) {
    let mut bytes = vec![];
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 8192);
    }
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization: bot syntheticprivatebottoken\r\n")
    );
}
fn spawn_fixture(root: &std::path::Path, name: String) -> ProcessHandle {
    // Redirection keeps test-harness text out of the protocol pipe; only this
    // child clears proxies before exec so no external route can be selected.
    let launch = "import os,sys\nfor key in ['HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','http_proxy','https_proxy','all_proxy','NO_PROXY','no_proxy']:\n os.environ.pop(key,None)\nos.dup2(2,1)\nos.execv(sys.argv[1],sys.argv[1:])\n";
    ProcessHandle::spawn(ProcessSpec {
        program: "/usr/bin/python3".into(),
        args: vec![
            "-c".into(),
            launch.into(),
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--exact".into(),
            name,
            "--test-threads=1".into(),
            "--nocapture".into(),
        ],
        working_directory: root.to_path_buf(),
        data_directory: root.to_path_buf(),
        lifetime: None,
    })
    .unwrap()
}
#[test]
fn supervisor_reaps_child_blocked_in_http_discovery_and_tls_handshake() {
    if let Some(data) = std::env::var_os("WAIFU_EXTENSION_DATA") {
        let data = PathBuf::from(data);
        if data.join("blocked-startup.json").is_file() {
            child_fixture(&data);
            return;
        }
    }
    for stage in ["http", "tls"] {
        let root =
            std::env::temp_dir().join(format!("discord-blocked-{stage}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!(
            "{}://{address}/gateway/bot",
            if stage == "http" { "http" } else { "wss" }
        );
        fs::write(
            root.join("blocked-startup.json"),
            serde_json::to_vec(&json!({"stage":stage,"url":url})).unwrap(),
        )
        .unwrap();
        let (ready, blocked) = mpsc::channel();
        let (closed, eof) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "local startup child never connected"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("local accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            if stage == "http" {
                receive_header(&mut stream);
            } else {
                let mut header = [0; 5];
                stream.read_exact(&mut header).unwrap();
                assert_eq!(header[0], 0x16, "expected actual TLS ClientHello record");
                assert_eq!(header[1], 0x03);
            }
            ready.send(()).unwrap();
            // Do not send HTTP headers or TLS ServerHello. Drain already queued
            // ClientHello bytes until supervisor termination closes the socket.
            let mut buffer = [0; 4096];
            let disconnected = loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break true,
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                        ) =>
                    {
                        break true;
                    }
                    Err(_) => break false,
                }
            };
            let _ = closed.send(disconnected);
        });
        let name = format!(
            "{}::supervisor_reaps_child_blocked_in_http_discovery_and_tls_handshake",
            module_path!().split_once("::").unwrap().1
        );
        let mut process = spawn_fixture(&root, name);
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            !process.finished(),
            "fixture must still be blocked before stop"
        );
        let pid = process.started().unwrap().pid;
        let stop = Instant::now();
        assert!(
            process.shutdown(Duration::from_secs(2)),
            "blocked {stage} child was not reaped"
        );
        assert!(stop.elapsed() < Duration::from_millis(2500));
        assert_eq!(process.termination().unwrap().reason, "Stopped");
        assert!(
            !root.join("continued").exists(),
            "startup continued after stop"
        );
        assert!(
            eof.recv_timeout(Duration::from_secs(2)).unwrap(),
            "{stage} socket remained open after reap"
        );
        server.join().unwrap();
        // SAFETY: probing the PID retained from this test's already reaped child.
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert!(process.send_line(b"late host input").is_err());
    }
}

#[test]
fn supervisor_reaps_pending_discovery_resolver_without_dns_or_tcp() {
    if let Some(data) = std::env::var_os("WAIFU_EXTENSION_DATA") {
        let data = PathBuf::from(data);
        if data.join("blocked-startup.json").is_file() {
            child_fixture(&data);
            return;
        }
    }
    let root = std::env::temp_dir().join(format!("discord-resolver-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    // This unused trap listener proves no address is ever returned/connected.
    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    trap.set_nonblocking(true).unwrap();
    let url = format!(
        "http://discord-startup.invalid:{}/gateway/bot",
        trap.local_addr().unwrap().port()
    );
    fs::write(
        root.join("blocked-startup.json"),
        serde_json::to_vec(&json!({"stage":"resolver","url":url})).unwrap(),
    )
    .unwrap();
    let name = format!(
        "{}::supervisor_reaps_pending_discovery_resolver_without_dns_or_tcp",
        module_path!().split_once("::").unwrap().1
    );
    let mut process = spawn_fixture(&root, name);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.join("resolver-entered").exists() {
        assert!(
            !process.finished(),
            "resolver fixture exited before entering pending resolution"
        );
        assert!(Instant::now() < deadline, "resolver hook was not entered");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(trap.accept(),Err(error) if error.kind()==io::ErrorKind::WouldBlock));
    let pid = process.started().unwrap().pid;
    let stop = Instant::now();
    assert!(process.shutdown(Duration::from_secs(2)));
    assert!(stop.elapsed() < Duration::from_millis(2500));
    assert_eq!(process.termination().unwrap().reason, "Stopped");
    assert!(!root.join("continued").exists());
    assert!(matches!(trap.accept(),Err(error) if error.kind()==io::ErrorKind::WouldBlock));
    // SAFETY: this PID was retained from this fixture's already reaped child.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    assert!(process.send_line(b"late host input").is_err());
}

#[test]
fn supervisor_reaps_actual_node_voice_worker_with_synchronously_blocked_event_loop() {
    check_blocked_node_worker(false);
}

#[test]
fn supervisor_terminates_blocked_node_workers_same_group_descendant() {
    check_blocked_node_worker(true);
}

fn check_blocked_node_worker(with_descendant: bool) {
    let root = std::env::temp_dir().join(format!("discord-node-blocked-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    // Resolve the installed runtime before the supervisor clears the environment.
    // This test must fail, rather than silently skip, if the Node stack is absent.
    let output = std::process::Command::new("node")
        .args(["--eval", "process.stdout.write(process.execPath)"])
        .output()
        .expect("Node runtime required for voice supervisor acceptance");
    assert!(output.status.success());
    let node = PathBuf::from(String::from_utf8(output.stdout).unwrap());
    assert!(node.is_absolute());
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = source.join("tools/discord-voice/tests/fixtures/voice-worker.mjs");
    let mut process = ProcessHandle::spawn(ProcessSpec {
        program: node,
        args: vec![
            fixture.to_string_lossy().into_owned(),
            if with_descendant {
                "sync-block-tree"
            } else {
                "sync-block"
            }
            .into(),
        ],
        working_directory: source,
        data_directory: root.clone(),
        lifetime: None,
    })
    .unwrap();
    process
        .send_line(
            serde_json::to_string(&json!({
                "api_version": 1, "type": "host", "sequence": 1,
                "session_id": "11111111-1111-4111-8111-111111111111", "process_generation": 1,
                "message": {"type": "handshake", "settings": {},
                    "capabilities": ["audio.transcribe", "message.deliver", "lifecycle.observe"]}
            }))
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
    // Cold module loading on a Windows-mounted checkout can take several
    // seconds; the measured shutdown budget below remains two seconds.
    let deadline = Instant::now() + Duration::from_secs(30);
    while fs::read_to_string(root.join("marker")).ok().as_deref() != Some("blocked") {
        assert!(
            Instant::now() < deadline,
            "Node fixture never blocked; terminal={:?}",
            process.termination()
        );
        assert!(
            !process.finished(),
            "Node fixture exited before entering its blocked state: {:?}",
            process.termination()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let pid = process.started().expect("actual Node spawn receipt").pid;
    let descendant = with_descendant.then(|| {
        let pid: u32 = fs::read_to_string(root.join("descendant-pid"))
            .unwrap()
            .parse()
            .unwrap();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_ne!(fields[0], "Z", "descendant must be alive before stop");
        assert_eq!(
            fields[2].parse::<u32>().unwrap(),
            process.started().unwrap().pid,
            "descendant must belong to the supervised process group"
        );
        pid
    });
    let stop = Instant::now();
    assert!(
        process.shutdown(Duration::from_secs(2)),
        "blocked Node worker was not reaped"
    );
    assert!(stop.elapsed() < Duration::from_millis(2500));
    assert_eq!(process.termination().unwrap().reason, "Stopped");
    if let Some(pid) = descendant {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Ok(stat)
                    if stat.rsplit_once(')').unwrap().1.split_whitespace().next() == Some("Z") =>
                {
                    break;
                }
                Err(error) => panic!("cannot inspect owned descendant: {error}"),
                _ => {
                    assert!(
                        Instant::now() < deadline,
                        "descendant survived group shutdown"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        // Orphan reaping belongs to the system reaper. A zombie proves exit,
        // not descendant reaping by ProcessHandle. Also check effects ceased.
        let heartbeat = fs::read(root.join("heartbeat")).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(fs::read(root.join("heartbeat")).unwrap(), heartbeat);
    }
    assert_eq!(fs::read_to_string(root.join("marker")).unwrap(), "blocked");
    assert!(!root.join("continued").exists());
    // SAFETY: probe only this fixture's PID after the supervisor's reap receipt.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    assert!(process.send_line(b"late host input").is_err());
}
