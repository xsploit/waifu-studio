//! Offline installed-bundle verification. Uses only a fresh temporary store.
//! Never supply credentials: a valid handshake must fail before bot construction.
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use waifu_studio::{
    extension_process::{ProcessEvent, ProcessHandle, ProcessSpec},
    extension_protocol::Capability,
    extension_store::ExtensionStore,
};

struct Cleanup(PathBuf);
impl Cleanup {
    fn finish(&mut self) -> Result<(), String> {
        fs::remove_dir_all(&self.0)
            .map_err(|e| format!("Temporary verifier store cleanup failed: {e}"))?;
        self.0.clear();
        Ok(())
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn verify_process(
    program: PathBuf,
    directory: PathBuf,
    data: PathBuf,
    wire: &[u8],
) -> Result<(), String> {
    let mut process = ProcessHandle::spawn(ProcessSpec {
        program,
        args: vec![],
        working_directory: directory,
        data_directory: data.clone(),
        lifetime: None,
    })?;
    let result = (|| {
        process.send_line(wire)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut logs = Vec::new();
        let mut lines = 0;
        loop {
            while let Some(event) = process.poll() {
                match event {
                    ProcessEvent::Log(line) => logs.push(line),
                    ProcessEvent::Line(_) => lines += 1,
                    _ => {}
                }
            }
            if process.finished() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Installed worker did not terminate".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // Drain once more after the authoritative supervisor completion.
        while let Some(event) = process.poll() {
            match event {
                ProcessEvent::Log(line) => logs.push(line),
                ProcessEvent::Line(_) => lines += 1,
                _ => {}
            }
        }
        process
            .started()
            .ok_or("No installed-worker spawn receipt")?;
        let terminal = process
            .termination()
            .ok_or("No installed-worker reap receipt")?;
        if terminal.code != Some(1)
            || lines != 0
            || logs != ["Discord voice worker stopped or cleanup failed"]
        {
            return Err(format!(
                "Unexpected fixed failure receipt: code={:?}, lines={lines}, logs={logs:?}",
                terminal.code
            ));
        }
        // The retained supervisor termination receipt proves our direct child
        // was reaped. A raw PID probe could instead observe a reused PID.
        if fs::read_dir(&data)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
        {
            return Err("Credential-free worker created unexpected data".into());
        }
        Ok(())
    })();
    if !process.shutdown(Duration::from_secs(2)) {
        return Err(match result {
            Ok(()) => "Installed worker cleanup exceeded bound".into(),
            Err(cause) => format!("{cause}; installed worker cleanup exceeded bound"),
        });
    }
    result
}

fn verify_installed_lifecycle(
    mut store: ExtensionStore,
    id: uuid::Uuid,
    grants: &[Capability],
    store_root: &std::path::Path,
) -> Result<ExtensionStore, String> {
    use waifu_studio::{
        animation::AnimationState,
        behavior::BehaviorController,
        extension_actions::{ActionCatalog, ActionControllers},
        extension_installed::InstalledRuntime,
        extension_lifecycle::{LifecycleEvent, LifecycleJournal},
        extension_runtime::ExtensionRuntime,
        extensions::PackView,
    };
    struct Guard(ExtensionRuntime);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.shutdown(Duration::from_secs(2));
        }
    }
    let guard = Guard(ExtensionRuntime::default());
    let runtime = &guard.0;
    let mut binding = InstalledRuntime::default();
    let mut animation = AnimationState::default();
    let mut behavior = BehaviorController::default();
    let catalog = ActionCatalog::from_packs([], &[]);
    let mut journal = LifecycleJournal::default();
    let mut starts = 0;
    let view = |store: &ExtensionStore| -> Result<PackView, String> {
        let installed = store
            .packs()
            .iter()
            .find(|p| p.installation_id == id)
            .unwrap();
        Ok(PackView {
            installed: installed.clone(),
            content: Some(store.resolved(installed)?),
            data_directory: Some(store.data_directory(installed)?),
            credentials: Default::default(),
            error: None,
        })
    };
    let mut tick = |store: &ExtensionStore| -> Result<usize, String> {
        binding.reconcile(&[view(store)?], runtime);
        if !binding.errors.is_empty() {
            return Err(format!("Reconciliation failed: {:?}", binding.errors));
        }
        runtime.frame(
            &[],
            None,
            false,
            &catalog,
            ActionControllers {
                external_messages: None,
                animation: &mut animation,
                behavior: &mut behavior,
            },
        );
        runtime.drain_observations(&mut journal);
        starts += journal
            .drain()
            .iter()
            .filter(|event| matches!(event.event, LifecycleEvent::ExtensionStarted { .. }))
            .count();
        Ok(starts)
    };
    tick(&store)?;
    if !runtime.is_empty() {
        return Err("Disabled installation started a runtime".into());
    }
    store.set_settings(
        id,
        serde_json::from_value(serde_json::json!({"guild_id":"111","channel_id":"222"})).unwrap(),
    )?;
    store.set_grants(id, grants.to_vec())?;
    for expected_starts in 1..=2 {
        store.set_enabled(id, true)?;
        store = verify_saved_state(store, store_root, id, true, grants)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let starts = tick(&store)?;
            let failed = runtime.views().iter().any(|v| {
                v.id == id
                    && !v.active
                    && v.logs
                        .iter()
                        .any(|line| line.contains("Discord voice worker stopped or cleanup failed"))
            });
            if failed && runtime.process_count() == 1 && starts == expected_starts {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Installed startup failure not retained: starts={starts}, expected={expected_starts}, views={:?}",
                    runtime.views()
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Repeated application frames must not automatically restart failed code.
        let observe_until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < observe_until {
            if tick(&store)? != expected_starts || runtime.process_count() != 1 {
                return Err("Failed installed worker restarted automatically".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        store.set_enabled(id, false)?;
        store = verify_saved_state(store, store_root, id, false, grants)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            tick(&store)?;
            if runtime.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Disabled installed worker was not removed".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    if !runtime.shutdown(Duration::from_secs(2)) {
        return Err("Runtime shutdown exceeded bound".into());
    }
    Ok(store)
}

fn verify_saved_state(
    store: ExtensionStore,
    root: &std::path::Path,
    id: uuid::Uuid,
    enabled: bool,
    grants: &[Capability],
) -> Result<ExtensionStore, String> {
    drop(store); // Release the exclusive store lock before actually reopening.
    let reopened = ExtensionStore::open(root.to_path_buf())?;
    let saved = reopened
        .packs()
        .iter()
        .find(|pack| pack.installation_id == id)
        .ok_or("Installation missing after store reopen")?;
    let expected_settings =
        serde_json::from_value(serde_json::json!({"guild_id":"111", "channel_id":"222"}))
            .map_err(|e| e.to_string())?;
    let mut expected_grants = grants.to_vec();
    expected_grants.sort_unstable();
    if saved.enabled != enabled
        || saved.grants != expected_grants
        || saved.settings != expected_settings
    {
        return Err("Enabled state, exact grants or voice settings did not persist".into());
    }
    Ok(reopened)
}

/// This verifier grants only the three known voice direction combinations,
/// never arbitrary capabilities declared by a package supplied on the CLI.
fn voice_grants(capabilities: &[String]) -> Result<(&'static str, Vec<Capability>), String> {
    let receive = ["audio.transcribe", "message.deliver", "lifecycle.observe"];
    let playback = ["audio.playback"];
    let duplex = [
        "audio.transcribe",
        "message.deliver",
        "lifecycle.observe",
        "audio.playback",
    ];
    let mode = [
        ("receive", receive.as_slice()),
        ("playback", playback.as_slice()),
        ("duplex", duplex.as_slice()),
    ]
    .into_iter()
    .find_map(|(mode, expected)| {
        (capabilities.len() == expected.len()
            && expected
                .iter()
                .all(|cap| capabilities.iter().any(|value| value == cap)))
        .then_some(mode)
    })
    .ok_or("Unexpected Discord voice capabilities")?;
    let grants =
        serde_json::from_value(serde_json::json!(capabilities)).map_err(|e| e.to_string())?;
    Ok((mode, grants))
}

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(
        args.next()
            .ok_or("Usage: verify_discord_voice_bundle BUNDLE")?,
    );
    if args.next().is_some() {
        return Err("Supply one bundle directory".into());
    }
    let root = std::env::temp_dir().join(format!("waifu-installed-voice-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    let mut cleanup = Cleanup(root.clone());
    let mut store = ExtensionStore::open(root.join("store"))?;
    let id = store.install_integration(&source)?;
    let installed = store
        .packs()
        .iter()
        .find(|p| p.installation_id == id)
        .unwrap();
    if installed.enabled || !installed.grants.is_empty() {
        return Err("Unexpected automatic enable/grant".into());
    }
    let resolved = store.resolved(installed)?;
    if resolved.manifest.id != "studio.integration.discord-voice" {
        return Err("Expected Discord voice bundle identity".into());
    }
    let (mode, grants) = voice_grants(&resolved.manifest.capabilities)?;
    let directory = store.directory(installed);
    let data = store.data_directory(installed)?;
    if fs::read_dir(&data)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("Expected empty temporary data".into());
    }
    let program = directory.join("run.sh");
    verify_process(program.clone(), directory.clone(), data.clone(), b"invalid")?;
    let handshake = serde_json::to_vec(&serde_json::json!({
        "api_version":1,"type":"host","sequence":1,
        "session_id":"11111111-1111-4111-8111-111111111111","process_generation":1,
        "message":{"type":"handshake","settings":{"guild_id":"111","channel_id":"222"},
            "capabilities":grants}
    }))
    .map_err(|e| e.to_string())?;
    verify_process(program, directory, data, &handshake)?;
    let store = verify_installed_lifecycle(store, id, &grants, &root.join("store"))?;
    drop(store);
    let reopened = ExtensionStore::open(root.join("store"))?;
    if reopened
        .packs()
        .iter()
        .find(|p| p.installation_id == id)
        .unwrap()
        .enabled
    {
        return Err("Disabled state did not survive reopening the store".into());
    }
    drop(reopened);
    cleanup.finish()?;
    println!(
        "Installed {mode} launcher passed Rust supervision: malformed input and valid handshake without credentials; no actions or data writes; direct workers reaped. Installed enable/fail/disable/re-enable verified without automatic retry during two-second observation windows. No live connection tested."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn caps(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn known_modes_preserve_exact_grants_independent_of_order() {
        for (expected, names) in [
            (
                "receive",
                vec!["message.deliver", "lifecycle.observe", "audio.transcribe"],
            ),
            ("playback", vec!["audio.playback"]),
            (
                "duplex",
                vec![
                    "audio.playback",
                    "audio.transcribe",
                    "lifecycle.observe",
                    "message.deliver",
                ],
            ),
        ] {
            let values = caps(&names);
            let (mode, grants) = voice_grants(&values).unwrap();
            assert_eq!(mode, expected);
            assert_eq!(
                serde_json::to_value(grants).unwrap(),
                serde_json::json!(values)
            );
        }
    }
    #[test]
    fn partial_duplicate_extra_and_unrelated_permissions_are_not_granted() {
        for names in [
            vec![],
            vec!["audio.transcribe"],
            vec!["audio.playback", "audio.playback"],
            vec!["audio.playback", "motion.play"],
            vec!["message.deliver", "lifecycle.observe"],
            vec!["audio.playback", "unknown"],
        ] {
            assert!(voice_grants(&caps(&names)).is_err());
        }
    }
}
