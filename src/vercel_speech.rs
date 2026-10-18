//! Buffered speech through Vercel AI Gateway's native v4 protocol.
//!
//! Wire contract checked against the official AI SDK on 2026-10-18:
//! https://github.com/vercel/ai/blob/8c659885c52ef519b21b3af1ff266d843e3c157f/packages/gateway/src/gateway-speech-model.ts
//! The gateway returns JSON containing base64 audio, rather than raw audio.
//! Voice identifiers and supported formats vary between models; the gateway
//! reports unsupported settings instead of this adapter guessing a replacement.
use crate::voice::{VoiceAudio, VoiceConfig, VoiceDelivery};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use std::{io::Read, time::Duration};

pub const ENDPOINT: &str = "https://ai-gateway.vercel.sh/v4/ai/speech-model";
const MAX_AUDIO: usize = 32 * 1024 * 1024;
const MAX_RESPONSE: usize = MAX_AUDIO.div_ceil(3) * 4 + 64 * 1024;

/// Runs on the existing speech worker, with the same cancellation generation.
/// Only buffered WAV and MP3 playback are supported by this transport.
pub fn synthesize(
    config: &VoiceConfig,
    text: &str,
    session_key: Option<&str>,
    cancelled: impl Fn() -> bool,
) -> Result<VoiceAudio, String> {
    let url = validate(config, text)?;
    if cancelled() {
        return Err("Speech cancelled".into());
    }
    let configured_key = (!config.api_key_env.trim().is_empty())
        .then(|| std::env::var(config.api_key_env.trim()).ok())
        .flatten();
    let default_key = std::env::var("AI_GATEWAY_API_KEY").ok();
    let oidc = std::env::var("VERCEL_OIDC_TOKEN").ok();
    let api_key = session_key
        .filter(|s| !s.trim().is_empty())
        .or(configured_key.as_deref().filter(|s| !s.trim().is_empty()))
        .or(default_key.as_deref().filter(|s| !s.trim().is_empty()));
    let (key, method) = match api_key {
        Some(key) => (key, "api-key"),
        None => (
            oidc.as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or("Set a Vercel session key or AI_GATEWAY_API_KEY")?,
            "oidc",
        ),
    };
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize Vercel speech client")?;
    let mut response = client
        .post(url)
        .bearer_auth(key)
        .header("ai-gateway-auth-method", method)
        .header("ai-gateway-protocol-version", "0.0.1")
        .header("ai-speech-model-specification-version", "4")
        .header("ai-model-id", config.model.trim())
        .json(&request_body(config, text))
        .send()
        .map_err(|_| "Vercel speech request failed (connection or timeout)")?;
    if cancelled() {
        return Err("Speech cancelled".into());
    }
    // Never include response bodies, endpoints, or credentials in errors.
    if !response.status().is_success() {
        return Err(format!(
            "Vercel speech returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE as u64)
    {
        return Err("Vercel speech response exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        if cancelled() {
            return Err("Speech cancelled".into());
        }
        let size = response
            .read(&mut chunk)
            .map_err(|_| "Could not receive Vercel speech response")?;
        if size == 0 {
            break;
        }
        if bytes.len().saturating_add(size) > MAX_RESPONSE {
            return Err("Vercel speech response exceeds its size limit".into());
        }
        bytes.extend_from_slice(&chunk[..size]);
    }
    if cancelled() {
        return Err("Speech cancelled".into());
    }
    decode_response(&bytes)
}

fn validate(config: &VoiceConfig, text: &str) -> Result<reqwest::Url, String> {
    if text.trim().is_empty() || text.chars().count() > 12_000 {
        return Err("Speech text must contain 1–12000 characters".into());
    }
    if config.delivery != VoiceDelivery::Buffered {
        return Err("Vercel speech currently requires buffered delivery".into());
    }
    if !config.speed.is_finite() || !(0.5..=2.0).contains(&config.speed) {
        return Err("Speech speed must be between 0.5 and 2".into());
    }
    if !matches!(config.format.as_str(), "wav" | "mp3") {
        return Err("Playback currently supports WAV or MP3 speech".into());
    }
    let model = config.model.trim();
    if model.len() > 512
        || model.chars().any(char::is_whitespace)
        || model.chars().any(char::is_control)
        || !model
            .split_once('/')
            .is_some_and(|(provider, name)| !provider.is_empty() && !name.is_empty())
    {
        return Err("Select a Vercel speech model with a provider/model identifier".into());
    }
    if config.voice.chars().count() > 2048 || config.voice.chars().any(char::is_control) {
        return Err("Speech voice identifier is invalid or too long".into());
    }
    if config.direction.chars().count() > 2000 {
        return Err("Voice instructions must be at most 2000 characters".into());
    }
    let url =
        reqwest::Url::parse(&config.endpoint).map_err(|_| "Invalid Vercel speech endpoint")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use a speech endpoint without credentials, query, or fragment".into());
    }
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("Speech endpoint requires HTTPS, or HTTP on localhost".into());
    }
    if !url.path().ends_with("/speech-model") {
        return Err("Vercel speech endpoint must end in /speech-model".into());
    }
    Ok(url)
}

fn request_body(config: &VoiceConfig, text: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "text": text,
        "outputFormat": config.format,
        "speed": config.speed,
    });
    if !config.voice.trim().is_empty() {
        body["voice"] = config.voice.trim().into();
    }
    if !config.direction.trim().is_empty() {
        body["instructions"] = config.direction.trim().into();
    }
    body
}

#[derive(Deserialize)]
struct WireResponse {
    audio: String,
}

