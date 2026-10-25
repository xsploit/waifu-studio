#![cfg(target_os = "linux")]
use std::{path::PathBuf, process::Command};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn root() -> Temp {
    let path = std::env::temp_dir().join(format!("waifu-identify-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    Temp(path)
}
fn run(root: &Temp, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_waifu-studio"))
        .args(args)
        .env_clear()
        .env("XDG_DATA_HOME", &root.0)
        .output()
        .unwrap()
}
#[test]
fn separate_processes_share_one_private_token_free_reservation() {
    let root = root();
    let first = run(&root, &["--discord-identify-admission"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(value, serde_json::json!({"version":1,"status":"reserved"}));
    let second = run(&root, &["--discord-identify-admission"]);
    assert!(second.status.success());
    assert!(second.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(value["status"], "wait");
    assert!((1..=5000).contains(&value["wait_ms"].as_u64().unwrap()));
    let file = root.0.join("waifu-studio/discord-identify/identify.lock");
    let state: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(state.as_object().unwrap().len(), 3);
    assert!(
        state.get("version").is_some()
            && state.get("boot").is_some()
            && state.get("next_ms").is_some()
    );
    use std::os::unix::fs::MetadataExt;
    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o077, 0);
}
#[test]
fn extra_or_misplaced_arguments_are_rejected_before_filesystem_admission() {
    let root = root();
    for args in [
        vec!["--discord-identify-admission", "extra"],
        vec!["extra", "--discord-identify-admission"],
    ] {
        let output = run(&root, &args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("Use --discord-identify-admission alone")
        );
    }
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
}
