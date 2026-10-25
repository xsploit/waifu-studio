//! Owned localhost TLS fixtures. Only per-client test trust is changed.
use super::*;
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::net::{TcpListener, TcpStream};
const CA: &[u8] = include_bytes!("../tests/fixtures/tls/ca.der");
const PEER: &[u8] = include_bytes!("../tests/fixtures/tls/server.der");
const WRONG_PEER: &[u8] = include_bytes!("../tests/fixtures/tls/wrong-server.der");
const TEST_SERVER_PRIVATE_KEY: &[u8] = include_bytes!("../tests/fixtures/tls/server-key.der");
type ServerStream = StreamOwned<ServerConnection, TcpStream>;
fn server<R: Send + 'static>(
    wrong: bool,
    handler: impl FnOnce(ServerStream) -> R + Send + 'static,
) -> (u16, std::thread::JoinHandle<R>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(TEST_SERVER_PRIVATE_KEY.to_vec()));
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(
                    if wrong { WRONG_PEER } else { PEER }.to_vec(),
                )],
                key,
            )
            .unwrap();
    let task = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "local TLS client never connected"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("local TLS accept failed: {error}"),
            }
        };
        // Accepted sockets inherit nonblocking mode on Windows.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        handler(StreamOwned::new(
            ServerConnection::new(Arc::new(config)).unwrap(),
            stream,
        ))
    });
    (port, task)
}
fn trusted_http() -> reqwest::blocking::Client {
    discovery_client_builder()
        .no_proxy()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_der(CA).unwrap())
        .build()
        .unwrap()
}
fn trusted_connector() -> tungstenite::Connector {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(CA.to_vec())).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    tungstenite::Connector::Rustls(Arc::new(config))
}
fn no_application_request(mut stream: ServerStream) -> bool {
    let mut byte = [0];
    !matches!(stream.read(&mut byte),Ok(count) if count>0)
}
#[test]
fn trusted_fixture_https_completes_discovery_and_checks_peer_name() {
    let (port, task) = server(false, |mut stream| {
        let mut bytes = vec![];
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 8192);
        }
        let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /gateway/bot http/1.1\r\n"));
        assert!(request.contains("authorization: bot syntheticprivatebottoken\r\n"));
        let body=json!({"url":"wss://gateway.discord.gg","shards":1,"session_start_limit":{"total":1000,"remaining":999,"reset_after":60000,"max_concurrency":1}}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        stream.flush().unwrap();
        stream.conn.send_close_notify();
        let _ = stream.flush();
    });
    let info = request_discovery(
        &trusted_http(),
        &format!("https://127.0.0.1:{port}/gateway/bot"),
        "SyntheticPrivateBotToken",
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(info.remaining, 999);
    assert_eq!(
        info.url.as_str(),
        "wss://gateway.discord.gg/?v=10&encoding=json"
    );
    task.join().unwrap();
}
#[test]
fn https_rejects_untrusted_root_and_wrong_peer_before_bot_header() {
    for wrong in [false, true] {
        let (port, task) = server(wrong, no_application_request);
        let client = if wrong {
            trusted_http()
        } else {
            discovery_client_builder().no_proxy().build().unwrap()
        };
        let error = request_discovery(
            &client,
            &format!("https://127.0.0.1:{port}/gateway/bot"),
            "SyntheticPrivateBotToken",
            Duration::from_secs(2),
        )
        .err()
        .unwrap();
        assert_eq!(error, "Discord Gateway discovery could not connect");
        assert!(!error.contains("SyntheticPrivateBotToken"));
        assert!(task.join().unwrap());
    }
}
#[test]
#[allow(clippy::result_large_err)] // tungstenite defines the callback error type.
fn trusted_fixture_wss_upgrades_without_provider_headers_and_decodes_gateway_hello() {
    let (port, task) = server(false, |stream| {
        let mut ws = tungstenite::accept_hdr(
            stream,
            |request: &tungstenite::handshake::server::Request, response| {
                assert_eq!(request.uri().path(), "/");
                assert!(!request.headers().contains_key("authorization"));
                assert!(!request.headers().contains_key("model"));
                Ok(response)
            },
        )
        .unwrap();
        ws.send(Message::Text(
            json!({"op":10,"d":{"heartbeat_interval":10000}})
                .to_string()
                .into(),
        ))
        .unwrap();
        let _ = ws.close(None);
    });
    let url = reqwest::Url::parse(&format!("wss://127.0.0.1:{port}/?v=10&encoding=json")).unwrap();
    let mut socket =
        crate::fish_ws::observation_socket_fixture_tls(&url, Some(trusted_connector())).unwrap();
    let Message::Text(hello) = socket.read().unwrap() else {
        panic!("missing Gateway Hello")
    };
    let mut gateway = Gateway::new(
        crate::discord_messages::ChannelFilter::new("111".into(), "222".into()).unwrap(),
    );
    gateway.connected(Duration::ZERO).unwrap();
    assert_eq!(
        gateway
            .packet(hello.as_bytes(), Duration::ZERO, 0.5)
            .unwrap(),
        vec![Effect::Identify]
    );
    drop(socket);
    task.join().unwrap();
}
#[test]
fn wss_rejects_untrusted_root_and_wrong_peer_before_websocket_upgrade() {
    for wrong in [false, true] {
        let (port, task) = server(wrong, no_application_request);
        let url = reqwest::Url::parse(&format!("wss://127.0.0.1:{port}/")).unwrap();
        let connector = if wrong {
            Some(trusted_connector())
        } else {
            None
        };
        let error = crate::fish_ws::observation_socket_fixture_tls(&url, connector)
            .err()
            .unwrap();
        assert_eq!(error, "Integration WebSocket handshake failed");
        assert!(task.join().unwrap());
    }
}
