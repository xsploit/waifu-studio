//! Opt-in live verification of production memory and motion decision adapters.
//! Uses synthetic notes in an isolated folder, never personal chat or memory.
#[path = "../src/motion_decision.rs"]
mod motion_decision;
#[path = "../src/motion_library.rs"]
mod motion_library;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::{
    credentials,
    data::Library,
    facts_memory::{FactsMemoryState, RecordKind},
    memory::MemoryState,
    semantic_memory::{MemorySources, SemanticMemory},
};

fn wait(mut ready: impl FnMut() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !ready() {
        if Instant::now() > deadline {
            return Err("Verification timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn main() -> Result<(), String> {
    if !std::env::args().any(|a| a == "--run") {
        println!("Explicit live verification: --run --output DIRECTORY");
        return Ok(());
    }
    let args: Vec<_> = std::env::args().collect();
    let output = PathBuf::from(
        args.windows(2)
            .find(|a| a[0] == "--output")
            .ok_or("Missing --output")?[1]
            .clone(),
    );
    std::fs::create_dir_all(&output).map_err(|_| "Cannot create output")?;
    let data = directories::ProjectDirs::from("", "", "waifu-studio")
        .ok_or("No app directory")?
        .data_local_dir()
        .to_owned();
    // SAFETY: process startup, before any worker or client exists.
    unsafe { credentials::load_startup(&data.join("credentials.env")) }
        .map_err(|_| "Cannot load credentials")?;
    let library: Library = serde_json::from_slice(
        &std::fs::read(data.join("library.json")).map_err(|_| "Cannot read profile")?,
    )
    .map_err(|_| "Invalid profile")?;
    let character = Some(Uuid::new_v4());
    let mut memory = MemoryState::load(output.join("synthetic-notes"));
    let mut facts = FactsMemoryState::load(output.join("synthetic-evidence"));
    memory.select_character(character);
    facts.select_character(character);
    wait(|| {
        memory.poll();
        facts.poll();
        memory.loaded && facts.loaded && !memory.busy && !facts.busy
    })?;
    memory.set_enabled(true)?;
    wait(|| {
        memory.poll();
        !memory.busy
    })?;
    memory.add_note("My favorite hot drink is jasmine tea.".into())?;
    facts.add_record(
        RecordKind::Preference,
        "I enjoy jasmine tea in the evening.".into(),
        "Synthetic verification fixture; not a user claim.".into(),
    )?;
    wait(|| {
        memory.poll();
        facts.poll();
        !memory.busy && !facts.busy
    })?;
    let mut semantic = SemanticMemory::load(output.join("synthetic-vectors"));
    wait(|| {
        let source = MemorySources {
            scope: semantic.scope(),
            memory: &memory,
            facts: &facts,
        };
        semantic.sync(character, &source);
        semantic.settings_loaded()
    })?;
    let started = Instant::now();
    wait(|| {
        let source = MemorySources {
            scope: semantic.scope(),
            memory: &memory,
            facts: &facts,
        };
        semantic.sync(character, &source);
        semantic.ready(character, &source)
    })?;
    let query = "Which warm beverage do I prefer?";
    {
        let source = MemorySources {
            scope: semantic.scope(),
            memory: &memory,
            facts: &facts,
        };
        semantic.query(character, &source, query);
    }
    wait(|| {
        let source = MemorySources {
            scope: semantic.scope(),
            memory: &memory,
            facts: &facts,
        };
        semantic.sync(character, &source);
        semantic.has_result_for(character, &source, query)
    })?;
    let recalled = {
        let source = MemorySources {
            scope: semantic.scope(),
            memory: &memory,
            facts: &facts,
        };
        semantic.take_context_for(character, &source, query)
    }
    .ok_or("No semantic matches")?;
    std::fs::write(output.join("synthetic-recall.txt"), &recalled)
        .map_err(|_| "Cannot write synthetic recall")?;
    if !recalled.contains("favorite hot drink") || !recalled.contains("evening") {
        return Err("Combined memory recall missed a source".into());
    }
    let memory_ms = started.elapsed().as_millis();
    let entries = motion_library::scan(&motion_library::roots());
    let allowed = motion_library::MotionCategory::ALL
        .into_iter()
        .filter(|c| *c != motion_library::MotionCategory::Locomotion)
        .collect::<Vec<_>>();
    let catalog =
        motion_library::MotionCatalog::offer(&entries, &allowed, motion_library::MAX_OFFERED);
    let offered = catalog.options().count();
    if offered <= 16 {
        return Err("Motion decision still has a truncated catalog".into());
    }
    let started = Instant::now();
    let choice = motion_decision::choose(
        &library.decision,
        "Hikari",
        "Character emotion: excited. I am so happy to see you! Let me give you an enthusiastic wave!",
        &catalog,
        &|| false,
    )?;
    let label = choice
        .as_ref()
        .and_then(|id| catalog.options().find(|o| o.id == *id))
        .map(|o| o.label.clone());
    let receipt = serde_json::json!({"memory":{"passed":true,"endpoint":semantic.settings.base_url,"model":semantic.settings.model,"scope":semantic.settings.scope,"sources":2,"latency_ms":memory_ms},"motion":{"passed":true,"decision_model":library.decision.model,"scanned":entries.len(),"offered":offered,"chosen":label,"latency_ms":started.elapsed().as_millis()}});
    std::fs::write(
        output.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .map_err(|_| "Cannot write receipt")?;
    println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
    Ok(())
}
