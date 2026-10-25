#![cfg(target_os = "linux")]
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{extension_protocol::*, extension_store::ExtensionStore};

#[test]
fn installed_launcher_runs_native_worker_and_reports_missing_credentials_without_network() {
    let root = std::env::temp_dir().join(format!("waifu-native-twitch-{}", Uuid::new_v4()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let mut store = ExtensionStore::open(root.join("extensions")).unwrap();
    let id = store
        .install_integration(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/twitch-live"),
        )
        .unwrap();
    let installed = store
        .packs()
        .iter()
        .find(|pack| pack.installation_id == id)
        .unwrap();
    let pack = store.resolved(installed).unwrap();
    let data = store.data_directory(installed).unwrap();
    let mut child = Command::new(pack.root.join("run.py"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(
            "WAIFU_STUDIO_EXECUTABLE",
            env!("CARGO_BIN_EXE_waifu-studio"),
        )
        .env("WAIFU_EXTENSION_DATA", data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let handshake = HostEnvelope {
        api_version: API_VERSION,
        message_type: HostEnvelopeType::Host,
        sequence: 1,
        session_id: Uuid::new_v4(),
        process_generation: 1,
        message: HostMessage::Handshake {
            origin: IntegrationOrigin {
                integration_id: installed.package_id.clone(),
                display_name: installed.name.clone(),
            },
            capabilities: vec![Capability::LifecycleObserve],
            settings: Default::default(),
        },
    };
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&encode_host_line(&handshake).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native Twitch startup did not stop promptly");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "Set the Twitch application ID in Credentials"
    );
}
