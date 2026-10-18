//! Offline runtime/model check. Does not open a microphone or contact a provider.
//! cargo run --example check_silero -- MODEL.onnx libonnxruntime.so
use waifu_studio::silero::SileroVad;

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: check_silero MODEL.onnx libonnxruntime.so".into());
    }
    // Reopen a session to exercise the repeated-recording initialization path.
    for recording in 0..2 {
        let mut vad = SileroVad::load(&args[0], &args[1])?;
        let mut maximum = 0.0_f32;
        for _ in 0..32 {
            maximum = maximum.max(vad.probability(&[0; 512])?);
        }
        if maximum >= 0.5 {
            return Err(format!(
                "Silence unexpectedly classified as speech: {maximum}"
            ));
        }
        println!(
            "Session {}: 32 silence windows, maximum speech probability {maximum:.6}",
            recording + 1
        );
    }
    Ok(())
}
