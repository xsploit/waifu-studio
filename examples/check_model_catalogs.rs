//! Read-only Vercel catalog check; --speak-vercel adds one five-character TTS request.
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use waifu_studio::{
    credentials,
    model_catalog::{CatalogKind, ModelCatalog},
    provider::{self, GatewayRoute, ProviderConfig, ProviderEvent},
    voice::{VoiceAdapter, VoiceConfig},
};

fn main() -> Result<(), String> {
    let speak = std::env::args().skip(1).any(|arg| arg == "--speak-vercel");
    let dir = directories::ProjectDirs::from("", "", "waifu-studio").ok_or("No data directory")?;
    // SAFETY: startup only, before creating any threads or clients.
    unsafe { credentials::load_startup(&dir.data_local_dir().join("credentials.env")) }
        .map_err(|_| "Private credentials could not be loaded")?;
    let (tx, rx) = mpsc::channel();
    provider::fetch_models(
        0,
        1,
        ProviderConfig::pinned(GatewayRoute::VercelGateway),
        tx,
    );
    match rx.recv_timeout(Duration::from_secs(35)) {
        Ok(ProviderEvent::Models(_, _, Ok(models))) if !models.is_empty() => {
            println!("Vercel LLM catalog PASS: {} chat models", models.len())
        }
        _ => return Err("Vercel chat catalog check failed (remote details withheld)".into()),
    }
    for (kind, endpoint) in [
        (CatalogKind::Speech, waifu_studio::vercel_speech::ENDPOINT),
        (
            CatalogKind::Transcription,
            "https://ai-gateway.vercel.sh/v4/ai/transcription-model",
        ),
    ] {
        let mut catalog = ModelCatalog::default();
        catalog.sync(endpoint, "AI_GATEWAY_API_KEY", "", kind);
        let deadline = Instant::now() + Duration::from_secs(25);
        while catalog.loading && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            catalog.poll();
        }
        if catalog.loading || catalog.models.is_empty() {
            return Err(format!("Vercel {kind:?} catalog check failed"));
        }
        println!(
            "Vercel {kind:?} catalog PASS: {} models",
            catalog.models.len()
        );
    }
    let mut fish = ModelCatalog::default();
    fish.sync(
        "https://api.fish.audio/v1/tts",
        "FISH_API_KEY",
        "",
        CatalogKind::Speech,
    );
    assert_eq!(fish.models.len(), 5);
    println!("Fish model picker PASS: all five documented hosted models");
    if speak {
        let config = VoiceConfig::preset(VoiceAdapter::Vercel);
        let audio = waifu_studio::vercel_speech::synthesize(&config, "Test.", None, || false)?;
        let decoder = rodio::Decoder::try_from(std::io::Cursor::new(audio.bytes.clone()))
            .map_err(|_| "Vercel speech returned audio the native player cannot decode")?;
        let samples = decoder.count();
        if samples == 0 {
            return Err("Vercel speech decoded no samples".into());
        }
        println!(
            "Vercel speech PASS: openai/tts-1, {} bytes {}, {samples} decoded samples; no playback",
            audio.bytes.len(),
            audio.format
        );
    }
    Ok(())
}
