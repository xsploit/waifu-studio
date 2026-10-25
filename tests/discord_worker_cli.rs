#![cfg(target_os = "linux")]
//! Only pre-network failure paths. No test supplies a bot token or connects.
use serde_json::json;
use std::{
    io::Write,
    process::{Command, Stdio},
};
fn run(input: &[u8], data: &std::path::Path) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_waifu-studio"))
        .arg("--discord-extension")
        .env("WAIFU_EXTENSION_DATA", data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}
fn handshake(grants: serde_json::Value, settings: serde_json::Value) -> Vec<u8> {
    let mut line=serde_json::to_vec(&json!({"api_version":1,"type":"host","sequence":1,
        "session_id":"00000000-0000-4000-8000-000000000001","process_generation":1,
        "message":{"type":"handshake","origin":{"integration_id":"discord-cli-fixture","display_name":"Discord"},"capabilities":grants,"settings":settings}})).unwrap();
    line.push(b'\n');
    line
}
#[test]
fn rejects_bad_handshake_permissions_configuration_and_missing_token_before_network() {
    let data = std::env::temp_dir().join(format!("discord-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data).unwrap();
    for (input, expected) in [
        (b"invalid\n".to_vec(), "Invalid Discord host message"),
        (
            handshake(json!([]), json!({})),
            "Discord needs observation and external-message permissions",
        ),
        (
            handshake(json!(["lifecycle.observe", "message.deliver"]), json!({})),
            "Discord requires explicit server and channel IDs",
        ),
        (
            handshake(
                json!(["lifecycle.observe", "message.deliver"]),
                json!({"guild_id":"111","channel_id":"222"}),
            ),
            "Set a valid Discord bot token",
        ),
    ] {
        let result = run(&input, &data);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    std::fs::remove_dir_all(data).unwrap();
}
#[test]
fn discord_packages_validate_without_execution() {
    for folder in ["discord-live", "discord-replay"] {
        let output = Command::new(env!("CARGO_BIN_EXE_waifu-studio"))
            .arg("--check-extension")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/extensions")
                    .join(folder),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
