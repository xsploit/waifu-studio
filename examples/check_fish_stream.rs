//! Explicit, one-utterance protocol check. Never plays audio or logs credentials.
use bevy::audio::Decodable;
use std::time::{Duration, Instant};
use waifu_studio::{
    credentials,
    data::Library,
    fish_live::LiveEvent,
    fish_ws::FishWsState,
    voice::{VoiceAdapter, VoiceDelivery},
};

fn main() -> Result<(), String> {
    if !std::env::args().any(|arg| arg == "--run") {
        println!(
            "Use --run for one short Fish synthesis request using the saved voice; no playback."
        );
        return Ok(());
    }
    // Provider checks must retain the user's configured proxy route.
    if !["https_proxy", "HTTPS_PROXY", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))
    {
        return Err("No proxy route configured; live check skipped".into());
    }
    let directory = directories::ProjectDirs::from("", "", "waifu-studio")
        .ok_or("No application data directory")?;
    // SAFETY: performed before any application workers start.
    unsafe { credentials::load_startup(&directory.data_local_dir().join("credentials.env")) }
        .map_err(|_| "Could not load private local credentials")?;
    let bytes = std::fs::read(directory.data_local_dir().join("library.json"))
        .map_err(|_| "Could not read saved library")?;
    let library: Library = serde_json::from_slice(&bytes).map_err(|_| "Invalid saved library")?;
    let config = library
        .selected_character()
        .and_then(|character| character.voice.as_ref())
        .unwrap_or(&library.voice);
    if config.adapter != VoiceAdapter::FishHosted
        || !matches!(
            config.delivery,
            VoiceDelivery::LiveWebSocket | VoiceDelivery::TimestampWebSocket
        )
        || config.endpoint != "https://api.fish.audio/v1/tts"
    {
        return Err("This check requires the saved hosted Fish WebSocket configuration".into());
    }
    let mut state = FishWsState::default();
    state.timestamps = config.delivery == VoiceDelivery::TimestampWebSocket;
    state.open(1, config, None)?;
    state.push_text("Hello.")?;
    state.finish()?;
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut decoder = None;
    let mut nonzero = 0usize;
    let mut finished = None;
    while Instant::now() < deadline {
        for event in state.poll() {
            match event {
                LiveEvent::Playing { audio, .. } => decoder = Some(audio.decoder()),
                LiveEvent::Finished { stats, .. } => finished = Some(stats),
                LiveEvent::Failed { message, .. } => return Err(message),
                _ => {}
            }
        }
        if let Some(source) = decoder.as_mut() {
            for _ in 0..4096 {
                match source.next() {
                    Some(sample) => nonzero += usize::from(sample != 0.0),
                    None if finished.is_some() => {
                        let stats = finished.as_ref().unwrap();
                        if nonzero == 0 {
                            return Err("Fish stream decoded no nonzero audio samples".into());
                        }
                        println!(
                            "Fish WebSocket PASS: {} bytes, {} frames, first audio {:?} ms, nonzero decoded samples {}; no playback",
                            stats.audio_bytes, stats.frames, stats.first_audio_ms, nonzero
                        );
                        return Ok(());
                    }
                    None => break,
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    state.cancel();
    Err("Fish WebSocket check exceeded 40 seconds; no retry".into())
}
