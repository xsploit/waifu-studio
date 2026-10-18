//! Optional CPU Silero ONNX VAD. Contract: upstream silero_vad/utils_vad.py
//! OnnxWrapper (16 kHz, 512 samples, 64 context, recurrent [2,1,128]).
//! The user supplies both model and native ONNX Runtime; nothing is downloaded.
use ort::{environment::Environment, session::Session, value::Tensor};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

// ort 2 rc14 permits only one environment per process, even after it is dropped.
// Keep its strong handle for the app lifetime and serialize initialization.
struct RuntimeCache {
    loaded_path: Option<PathBuf>,
    environment: Option<Environment>,
}
static RUNTIME: Mutex<RuntimeCache> = Mutex::new(RuntimeCache {
    loaded_path: None,
    environment: None,
});
fn environment(runtime: &str) -> Result<Environment, String> {
    let path = std::fs::canonicalize(runtime).map_err(|_| "Cannot resolve ONNX Runtime library")?;
    let mut cache = RUNTIME
        .lock()
        .map_err(|_| "ONNX Runtime initialization lock failed")?;
    if cache
        .loaded_path
        .as_ref()
        .is_some_and(|loaded| loaded != &path)
    {
        return Err("Restart the app to change the ONNX Runtime library".into());
    }
    if let Some(environment) = &cache.environment {
        return Ok(environment.clone());
    }
    let builder = ort::init_from(&path).map_err(|_| "Cannot load ONNX Runtime library")?;
    // The native library is process-global even if environment creation fails.
    // Allow a retry with this library, but never silently accept another path.
    cache.loaded_path = Some(path);
    let environment = builder
        .with_telemetry(false)
        .build()
        .map_err(|_| "Cannot initialize ONNX Runtime")?;
    cache.environment = Some(environment.clone());
    Ok(environment)
}
pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    context: [f32; 64],
}
impl SileroVad {
    /// Start an independent utterance without rebuilding the ONNX session.
    pub fn reset(&mut self) {
        self.state.fill(0.);
        self.context.fill(0.);
    }

    pub fn load(model: &str, runtime: &str) -> Result<Self, String> {
        if !Path::new(model).is_file() || !Path::new(runtime).is_file() {
            return Err("Select existing Silero ONNX model and ONNX Runtime library files".into());
        }
        if std::fs::metadata(model)
            .map_err(|_| "Cannot inspect Silero model")?
            .len()
            > 64 * 1024 * 1024
        {
            return Err("Silero model exceeds 64 MiB limit".into());
        }
        let env = environment(runtime)?;
        let session = Session::builder(&env)
            .map_err(|_| "Cannot create Silero session")?
            .with_intra_threads(1)
            .map_err(|_| "Cannot configure Silero threads")?
            .with_inter_threads(1)
            .map_err(|_| "Cannot configure Silero threads")?
            .commit_from_file(model)
            .map_err(|_| "Cannot load Silero ONNX model")?;
        Ok(Self {
            session,
            state: vec![0.; 256],
            context: [0.; 64],
        })
    }
    /// Called on the recording worker, never in the device callback.
    pub fn probability(&mut self, samples: &[i16; 512]) -> Result<f32, String> {
        let mut input = Vec::with_capacity(576);
        input.extend_from_slice(&self.context);
        input.extend(samples.iter().map(|s| *s as f32 / 32768.));
        let audio = Tensor::from_array(([1usize, 576], input.clone().into_boxed_slice()))
            .map_err(|_| "Cannot prepare Silero input")?;
        let state = Tensor::from_array(([2usize, 1, 128], self.state.clone().into_boxed_slice()))
            .map_err(|_| "Cannot prepare Silero state")?;
        let rate = Tensor::from_array((Vec::<usize>::new(), vec![16000_i64].into_boxed_slice()))
            .map_err(|_| "Cannot prepare Silero rate")?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => audio, "state" => state, "sr" => rate])
            .map_err(|_| "Silero inference failed; check model version")?;
        let output = outputs
            .get("output")
            .ok_or("Silero model has no output tensor")?;
        let (_, probability) = output
            .try_extract_tensor::<f32>()
            .map_err(|_| "Invalid Silero probability tensor")?;
        let p = *probability.first().ok_or("Empty Silero probability")?;
        let next = outputs
            .get("stateN")
            .ok_or("Silero model has no stateN tensor")?;
        let (_, next) = next
            .try_extract_tensor::<f32>()
            .map_err(|_| "Invalid Silero recurrent tensor")?;
        if next.len() != 256
            || !p.is_finite()
            || !(0. ..=1.).contains(&p)
            || next.iter().any(|x| !x.is_finite())
        {
            return Err("Invalid Silero model output".into());
        }
        self.state.copy_from_slice(next);
        self.context.copy_from_slice(&input[512..]);
        Ok(p)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_files_fail_without_loading_native_code() {
        assert!(
            SileroVad::load("/nonexistent/silero.onnx", "/nonexistent/libonnxruntime.so").is_err()
        );
    }
}
