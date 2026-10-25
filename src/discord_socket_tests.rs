//! Real localhost sockets only. No service credentials, remote endpoints or DNS.
use super::tests::{envelope, host};
use super::*;
use std::{
    net::{TcpListener, TcpStream},
    thread::JoinHandle,
};

type ServerSocket = WebSocket<TcpStream>;
fn pair(
    handler: impl FnOnce(&mut ServerSocket) + Send + 'static,
) -> (Socket, JoinHandle<()>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (done, hold) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        handler(&mut socket);
        let _ = hold.recv_timeout(Duration::from_secs(3));
    });
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let config = tungstenite::protocol::WebSocketConfig::default()
        .write_buffer_size(0)
        .max_message_size(Some(64 * 1024))
        .max_frame_size(Some(64 * 1024));
    let (socket, _) = tungstenite::client::client_with_config(
        format!("ws://{address}"),
        MaybeTlsStream::Plain(stream),
        Some(config),
    )
    .unwrap();
    (socket, server, done)
}
fn send_json(socket: &mut ServerSocket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .unwrap();
}
fn read_op(socket: &mut ServerSocket, op: u64) -> Value {
    for _ in 0..8 {
        match socket.read().unwrap() {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["op"] == op {
                    return value;
                }
            }
            Message::Ping(_) => socket.flush().unwrap(),
            _ => {}
        }
    }
    panic!("missing expected control opcode {op}");
}
fn hello(socket: &mut ServerSocket) {
    send_json(socket, json!({"op":10,"d":{"heartbeat_interval":1000}}));
}
fn ready(socket: &mut ServerSocket) {
    send_json(
        socket,
        json!({"op":0,"t":"READY","s":1,"d":{"session_id":"local-session","resume_gateway_url":"wss://gateway.discord.gg","user":{"id":"444","bot":true}}}),
    );
}
fn chat(socket: &mut ServerSocket) {
    send_json(
        socket,
        json!({"op":0,"t":"MESSAGE_CREATE","s":2,"d":{"id":"333","guild_id":"111","channel_id":"222","type":0,"content":"local fixture original","author":{"id":"555","username":"Viewer"}}}),
    );
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
fn gateway() -> Gateway {
    Gateway::new(crate::discord_messages::ChannelFilter::new("111".into(), "222".into()).unwrap())
}
fn drive(
    socket: &mut Socket,
    input: &Input,
    host: &mut HostBridge,
    gateway: &mut Gateway,
    writer: &mut impl Write,
    start: Instant,
) -> Result<SessionExit, String> {
    gateway.connected(start.elapsed()).unwrap();
    let mut last = None;
    let mut failures = 0;
    run_session(
        socket,
        Session {
            input,
            host,
            gateway,
            start,
            token: "SyntheticPrivateBotToken",
            writer,
            last_identify: &mut last,
            failures: &mut failures,
            heartbeat_jitter: || 0.0,
            identify_gate: &mut || Ok(crate::discord_identify::Admission::Reserved),
        },
    )
}
fn finish(socket: Socket, done: mpsc::Sender<()>, server: JoinHandle<()>) {
    drop(socket);
    let _ = done.send(());
    server.join().unwrap();
}

#[test]
fn fragmented_hello_and_chat_reach_host_then_shutdown_cancels_socket_loop() {
    let (input, tx) = input();
    let (notify, receipt) = mpsc::channel();
    let (mut socket, server, done) = pair(move |socket| {
        // Two actual unmasked server fragments, not two independent JSON messages.
        let bytes = br#"{"op":10,"d":{"heartbeat_interval":1000}}"#;
        let split = bytes.len() / 2;
        let mut frames = vec![0x01, split as u8];
        frames.extend(&bytes[..split]);
        frames.extend([0x80, (bytes.len() - split) as u8]);
        frames.extend(&bytes[split..]);
        socket.get_mut().write_all(&frames).unwrap();
        socket.get_mut().flush().unwrap();
        let identify = read_op(socket, 2);
        assert_eq!(identify["d"]["token"], "SyntheticPrivateBotToken");
        ready(socket);
        chat(socket);
        receipt.recv_timeout(Duration::from_secs(2)).unwrap();
        tx.send(envelope(
            3,
            HostMessage::Event {
                event: HostEvent::AppStopping {
                    app_run_id: uuid::Uuid::from_u128(3),
                },
            },
        ))
        .unwrap();
    });
    struct Receipt {
        bytes: Vec<u8>,
        notify: mpsc::Sender<()>,
    }
    impl Write for Receipt {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            let _ = self.notify.send(());
            Ok(())
        }
    }
    let mut writer = Receipt {
        bytes: vec![],
        notify,
    };
    let mut host = host();
    let mut gateway = gateway();
    assert_eq!(
        drive(
            &mut socket,
            &input,
            &mut host,
            &mut gateway,
            &mut writer,
            Instant::now()
        )
        .unwrap(),
        SessionExit::Stopped
    );
    assert!(host.stopped());
    let action = parse_action_line(&writer.bytes).unwrap();
    let Action::ChatOffer { message } = action.action else {
        panic!("expected offered chat")
    };
    assert_eq!(message.text, "local fixture original");
    assert_eq!(message.source_id, "discord:111:222:333");
    assert!(
        !String::from_utf8(writer.bytes)
            .unwrap()
            .contains("SyntheticPrivateBotToken")
    );
    finish(socket, done, server);
}
#[test]
fn missing_heartbeat_ack_returns_reconnect_without_waiting_for_socket_close() {
    let (input, _tx) = input();
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        read_op(socket, 2);
        ready(socket);
        read_op(socket, 1);
    });
    let mut host = host();
    let mut gateway = gateway();
    let mut writer = vec![];
    let start = Instant::now();
    assert_eq!(
        drive(
            &mut socket,
            &input,
            &mut host,
            &mut gateway,
            &mut writer,
            start
        )
        .unwrap(),
        SessionExit::Reconnect(Duration::from_secs(1))
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(writer.is_empty());
    finish(socket, done, server);
}
#[test]
fn recoverable_socket_session_resumes_and_delivers_replay_once() {
    let (input, _tx) = input();
    let mut host = host();
    let mut gateway = gateway();
    let mut writer = vec![];
    let start = Instant::now();
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        read_op(socket, 2);
        ready(socket);
        send_json(socket, json!({"op":7,"d":null}));
    });
    assert!(matches!(
        drive(
            &mut socket,
            &input,
            &mut host,
            &mut gateway,
            &mut writer,
            start
        )
        .unwrap(),
        SessionExit::Reconnect(_)
    ));
    finish(socket, done, server);
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        let resume = read_op(socket, 6);
        assert_eq!(resume["d"]["session_id"], "local-session");
        assert_eq!(resume["d"]["seq"], 1);
        chat(socket);
        chat(socket);
        send_json(socket, json!({"op":0,"t":"RESUMED","s":3,"d":{}}));
        send_json(socket, json!({"op":7,"d":null}));
    });
    gateway.connected(start.elapsed()).unwrap();
    let mut last = None;
    let mut failures = 0;
    assert!(matches!(
        run_session(
            &mut socket,
            Session {
                input: &input,
                host: &mut host,
                gateway: &mut gateway,
                start,
                token: "SyntheticPrivateBotToken",
                writer: &mut writer,
                last_identify: &mut last,
                failures: &mut failures,
                heartbeat_jitter: || 0.0,
                identify_gate: &mut || panic!("Resume must not reserve an Identify slot"),
            }
        )
        .unwrap(),
        SessionExit::Reconnect(_)
    ));
    assert_eq!(writer.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert!(matches!(
        parse_action_line(&writer).unwrap().action,
        Action::ChatOffer { .. }
    ));
    finish(socket, done, server);
}
#[test]
fn authentication_close_is_terminal_and_quiesces_host() {
    let (input, _tx) = input();
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        read_op(socket, 2);
        socket
            .close(Some(tungstenite::protocol::CloseFrame {
                code: 4004.into(),
                reason: "synthetic auth refusal".into(),
            }))
            .unwrap();
    });
    let mut host = host();
    let mut gateway = gateway();
    let mut writer = vec![];
    assert!(
        drive(
            &mut socket,
            &input,
            &mut host,
            &mut gateway,
            &mut writer,
            Instant::now()
        )
        .is_err()
    );
    assert!(host.stopped());
    assert!(writer.is_empty());
    finish(socket, done, server);
}
#[test]
fn binary_input_is_terminal_and_eof_interrupts_silent_socket() {
    let (input, _tx) = input();
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        read_op(socket, 2);
        socket.send(Message::Binary(vec![1, 2, 3].into())).unwrap();
    });
    let mut host1 = host();
    let mut gateway1 = gateway();
    let mut writer = vec![];
    assert!(
        drive(
            &mut socket,
            &input,
            &mut host1,
            &mut gateway1,
            &mut writer,
            Instant::now()
        )
        .is_err()
    );
    assert!(host1.stopped());
    finish(socket, done, server);
    let (input, _tx) = self::input();
    let status = input.status.clone();
    let (mut socket, server, done) = pair(move |socket| {
        hello(socket);
        read_op(socket, 2);
        status.store(EOF, Ordering::Release);
    });
    let mut host2 = host();
    let mut gateway2 = gateway();
    let start = Instant::now();
    assert_eq!(
        drive(
            &mut socket,
            &input,
            &mut host2,
            &mut gateway2,
            &mut writer,
            start
        )
        .unwrap(),
        SessionExit::Stopped
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(writer.is_empty());
    finish(socket, done, server);
}