fn decode_response(bytes: &[u8]) -> Result<VoiceAudio, String> {
    if bytes.len() > MAX_RESPONSE {
        return Err("Vercel speech response exceeds its size limit".into());
    }
    let response: WireResponse =
        serde_json::from_slice(bytes).map_err(|_| "Invalid Vercel speech JSON response")?;
    if response.audio.len() > MAX_AUDIO.div_ceil(3) * 4 {
        return Err("Speech audio exceeds 32 MiB".into());
    }
    let bytes = STANDARD
        .decode(response.audio)
        .map_err(|_| "Invalid base64 audio in Vercel speech response")?;
    if bytes.is_empty() || bytes.len() > MAX_AUDIO {
        return Err("Speech audio is empty or exceeds 32 MiB".into());
    }
    // The gateway schema has no MIME field. Identify its actual returned bytes
    // so a provider returning a different format cannot reach the wrong decoder.
    let (format, mime) = if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        ("wav", "audio/wav")
    } else if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        ("mp3", "audio/mpeg")
    } else {
        return Err("Vercel speech returned unsupported audio; choose WAV or MP3".into());
    };
    Ok(VoiceAudio {
        bytes,
        mime: mime.into(),
        format: format.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener};

    fn server(status: u16, payload: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/v4/ai/speech-model",
            listener.local_addr().unwrap()
        );
        let payload = payload.to_owned();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                assert!(request.len() < 1024 * 1024);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let length = String::from_utf8_lossy(&request[..end])
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len()).unwrap();
            String::from_utf8(request).unwrap()
        });
        (endpoint, handle)
    }

    #[test]
    fn native_v4_request_and_base64_response() {
        let audio = b"RIFF\x04\0\0\0WAVE";
        let payload = serde_json::json!({"audio":STANDARD.encode(audio),"warnings":[]}).to_string();
        let (endpoint, server) = server(200, &payload);
        let config = VoiceConfig {
            endpoint,
            model: "openai/tts-1".into(),
            voice: "alloy".into(),
            direction: "Speak softly".into(),
            speed: 1.25,
            ..VoiceConfig::preset(crate::voice::VoiceAdapter::Vercel)
        };
        let result = synthesize(&config, "Hello!", Some("test-key"), || false).unwrap();
        assert_eq!(result.bytes, audio);
        assert_eq!(result.mime, "audio/wav");
        assert_eq!(result.format, "wav");
        let request = server.join().unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        let headers = headers.to_ascii_lowercase();
        assert!(headers.starts_with("post /v4/ai/speech-model http/1.1"));
        assert!(headers.contains("ai-model-id: openai/tts-1"));
        assert!(headers.contains("ai-speech-model-specification-version: 4"));
        assert!(headers.contains("ai-gateway-protocol-version: 0.0.1"));
        assert!(headers.contains("ai-gateway-auth-method: api-key"));
        assert!(headers.contains("authorization: bearer test-key"));
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"text":"Hello!","voice":"alloy","outputFormat":"wav","instructions":"Speak softly","speed":1.25})
        );
    }

    #[test]
    fn provider_errors_expose_only_status() {
        let (endpoint, server) = server(401, "secret from provider body");
        let config = VoiceConfig {
            endpoint,
            model: "openai/tts-1".into(),
            ..VoiceConfig::preset(crate::voice::VoiceAdapter::Vercel)
        };
        let error = synthesize(&config, "Hi", Some("test-key"), || false).unwrap_err();
        assert_eq!(error, "Vercel speech returned HTTP 401");
        server.join().unwrap();
    }

    #[test]
    fn base64_and_actual_audio_format_are_checked() {
        let audio = decode_response(br#"{"audio":"SUQzYWJj"}"#).unwrap();
        assert_eq!(audio.format, "mp3");
        assert_eq!(audio.mime, "audio/mpeg");
        assert!(decode_response(br#"{"audio":"invalid"}"#).is_err());
        assert!(decode_response(br#"{"audio":""}"#).is_err());
        assert!(decode_response(br#"{"audio":"aGVsbG8="}"#).is_err());
        assert!(decode_response(br#"{"text":"no audio"}"#).is_err());
    }

    #[test]
    fn unsafe_endpoints_and_incompatible_delivery_fail_before_network() {
        let mut config = VoiceConfig {
            endpoint: ENDPOINT.into(),
            model: "openai/tts-1".into(),
            ..VoiceConfig::preset(crate::voice::VoiceAdapter::Vercel)
        };
        assert!(validate(&config, "Hi").is_ok());
        for endpoint in [
            "http://example.com/v4/ai/speech-model",
            "https://user:pass@example.com/speech-model",
            "https://example.com/speech-model?key=x",
            "https://example.com/speech-model#fragment",
            "https://example.com/chat/completions",
        ] {
            config.endpoint = endpoint.into();
            assert!(validate(&config, "Hi").is_err());
        }
        config.endpoint = ENDPOINT.into();
        config.delivery = VoiceDelivery::HttpWavStream;
        assert!(validate(&config, "Hi").is_err());
        config.delivery = VoiceDelivery::Buffered;
        config.model = "tts-1".into();
        assert!(validate(&config, "Hi").is_err());
    }

    #[test]
    fn cancelled_request_never_connects() {
        let config = VoiceConfig {
            endpoint: ENDPOINT.into(),
            model: "openai/tts-1".into(),
            ..VoiceConfig::preset(crate::voice::VoiceAdapter::Vercel)
        };
        assert_eq!(
            synthesize(&config, "Hi", Some("test-key"), || true).unwrap_err(),
            "Speech cancelled"
        );
    }
}
