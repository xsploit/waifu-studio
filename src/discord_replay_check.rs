//! Developer-only headless application check; owns a fresh temporary profile.
use super::*;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use waifu_studio::{
    extension_installed::InstalledRuntime, extension_protocol::Capability,
    extension_store::ExtensionStore, extensions::PackView,
};

fn exercise(root: &Path, runtime: &ExtensionRuntime) -> Result<serde_json::Value, String> {
    let (store, mut library) = waifu_studio::data::Store::open(root.join("profile"))?;
    let character = Uuid::new_v4();
    library.characters.push(serde_json::from_value(serde_json::json!({"id":character,"name":"Discord intake fixture","model":"fixture.vrm","persona":"Owned offline fixture","format":"VRM 1.0"})).map_err(|_|"Cannot create offline character fixture")?);
    library.selected = Some(character);
    library.scene.reply_to_external_messages = false;
    library.scene.select_incoming_messages = true; // Must remain gated by reply opt-in.
    library.scene.memory_triage.enabled = true;
    library.voice.enabled = false;
    library.decision.endpoint = "://offline-disabled".into();
    library.decision.model.clear();
    library.decision.key_env = "WAIFU_OFFLINE_REPLAY_UNUSED".into();
    for provider in &mut library.providers {
        provider.base_url = "://offline-disabled".into();
        provider.model.clear();
    }
    let mut studio = Studio::new(store, library);
    studio.model_discovery_enabled = false;
    studio.composer = "Preserve the local draft".into();
    studio.facts.enabled = true;
    studio.facts.candidates_enabled = true;
    let mut memory = MemoryState::load(root.join("notes"));
    memory.enabled = true;
    memory.select_character(Some(character));
    let loaded = Instant::now() + Duration::from_secs(3);
    loop {
        studio.facts.poll();
        memory.poll();
        if studio.facts.loaded && !studio.facts.busy && memory.loaded && !memory.busy {
            break;
        }
        if Instant::now() >= loaded {
            return Err("Offline memory fixture did not settle".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    studio.facts.triage_status = "offline replay idle".into();
    let facts_generation = studio.facts.generation();
    let mut store = ExtensionStore::open(root.join("extensions"))?;
    let id = store.install_integration(
        &waifu_studio::bundled::extension_examples_path().join("discord-replay"),
    )?;
    let defaults_disabled = !store.packs()[0].enabled && store.packs()[0].grants.is_empty();
    if !defaults_disabled {
        return Err("Replay installation did not preserve opt-in defaults".into());
    }
    store.set_grants(
        id,
        vec![Capability::LifecycleObserve, Capability::MessageDeliver],
    )?;
    store.set_enabled(id, true)?;
    let installed = store.packs()[0].clone();
    let content = store.resolved(&installed)?;
    let data = store.data_directory(&installed)?;
    let credential_free =
        content.manifest.credentials.is_empty() && !data.join("credentials.json").exists();
    let view = PackView {
        installed,
        content: Some(content),
        data_directory: Some(data),
        credentials: Default::default(),
        error: None,
    };
    let mut binder = InstalledRuntime::default();
    let mut incoming = IncomingChat::default();
    let mut picker = ChatPickerState::default();
    let mut animation = AnimationState::default();
    let mut behavior = waifu_studio::behavior::BehaviorController::default();
    let target = Target {
        uuid: character,
        generation: 1,
    };
    let source_revision = Uuid::new_v4();
    let mut accepted_before_delivery = false;
    let mut observed_native_start = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        binder.reconcile(std::slice::from_ref(&view), runtime);
        if !binder.errors.is_empty() {
            return Err("Installed replay could not start".into());
        }
        let generation = runtime.chat_source_generation(id);
        let allowed: BTreeMap<String, Uuid> = if generation.is_some() {
            [(view.installed.package_id.clone(), source_revision)].into()
        } else {
            BTreeMap::new()
        };
        let now = Instant::now();
        incoming.reconcile(Some(target), &allowed, now);
        {
            let mut sink = StudioExternalMessages {
                outputs: None,
                audio: None,
                studio: &mut studio,
                memory: &mut memory,
                scene_busy: false,
                target: Some(target),
                incoming: &mut incoming,
                picker: &mut picker,
                allowed: &allowed,
            };
            sink.flush_chat(now);
            runtime.frame(
                &[],
                Some(target),
                false,
                &ActionCatalog::from_packs([], &[]),
                ActionControllers {
                    animation: &mut animation,
                    behavior: &mut behavior,
                    external_messages: Some(&mut sink),
                },
            );
        }
        observed_native_start |= runtime
            .views()
            .iter()
            .any(|entry| entry.id == id && entry.active);
        // A successful offer is queued for the real four-second chunk window.
        accepted_before_delivery |= incoming.has_pending() && studio.turns.is_empty();
        if !studio.turns.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            return Err("Installed replay did not reach application history".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    studio.facts.poll();
    memory.poll();
    if studio.turns.len() != 1 {
        return Err("Replay created duplicate application turns".into());
    }
    let turn = &studio.turns[0];
    let external = turn
        .external
        .as_ref()
        .ok_or("Replay lost external attribution")?;
    if turn.role != "external"
        || external.origin.integration_id != "studio.example.discord-replay"
        || external.origin.display_name != "Discord replay (offline)"
    {
        return Err("Replay attribution changed at application intake".into());
    }
    let origin_id = external.origin.integration_id.clone();
    let chunk: serde_json::Value = serde_json::from_str(&turn.content)
        .map_err(|_| "Replay history did not contain a structured chunk")?;
    let originals: Vec<ChatMessage> = serde_json::from_value(chunk["messages"].clone())
        .map_err(|_| "Replay history originals are invalid")?;
    if chunk["kind"] != "chat_chunk"
        || originals.len() != 1
        || originals[0].author_id != "555"
        || originals[0].author_name != "Replay viewer"
        || originals[0].text != "Synthetic Discord replay: hello! This is an offline test message."
        || !originals[0].source_id.starts_with("discord:111:222:")
    {
        return Err("Replay originals changed before persistence".into());
    }
    if studio.store.history(character)? != studio.turns {
        return Err("Replay disk history differs from visible application history".into());
    }
    let no_model_work = !studio.has_external_response_work()
        && studio.pending_response.is_empty()
        && picker.pending.is_none()
        && !studio.voice_busy;
    let memory_unchanged = !studio.facts.busy
        && !memory.busy
        && studio.facts.triage_status == "offline replay idle"
        && studio.facts.generation() == facts_generation
        && studio.facts.candidates().is_empty()
        && studio.facts.records().is_empty()
        && memory.notes().is_empty();
    let draft_preserved = studio.composer == "Preserve the local draft";
    if !no_model_work
        || !memory_unchanged
        || !draft_preserved
        || !accepted_before_delivery
        || !credential_free
        || !observed_native_start
    {
        return Err("Offline replay violated application intake boundaries".into());
    }
    // Flush once more through the actual sink to prove this chunk is consumed.
    {
        let allowed = [(view.installed.package_id.clone(), source_revision)].into();
        let mut sink = StudioExternalMessages {
            outputs: None,
            audio: None,
            studio: &mut studio,
            memory: &mut memory,
            scene_busy: false,
            target: Some(target),
            incoming: &mut incoming,
            picker: &mut picker,
            allowed: &allowed,
        };
        sink.flush_chat(Instant::now());
    }
    if studio.turns.len() != 1 {
        return Err("Replay chunk was delivered twice".into());
    }
    if view
        .data_directory
        .as_ref()
        .is_none_or(|data| data.join("credentials.json").exists())
    {
        return Err("Offline replay unexpectedly created credential state".into());
    }
    runtime.begin_shutdown(&mut animation, &mut behavior);
    Ok(
        serde_json::json!({"status":"ok","native_process_observed":observed_native_start,"history_rows":1,"role":"external","origin_id":origin_id,"original_count":1,"queued_before_delivery":accepted_before_delivery,"disk_matches":true,"memory_unchanged":memory_unchanged,"model_work_started":!no_model_work,"draft_preserved":draft_preserved,"credential_free":credential_free,"defaults_disabled":defaults_disabled,"headless_target":true}),
    )
}

pub(crate) fn run() -> Result<(), String> {
    let root = std::env::temp_dir().join(format!("waifu-discord-intake-check-{}", Uuid::new_v4()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&root)
        .map_err(|_| "Cannot create private temporary replay profile")?;
    let runtime = ExtensionRuntime::default();
    struct Cleanup {
        root: std::path::PathBuf,
        runtime: ExtensionRuntime,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.runtime.shutdown(Duration::from_secs(2));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    let _cleanup = Cleanup {
        root: root.clone(),
        runtime: runtime.clone(),
    };
    let result = exercise(&root, &runtime);
    let stopped = runtime.shutdown(Duration::from_secs(2));
    let removed = std::fs::remove_dir_all(&root).is_ok() && !root.exists();
    if !stopped {
        return Err("Offline replay process did not finish shutdown".into());
    }
    if !removed {
        return Err("Cannot remove temporary replay profile".into());
    }
    let mut report = result?;
    report["cleanup_verified"] = serde_json::json!(true);
    println!("{report}");
    Ok(())
}
