//! Tiny live checks. One capped Vercel generation and one read-only Fish GET.
//! Run: cargo run --example check_connections -- [application-data-directory]
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use waifu_studio::{
    credentials,
    data::Turn,
    provider::{self, GatewayRoute, ProviderConfig, ProviderEvent},
};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let directory = match args.next() {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!(
                "check_connections [application-data-directory]\nMakes one Vercel gpt-5-nano request (128 output tokens maximum), then one read-only Fish model-list GET. Reads private credentials.env; existing environment wins. No keys or remote error bodies are printed."
            );
            return;
        }
        Some(path) => PathBuf::from(path),
        None => match directories::ProjectDirs::from("", "", "waifu-studio") {
            Some(project) => project.data_local_dir().to_path_buf(),
            None => {
                eprintln!("CHECK FAILED: cannot locate application data directory");
                std::process::exit(1);
            }
        },
    };
    if args.next().is_some() {
        eprintln!("Usage: check_connections [application-data-directory]");
        std::process::exit(1);
    }
    // SAFETY: standalone startup before creating HTTP clients or provider workers.
    if let Err(error) = unsafe { credentials::load_startup(&directory.join("credentials.env")) } {
        eprintln!("CHECK FAILED: {error}");
        std::process::exit(1);
    }
    let vercel = check_vercel();
    let fish = check_fish();
    if !vercel || !fish {
        std::process::exit(1);
    }
}

fn check_vercel() -> bool {
    if std::env::var("AI_GATEWAY_API_KEY")
        .ok()
        .is_none_or(|value| value.trim().is_empty())
    {
        eprintln!("Vercel FAILED: AI_GATEWAY_API_KEY is missing");
        return false;
    }
    let mut config = ProviderConfig::pinned(GatewayRoute::VercelGateway);
    config.model = "openai/gpt-5-nano".into();
    config.max_tokens = Some(128);
    // This check uses the gateway credential only; do not forward optional BYOK.
    config.byok_openai_key_env.clear();
    let (tx, rx) = mpsc::channel();
    let job = provider::spawn_chat(
        config,
        vec![Turn {
            external: None,
            role: "user".into(),
            content: "Reply with exactly OK".into(),
        }],
        tx,
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut answer = String::new();
    let mut deltas = 0usize;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            job.cancel();
            eprintln!("Vercel FAILED: 60-second check deadline");
            return false;
        };
        match rx.recv_timeout(remaining) {
            Ok(ProviderEvent::Delta(id, delta)) if id == job.id => {
                if answer.len().saturating_add(delta.len()) > 64 {
                    job.cancel();
                    eprintln!("Vercel FAILED: output exceeded smoke-test bound");
                    return false;
                }
                answer.push_str(&delta);
                deltas += 1;
            }
            Ok(ProviderEvent::Finished(id)) if id == job.id => {
                if deltas > 0 && answer.trim() == "OK" {
                    println!(
                        "Vercel PASS: native provider streamed exact OK ({deltas} delta events; 128-token output cap; gpt-5-nano)"
                    );
                    return true;
                }
                eprintln!("Vercel FAILED: completed reply did not match exact OK");
                return false;
            }
            Ok(ProviderEvent::Failed(id, _)) if id == job.id => {
                // Intentionally discard provider text, URLs and remote bodies.
                eprintln!("Vercel FAILED: provider request failed (details withheld)");
                return false;
            }
            Ok(_) => (),
            Err(_) => {
                job.cancel();
                eprintln!("Vercel FAILED: stream unavailable or timed out");
                return false;
            }
        }
    }
}

fn check_fish() -> bool {
    let key = match std::env::var("FISH_API_KEY") {
        Ok(key) if !key.trim().is_empty() => key,
        _ => {
            eprintln!("Fish FAILED: FISH_API_KEY is missing");
            return false;
        }
    };
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            eprintln!("Fish FAILED: unable to initialize HTTP client");
            return false;
        }
    };
    match client
        .get("https://api.fish.audio/model")
        .query(&[("self", "true"), ("page_size", "1"), ("page_number", "1")])
        .bearer_auth(key)
        .send()
    {
        Ok(response) if response.status().is_success() => {
            // A success status proves this read-only authenticated route accepted
            // the request. Do not read, retain, or print private model metadata.
            println!(
                "Fish PASS: authenticated read-only model-list GET (HTTP {})",
                response.status().as_u16()
            );
            true
        }
        Ok(response) => {
            eprintln!(
                "Fish FAILED: model-list HTTP {} (body withheld)",
                response.status().as_u16()
            );
            false
        }
        Err(_) => {
            eprintln!("Fish FAILED: request could not complete (details withheld)");
            false
        }
    }
}
