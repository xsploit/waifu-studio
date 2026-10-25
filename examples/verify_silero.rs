//! Offline CPU inference check using the same model/runtime loader as the microphone.
use rodio::Source;
use waifu_studio::silero::SileroVad;

fn main() -> Result<(), String> {
    let mut vad = SileroVad::load("", "")?;
    let silence = vad.probability(&[0; 512])?;
    if silence >= 0.1 {
        return Err("Silence was classified as speech".into());
    }
    vad.reset();
    let repeated = vad.probability(&[0; 512])?;
    if (silence - repeated).abs() > 0.00001 {
        return Err("Reset did not restore recurrent state".into());
    }
    let mut speech_frames = 0;
    let mut frames = 0;
    if let Some(path) = std::env::args().nth(1) {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::try_from(file).map_err(|e| e.to_string())?;
        if decoder.sample_rate().get() != 16_000 || decoder.channels().get() != 1 {
            return Err("Supply a 16 kHz mono WAV fixture".into());
        }
        vad.reset();
        let mut frame = [0; 512];
        let mut fill = 0;
        for sample in decoder {
            frame[fill] = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            fill += 1;
            if fill == frame.len() {
                speech_frames += usize::from(vad.probability(&frame)? >= 0.5);
                frames += 1;
                fill = 0;
            }
        }
        if speech_frames < 3 {
            return Err("Speech fixture did not activate the detector".into());
        }
    }
    println!(
        "{}",
        serde_json::json!({"silero_cpu": "ok", "silence_probability": silence, "frames": frames, "speech_frames": speech_frames})
    );
    Ok(())
}
