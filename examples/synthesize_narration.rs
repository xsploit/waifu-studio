//! Explicit Fish narration using the selected saved voice and production adapter.
//! No playback, key logging, voice substitution, or saved-setting changes.
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use waifu_studio::{
    credentials,
    data::Library,
    voice::{VoiceAdapter, VoiceDelivery, VoiceEvent, VoiceWorker},
};

fn arg(name: &str) -> Result<String, String> {
    std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|a| a[0] == name)
        .map(|a| a[1].clone())
        .ok_or_else(|| format!("Missing {name}"))
}
fn main() -> Result<(), String> {
    if !std::env::args().any(|a| a == "--run") {
        println!("Explicit paid synthesis: --run --storyboard FILE.json --output-dir EMPTY_DIR");
        return Ok(());
    }
    let directory = directories::ProjectDirs::from("", "", "waifu-studio")
        .ok_or("No app directory")?
        .data_local_dir()
        .to_path_buf();
    // SAFETY: startup, before worker threads and HTTP clients.
    unsafe { credentials::load_startup(&directory.join("credentials.env")) }
        .map_err(|_| "Could not load credentials")?;
    let library: Library = serde_json::from_slice(
        &std::fs::read(directory.join("library.json")).map_err(|_| "Cannot read saved library")?,
    )
    .map_err(|_| "Invalid library")?;
    let mut voice = library
        .selected_character()
        .and_then(|c| c.voice.clone())
        .unwrap_or(library.voice);
    if voice.adapter != VoiceAdapter::FishHosted || voice.reference_id.is_empty() {
        return Err(
            "Select a hosted Fish voice before narration; no substitution performed".into(),
        );
    }
    voice.delivery = VoiceDelivery::Buffered;
    voice.format = "wav".into();
    voice.enabled = true;
    voice.latency = "balanced".into();
    let plan: Value = serde_json::from_slice(
        &std::fs::read(arg("--storyboard")?).map_err(|_| "Cannot read storyboard")?,
    )
    .map_err(|_| "Invalid storyboard")?;
    let chapters = plan["chapters"].as_array().ok_or("Missing chapters")?;
    if chapters.is_empty() || chapters.len() > 12 {
        return Err("Expected 1–12 chapters".into());
    }
    let output = PathBuf::from(arg("--output-dir")?);
    std::fs::create_dir_all(&output).map_err(|_| "Cannot create output directory")?;
    if std::fs::read_dir(&output)
        .map_err(|_| "Cannot read output directory")?
        .next()
        .is_some()
    {
        return Err("Narration output directory must be empty".into());
    }
    // Validate the complete plan before the first paid call.
    let clips: Vec<_> = chapters
        .iter()
        .map(|chapter| {
            let id = chapter["id"].as_str().ok_or("Missing chapter ID")?;
            let text = chapter["narration"].as_str().ok_or("Missing narration")?;
            if id.is_empty()
                || id.len() > 32
                || !id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                || text.trim().is_empty()
                || text.chars().count() > 1800
            {
                return Err("Invalid narration chapter");
            }
            Ok((id.to_owned(), text.to_owned()))
        })
        .collect::<Result<_, _>>()?;
    let worker = VoiceWorker::new();
    for (index, (id, text)) in clips.into_iter().enumerate() {
        worker.start(index as u64, voice.clone(), text, None)?;
        let start = Instant::now();
        loop {
            if let Some(event) = worker.try_recv() {
                match event {
                    VoiceEvent::Ready { audio, .. } => {
                        std::fs::write(output.join(format!("{id}.wav")), audio.bytes)
                            .map_err(|_| "Cannot save narration")?;
                        println!("Saved {id} narration using {}", voice.model);
                        break;
                    }
                    VoiceEvent::Failed { message, .. } => {
                        return Err(waifu_studio::mcp::redact(&message, None));
                    }
                    _ => return Err("Unexpected streaming narration".into()),
                }
            }
            if start.elapsed() > Duration::from_secs(120) {
                worker.cancel();
                return Err("Narration timed out".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
