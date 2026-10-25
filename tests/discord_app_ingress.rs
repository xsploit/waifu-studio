#![cfg(any(target_os = "linux", windows))]
//! Actual app binary, unchanged native replay launcher, real sink and disk history.
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn finish(mut child: std::process::Child) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("offline application replay did not finish");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn native_replay_reaches_application_history_without_models_memory_or_profile_leaks() {
    let output = finish(
        Command::new(env!("CARGO_BIN_EXE_waifu-studio"))
            .arg("--verify-discord-replay")
            .env(
                "WAIFU_STUDIO_ASSET_ROOT",
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(report["history_rows"], 1);
    assert_eq!(report["role"], "external");
    assert_eq!(report["origin_id"], "studio.example.discord-replay");
    assert_eq!(report["original_count"], 1);
    for field in [
        "native_process_observed",
        "queued_before_delivery",
        "disk_matches",
        "memory_unchanged",
        "draft_preserved",
        "credential_free",
        "defaults_disabled",
        "headless_target",
        "cleanup_verified",
    ] {
        assert_eq!(report[field], true, "missing evidence for {field}");
    }
    assert_eq!(report["model_work_started"], false);
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn verification_rejects_profile_arguments_before_opening_any_profile() {
    let root = std::env::temp_dir().join(format!("discord-check-args-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let sentinel = root.join("sentinel");
    std::fs::write(&sentinel, b"keep").unwrap();
    let output = finish(
        Command::new(env!("CARGO_BIN_EXE_waifu-studio"))
            .arg("--verify-discord-replay")
            .arg(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Use --verify-discord-replay alone"));
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_dir_all(root).unwrap();
}
