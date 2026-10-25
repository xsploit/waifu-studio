//! Real binary boundary: checks precede app startup and never run declared code.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("extension-cli-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn run(&self, args: &[&std::ffi::OsStr]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_waifu-studio"))
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("XDG_DATA_HOME", self.0.join("app-data"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("manifest check entered a long-lived path");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            !self.0.join("app-data").exists(),
            "checker initialized app data"
        );
        output
    }
    fn check(&self, path: &Path) -> Output {
        self.run(&[std::ffi::OsStr::new("--check-extension"), path.as_os_str()])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bundled_content_process_and_hook_manifests_check_without_app_startup() {
    let fixture = Fixture::new();
    for (folder, kind) in [
        ("companion-starter", "content-pack"),
        ("twitch-live", "integration"),
        ("reaction-hook", "integration"),
    ] {
        let result = fixture.check(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/extensions")
                .join(folder),
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains(&format!("Valid {kind} manifest")));
        assert!(text.contains("No code ran and nothing was installed"));
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn declared_script_is_not_executed_and_invalid_capabilities_fail() {
    let fixture = Fixture::new();
    let pack = fixture.0.join("pack");
    fs::create_dir(&pack).unwrap();
    let marker = fixture.0.join("EXECUTED");
    fs::write(
        pack.join("worker.sh"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(pack.join("worker.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut manifest = serde_json::json!({"api_version":1,"id":"org.example.check","version":"1.0.0","name":"Checker fixture","description":"No execution","provenance":{"author":"Fixture","license":"MIT"},"process":{"program":"worker.sh"},"capabilities":["lifecycle.observe"]});
    fs::write(
        pack.join("extension.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let before = fs::read(pack.join("extension.json")).unwrap();
    let result = fixture.check(&pack);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!marker.exists());
    assert_eq!(fs::read(pack.join("extension.json")).unwrap(), before);
    manifest["capabilities"] = serde_json::json!(["not.a.capability"]);
    fs::write(
        pack.join("extension.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let result = fixture.check(&pack);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!marker.exists());
}

#[test]
fn check_mode_rejects_missing_folder_extra_options_and_missing_assets() {
    use std::ffi::OsStr;
    let fixture = Fixture::new();
    assert!(
        !fixture
            .run(&[OsStr::new("--check-extension")])
            .status
            .success()
    );
    assert!(
        !fixture
            .run(&[
                OsStr::new("--check-extension"),
                fixture.0.as_os_str(),
                OsStr::new("--desktop")
            ])
            .status
            .success()
    );
    let combined_worker = fixture.run(&[
        OsStr::new("--twitch-extension"),
        OsStr::new("--check-extension"),
        fixture.0.as_os_str(),
    ]);
    assert!(!combined_worker.status.success());
    assert!(String::from_utf8_lossy(&combined_worker.stderr).contains("first option"));
    assert!(!fixture.check(&fixture.0.join("missing")).status.success());
    let manifest = serde_json::json!({"api_version":1,"id":"org.example.missing","version":"1.0.0","name":"Missing asset","description":"Fixture","provenance":{"author":"Fixture","license":"MIT"},"content":{"motions":[{"id":"missing","name":"Missing","asset":"missing.bvh","role":"gesture","loopable":false}]}});
    fs::write(
        fixture.0.join("extension.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let result = fixture.check(&fixture.0);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
}
