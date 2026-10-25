//! Local fake HTTP and controlled startup boundaries; no service URLs are dialed.
use super::tests::host;
use super::*;
use std::net::{TcpListener, TcpStream};

fn server(
    handler: impl FnOnce(&mut TcpStream) + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        handler(&mut stream);
    });
    (format!("http://{address}/gateway/bot"), task)
}
fn request(stream: &mut TcpStream) -> String {
    let mut bytes = vec![];
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 8192);
    }
    String::from_utf8(bytes).unwrap()
}
fn reply(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(body));
}
fn body() -> Vec<u8> {
    serde_json::to_vec(&json!({"url":"wss://gateway.discord.gg","shards":1,
    "session_start_limit":{"total":1000,"remaining":99,"reset_after":60000,"max_concurrency":1}}))
    .unwrap()
}
fn client(timeout: Duration) -> reqwest::blocking::Client {
    discovery_client_builder()
        .no_proxy()
        .timeout(timeout)
        .build()
        .unwrap()
}
fn input() -> (Input, mpsc::SyncSender<HostEnvelope>) {
    let (tx, rx) = mpsc::sync_channel(32);
    (
        Input {
            rx,
            status: Arc::new(AtomicU8::new(RUNNING)),
        },
        tx,
    )
}
const TOKEN: &str = "SyntheticPrivateBotToken";
#[test]
fn discovery_sends_bot_header_and_parses_only_valid_bounded_response() {
    let (url, task) = server(|stream| {
        let request = request(stream);
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /gateway/bot HTTP/1.1\r\n"));
        assert!(lower.contains("authorization: bot syntheticprivatebottoken\r\n"));
        assert!(lower.contains("user-agent: waifustudio/0.1\r\n"));
        reply(stream, "200 OK", &body());
    });
    let info = request_discovery(
        &client(Duration::from_secs(2)),
        &url,
        TOKEN,
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(info.remaining, 99);
    assert_eq!(
        info.url.as_str(),
        "wss://gateway.discord.gg/?v=10&encoding=json"
    );
    task.join().unwrap();
}
#[test]
fn discovery_does_not_follow_redirects_or_forward_credentials_to_redirect_target() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let address = target.local_addr().unwrap();
    let (url, task) = server(move |stream| {
        request(stream);
        let _ = write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: http://{address}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    });
    assert!(
        request_discovery(
            &client(Duration::from_secs(1)),
            &url,
            TOKEN,
            Duration::from_secs(1)
        )
        .is_err()
    );
    task.join().unwrap();
    assert!(matches!(target.accept(),Err(error) if error.kind()==io::ErrorKind::WouldBlock));
}
#[test]
fn remote_error_body_and_credentials_never_enter_diagnostics() {
    let (url, task) = server(|stream| {
        request(stream);
        reply(
            stream,
            "401 Unauthorized",
            b"SyntheticPrivateBotToken remote-private-body",
        );
    });
    let error = request_discovery(
        &client(Duration::from_secs(1)),
        &url,
        TOKEN,
        Duration::from_secs(1),
    )
    .err()
    .unwrap();
    assert_eq!(
        error,
        "Discord Gateway discovery failed; check bot token and service limits"
    );
    assert!(!error.contains(TOKEN));
    assert!(!error.contains("remote-private-body"));
    task.join().unwrap();
}
#[test]
fn discovery_rejects_oversized_body_before_json_parsing() {
    let (url, task) = server(|stream| {
        request(stream);
        reply(stream, "200 OK", &vec![b' '; 65537]);
    });
    let error = request_discovery(
        &client(Duration::from_secs(1)),
        &url,
        TOKEN,
        Duration::from_secs(1),
    )
    .err()
    .unwrap();
    assert_eq!(error, "Discord Gateway response exceeds its limit");
    task.join().unwrap();
}
#[test]
fn slow_drip_body_cannot_extend_absolute_request_timeout() {
    let (url, task) = server(|stream| {
        request(stream);
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n");
        for _ in 0..100 {
            if stream.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let start = Instant::now();
    let error = request_discovery(
        &client(Duration::from_millis(400)),
        &url,
        TOKEN,
        Duration::from_millis(400),
    )
    .err()
    .unwrap();
    assert!(start.elapsed() < Duration::from_millis(1500));
    assert_eq!(error, "Cannot read Discord Gateway discovery");
    task.join().unwrap();
}
#[test]
fn host_eof_during_discovery_wins_over_late_success_and_error() {
    for success in [false, true] {
        let (input, _tx) = input();
        let status = input.status.clone();
        let (url, task) = server(move |stream| {
            request(stream);
            status.store(EOF, Ordering::Release);
            std::thread::sleep(Duration::from_millis(20));
            reply(
                stream,
                if success {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                },
                &body(),
            );
        });
        let mut host = host();
        let http = client(Duration::from_secs(1));
        let result = while_host_active(&input, &mut host, Instant::now(), || {
            request_discovery(&http, &url, TOKEN, Duration::from_secs(1))
        })
        .unwrap();
        assert!(result.is_none());
        assert!(host.stopped());
        task.join().unwrap();
    }
}
#[test]
fn startup_skips_work_after_eof_and_drops_late_connected_values() {
    let (input, _tx) = input();
    let mut host = host();
    input.status.store(EOF, Ordering::Release);
    let result: Option<()> = while_host_active(&input, &mut host, Instant::now(), || {
        panic!("operation after EOF")
    })
    .unwrap();
    assert!(result.is_none());
    struct ValueToDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for ValueToDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let (input, _tx) = self::input();
    let mut host = super::tests::host();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let result = while_host_active(&input, &mut host, Instant::now(), || {
        input.status.store(EOF, Ordering::Release);
        ValueToDrop(dropped.clone())
    })
    .unwrap();
    assert!(result.is_none());
    assert!(dropped.load(Ordering::Acquire));
    assert!(host.stopped());
}
#[test]
fn host_input_failure_during_discovery_is_terminal_before_returned_data() {
    let (input, _tx) = input();
    let status = input.status.clone();
    let (url, task) = server(move |stream| {
        request(stream);
        status.store(FAILED, Ordering::Release);
        reply(stream, "200 OK", &body());
    });
    let mut host = host();
    let http = client(Duration::from_secs(1));
    let result = while_host_active(&input, &mut host, Instant::now(), || {
        request_discovery(&http, &url, TOKEN, Duration::from_secs(1))
    });
    assert!(matches!(
        result,
        Err("Discord host input failed or overflowed")
    ));
    assert!(host.stopped());
    task.join().unwrap();
}