#[test]
fn shared_identify_wait_prevents_authentication_control_write() {
    let (input, _tx) = input();
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        // Driver must return its wait decision before any Identify is written.
        assert!(!matches!(socket.read(), Ok(Message::Text(_))));
    });
    let mut host = host();
    let mut gateway = gateway();
    let start = Instant::now();
    gateway.connected(start.elapsed()).unwrap();
    let mut writer = vec![];
    let mut last = None;
    let mut failures = 0;
    let mut checks = 0;
    let mut gate = || {
        checks += 1;
        Ok(crate::discord_identify::Admission::Wait(
            Duration::from_millis(4200),
        ))
    };
    let result = run_session(
        &mut socket,
        Session {
            input: &input,
            host: &mut host,
            gateway: &mut gateway,
            start,
            token: "SyntheticPrivateBotToken",
            writer: &mut writer,
            last_identify: &mut last,
            failures: &mut failures,
            heartbeat_jitter: || 0.5,
            identify_gate: &mut gate,
        },
    )
    .unwrap();
    assert_eq!(
        result,
        SessionExit::IdentifyWait(Duration::from_millis(4200))
    );
    assert_eq!(checks, 1);
    assert!(last.is_none());
    assert!(writer.is_empty());
    finish(socket, done, server);
}

#[test]
fn host_cancellation_after_reservation_prevents_identify_write() {
    let (input, _tx) = input();
    let status = input.status.clone();
    let (mut socket, server, done) = pair(|socket| {
        hello(socket);
        assert!(!matches!(socket.read(), Ok(Message::Text(_))));
    });
    let mut host = host();
    let mut gateway = gateway();
    let start = Instant::now();
    gateway.connected(start.elapsed()).unwrap();
    let mut writer = vec![];
    let mut last = None;
    let mut failures = 0;
    let mut reserved = 0;
    let mut gate = || {
        reserved += 1;
        status.store(EOF, Ordering::Release);
        Ok(crate::discord_identify::Admission::Reserved)
    };
    let result = run_session(
        &mut socket,
        Session {
            input: &input,
            host: &mut host,
            gateway: &mut gateway,
            start,
            token: "SyntheticPrivateBotToken",
            writer: &mut writer,
            last_identify: &mut last,
            failures: &mut failures,
            heartbeat_jitter: || 0.5,
            identify_gate: &mut gate,
        },
    )
    .unwrap();
    assert_eq!(result, SessionExit::Stopped);
    assert_eq!(reserved, 1);
    assert!(last.is_none());
    assert!(host.stopped());
    assert!(writer.is_empty());
    finish(socket, done, server);
}
