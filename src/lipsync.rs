//! MFCC vowel lip-sync analysis, ported from uLipSync / wLipSync.
//!
//! Rust implementation of the uLipSync pipeline (hecomi, MIT), using
//! wLipSync's radix-2 FFT structure (Noeri Huisman, MIT): ring buffer, RMS
//! volume, windowed-sinc low-pass, decimation, pre-emphasis, Hamming window,
//! peak normalisation, radix-2 FFT magnitude, mel filter bank, dB, DCT,
//! then L1 / L2 / cosine scoring against a calibrated profile. See
//! `LIPSYNC-NOTICES.txt` for the upstream copyright and licence notices.
//!
//! Uses native f32 math and round-half-even, like the calibration pipeline.
//! Tests compare 30 vectors against pinned upstream C kernels using native
//! libm and documented portability/bounds corrections. This does not establish
//! bit parity with Unity Burst or wLipSync's approximate-math WASM build.
//!
//! The classifier only distinguishes the phonemes a profile was calibrated
//! with; the output is five vowel weights (A I U E O) plus amplitude. It is
//! not a phoneme transcriber.
//!
//! Threading: `LipSyncAnalyzer` allocates everything up front and never
//! allocates while pushing audio, but one analysis frame costs a few hundred
//! thousand multiply-adds. Run it on an analysis worker fed from the audio
//! callback (for example through a channel), never inside the callback.

use std::sync::Arc;

use serde::Deserialize;
use thiserror::Error;

/// Number of MFCC coefficients. Both upstreams hard-code 12.
pub const MFCC_NUM: usize = 12;
/// Upper bound on phoneme entries (wLipSync `MAX_PHONEMES`).
pub const MAX_PHONEMES: usize = 128;
/// Upper bound on calibration arrays across a profile (wLipSync `MAX_MFCC_SAMPLES`).
pub const MAX_CALIBRATION_SAMPLES: usize = MAX_PHONEMES * 16;
/// Largest profile file accepted by [`LipSyncProfile::from_bytes`].
pub const MAX_PROFILE_BYTES: usize = 8 * 1024 * 1024;
/// Supported input sample-rate range.
pub const MIN_INPUT_SAMPLE_RATE: u32 = 4_000;
pub const MAX_INPUT_SAMPLE_RATE: u32 = 384_000;

const PRE_EMPHASIS: f32 = 0.97;
const LOW_PASS_RANGE_HZ: f32 = 500.0;
const EPSILON: f32 = f32::from_bits(1); // Unity.Mathematics math.EPSILON
const MAX_INPUT_WINDOW: usize = 65_536;
const MAX_FILTER_PRODUCTS: usize = 4_000_000;
const MAX_COEFFICIENT: f32 = 1_000_000.0;
const MIN_HOP_SECONDS: f32 = 128.0 / 48_000.0;
const BINARY_SIGNATURE: &[u8; 4] = b"WLIP";

#[derive(Debug, Error, PartialEq)]
pub enum LipSyncError {
    #[error("lip-sync profile is invalid: {0}")]
    InvalidProfile(String),
    #[error("input sample rate {rate} Hz is unsupported: {reason}")]
    UnsupportedSampleRate { rate: u32, reason: String },
    #[error("lip-sync settings are invalid: {0}")]
    InvalidSettings(String),
    #[error("audio sample {index} is not finite; the chunk was rejected")]
    NonFiniteSample { index: usize },
    #[error("audio sample {index} is outside normalized PCM range -1..=1; the chunk was rejected")]
    SampleOutOfRange { index: usize },
    #[error(
        "interleaved audio needs 1..=32 channels and whole frames (got {channels} channels, {len} samples)"
    )]
    InvalidChannels { channels: usize, len: usize },
}

/// The five output vowels, in output order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vowel {
    A = 0,
    I = 1,
    U = 2,
    E = 3,
    O = 4,
}

impl Vowel {
    pub const ALL: [Vowel; 5] = [Vowel::A, Vowel::I, Vowel::U, Vowel::E, Vowel::O];

    /// Maps a profile phoneme name to a vowel ("A", "i", " O " ...). Other
    /// names (uLipSync's "S" silence class, "N", "-") still take part in
    /// scoring but feed no vowel.
    pub fn from_phoneme_name(name: &str) -> Option<Vowel> {
        match name.trim() {
            n if n.eq_ignore_ascii_case("a") => Some(Vowel::A),
            n if n.eq_ignore_ascii_case("i") => Some(Vowel::I),
            n if n.eq_ignore_ascii_case("u") => Some(Vowel::U),
            n if n.eq_ignore_ascii_case("e") => Some(Vowel::E),
            n if n.eq_ignore_ascii_case("o") => Some(Vowel::O),
            _ => None,
        }
    }
}

/// uLipSync `CompareMethod`; the integer values are the profile encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareMethod {
    L1Norm = 0,
    L2Norm = 1,
    CosineSimilarity = 2,
}

impl CompareMethod {
    fn from_code(code: i64) -> Result<Self, LipSyncError> {
        match code {
            0 => Ok(Self::L1Norm),
            1 => Ok(Self::L2Norm),
            2 => Ok(Self::CosineSimilarity),
            other => Err(invalid(format!("compareMethod {other} is not 0, 1 or 2"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfilePhoneme {
    pub name: String,
    pub vowel: Option<Vowel>,
    /// Average MFCC of the calibration data (uLipSync `mfccNativeArray`).
    pub mfcc: [f32; MFCC_NUM],
}

/// A validated uLipSync / wLipSync calibration profile.
#[derive(Debug, Clone, PartialEq)]
pub struct LipSyncProfile {
    pub target_sample_rate: u32,
    pub sample_count: usize,
    pub mel_filter_bank_channels: usize,
    pub compare_method: CompareMethod,
    pub use_standardization: bool,
    pub phonemes: Vec<ProfilePhoneme>,
    pub means: [f32; MFCC_NUM],
    pub standard_deviations: [f32; MFCC_NUM],
}

fn invalid(reason: impl Into<String>) -> LipSyncError {
    LipSyncError::InvalidProfile(reason.into())
}

// uLipSync `Profile` defaults apply to omitted fields, like JsonUtility.FromJsonOverwrite.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonProfile {
    #[serde(default = "default_mfcc_num")]
    mfcc_num: i64,
    #[serde(default = "default_mfcc_data_count")]
    mfcc_data_count: i64,
    #[serde(default = "default_mel_channels")]
    mel_filter_bank_channels: i64,
    #[serde(default = "default_target_rate")]
    target_sample_rate: i64,
    #[serde(default = "default_sample_count")]
    sample_count: i64,
    #[serde(default)]
    use_standardization: bool,
    #[serde(default = "default_compare_method")]
    compare_method: i64,
    mfccs: Vec<JsonMfcc>,
    // Present only in wLipSync profiles converted from the binary format.
    means: Option<Vec<f32>>,
    std_devs: Option<Vec<f32>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonMfcc {
    name: String,
    #[serde(default)]
    mfcc_calibration_data_list: Vec<JsonCalibration>,
    values: Option<Vec<f32>>,
}

#[derive(Deserialize)]
struct JsonCalibration {
    array: Vec<f32>,
}

fn default_mfcc_num() -> i64 {
    12
}
fn default_mfcc_data_count() -> i64 {
    16
}
fn default_mel_channels() -> i64 {
    30
}
fn default_target_rate() -> i64 {
    16_000
}
fn default_sample_count() -> i64 {
    1024
}
fn default_compare_method() -> i64 {
    1
}

impl LipSyncProfile {
    /// Loads a uLipSync / wLipSync JSON profile, or a wLipSync binary
    /// profile (`WLIP` signature, produced by `tools/json2bin.js`).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LipSyncError> {
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(invalid(format!(
                "profile is {} bytes, limit is {MAX_PROFILE_BYTES}",
                bytes.len()
            )));
        }
        if bytes.starts_with(BINARY_SIGNATURE) {
            Self::from_wlipsync_binary(bytes)
        } else {
            Self::from_json(bytes)
        }
    }

    fn from_json(bytes: &[u8]) -> Result<Self, LipSyncError> {
        let json: JsonProfile =
            serde_json::from_slice(bytes).map_err(|e| invalid(format!("JSON: {e}")))?;
        if json.mfcc_num != MFCC_NUM as i64 {
            return Err(invalid(format!(
                "mfccNum must be {MFCC_NUM}, got {}",
                json.mfcc_num
            )));
        }
        let header = Header::validate(
            json.target_sample_rate,
            json.sample_count,
            json.mel_filter_bank_channels,
            json.compare_method,
            json.mfccs.len(),
        )?;
        let precomputed = json.mfccs.iter().any(|m| m.values.is_some())
            || json.means.is_some()
            || json.std_devs.is_some();
        if precomputed
            && !(json.mfccs.iter().all(|m| m.values.is_some())
                && json.means.is_some()
                && json.std_devs.is_some())
        {
            return Err(invalid(
                "precomputed profiles require values for every phoneme, means and stdDevs",
            ));
        }
        if precomputed {
            let phonemes = json
                .mfccs
                .into_iter()
                .map(|m| {
                    Ok((
                        m.name,
                        coefficients(&m.values.unwrap_or_default(), "values")?,
                    ))
                })
                .collect::<Result<Vec<_>, LipSyncError>>()?;
            let means = coefficients(&json.means.unwrap_or_default(), "means")?;
            let std_devs = coefficients(&json.std_devs.unwrap_or_default(), "stdDevs")?;
            return header.finish(json.use_standardization, phonemes, means, std_devs);
        }

        if !(1..=MAX_CALIBRATION_SAMPLES as i64).contains(&json.mfcc_data_count) {
            return Err(invalid(format!(
                "mfccDataCount {} is outside 1..={MAX_CALIBRATION_SAMPLES}",
                json.mfcc_data_count
            )));
        }
        let keep = json.mfcc_data_count as usize;
        let mut total = 0usize;
        let mut calibration = Vec::with_capacity(json.mfccs.len());
        for mfcc in &json.mfccs {
            let list = &mfcc.mfcc_calibration_data_list;
            if list.is_empty() {
                return Err(invalid(format!(
                    "phoneme '{}' has no calibration data",
                    mfcc.name
                )));
            }
            // Profile.RemoveOldCalibrationData keeps the newest mfccDataCount entries.
            let kept = &list[list.len().saturating_sub(keep)..];
            total += kept.len();
            if total > MAX_CALIBRATION_SAMPLES {
                return Err(invalid(format!(
                    "more than {MAX_CALIBRATION_SAMPLES} calibration arrays"
                )));
            }
            let arrays = kept
                .iter()
                .map(|c| coefficients(&c.array, &format!("calibration data of '{}'", mfcc.name)))
                .collect::<Result<Vec<_>, _>>()?;
            calibration.push((mfcc.name.clone(), arrays));
        }

        // MfccData.UpdateNativeArray
        let phonemes = calibration
            .iter()
            .map(|(name, arrays)| {
                let mut avg = [0f32; MFCC_NUM];
                for (i, slot) in avg.iter_mut().enumerate() {
                    for a in arrays {
                        *slot += a[i];
                    }
                    *slot /= arrays.len() as f32;
                }
                (name.clone(), avg)
            })
            .collect();

        // Profile.UpdateMeans / UpdateStandardizations
        let mut means = [0f32; MFCC_NUM];
        let mut std_devs = [1f32; MFCC_NUM];
        if json.use_standardization {
            let all = || calibration.iter().flat_map(|(_, arrays)| arrays.iter());
            let n = all().count() as f32;
            for a in all() {
                for i in 0..MFCC_NUM {
                    means[i] += a[i];
                }
            }
            means.iter_mut().for_each(|m| *m /= n);
            std_devs = [0f32; MFCC_NUM];
            for a in all() {
                for i in 0..MFCC_NUM {
                    std_devs[i] += (a[i] - means[i]).powf(2.0);
                }
            }
            std_devs.iter_mut().for_each(|s| *s = (*s / n).sqrt());
        }
        header.finish(json.use_standardization, phonemes, means, std_devs)
    }

    fn from_wlipsync_binary(bytes: &[u8]) -> Result<Self, LipSyncError> {
        // Layout from wLipSync tools/json2bin.js / www/parse.ts (big-endian DataView).
        let mut r = Reader {
            bytes,
            offset: BINARY_SIGNATURE.len(),
        };
        let target_sample_rate = r.i32()? as i64;
        let sample_count = r.i32()? as i64;
        let mel = r.u8()? as i64;
        let compare = r.u8()? as i64;
        let phoneme_count = r.u8()? as usize;
        if r.u8()? == 0 {
            return Err(invalid("binary mfccDataCount must be positive"));
        }
        let use_standardization = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid("binary standardization flag must be 0 or 1")),
        };
        let header = Header::validate(
            target_sample_rate,
            sample_count,
            mel,
            compare,
            phoneme_count,
        )?;
        let mut values = Vec::with_capacity(phoneme_count);
        for _ in 0..phoneme_count {
            values.push(r.floats()?);
        }
        let means = r.floats()?;
        let std_devs = r.floats()?;
        let mut phonemes = Vec::with_capacity(phoneme_count);
        for v in values {
            let len = r.u8()? as usize;
            let name = String::from_utf8_lossy(r.take(len)?).into_owned();
            phonemes.push((name, v));
        }
        if r.offset != bytes.len() {
            return Err(invalid(format!(
                "{} trailing bytes after binary profile",
                bytes.len() - r.offset
            )));
        }
        let check = |a: &[f32; MFCC_NUM], what: &str| coefficients(a, what).map(|_| ());
        for (name, v) in &phonemes {
            check(v, &format!("values of '{name}'"))?;
        }
        check(&means, "means")?;
        check(&std_devs, "stdDevs")?;
        header.finish(use_standardization, phonemes, means, std_devs)
    }

    /// Revalidates public fields, including profiles constructed by callers.
    pub fn validate(&self) -> Result<(), LipSyncError> {
        let sample_count =
            i64::try_from(self.sample_count).map_err(|_| invalid("sampleCount overflow"))?;
        let mel = i64::try_from(self.mel_filter_bank_channels)
            .map_err(|_| invalid("melFilterBankChannels overflow"))?;
        Header::validate(
            self.target_sample_rate as i64,
            sample_count,
            mel,
            self.compare_method as i64,
            self.phonemes.len(),
        )?;
        coefficients(&self.means, "means")?;
        coefficients(&self.standard_deviations, "standard deviations")?;
        if self.standard_deviations.iter().any(|s| *s <= 0.0) {
            return Err(invalid("standard deviations must be positive"));
        }
        for p in &self.phonemes {
            if p.name.trim().is_empty() || p.name.len() > 255 || p.name.contains('\0') {
                return Err(invalid("phoneme name must be 1..=255 bytes without NUL"));
            }
            if p.vowel != Vowel::from_phoneme_name(&p.name) {
                return Err(invalid("phoneme vowel mapping does not match its name"));
            }
            coefficients(&p.mfcc, "phoneme MFCC")?;
            if p.mfcc
                .iter()
                .enumerate()
                .any(|(i, x)| !((x - self.means[i]) / self.standard_deviations[i]).is_finite())
            {
                return Err(invalid("standardized phoneme MFCC is not finite"));
            }
        }
        if self.phonemes.iter().all(|p| p.vowel.is_none()) {
            return Err(invalid("no phoneme is named A, I, U, E or O"));
        }
        Ok(())
    }
    /// Which output vowels have at least one calibration phoneme.
    pub fn vowel_coverage(&self) -> [bool; 5] {
        let mut out = [false; 5];
        for p in &self.phonemes {
            if let Some(v) = p.vowel {
                out[v as usize] = true;
            }
        }
        out
    }
}

struct Header {
    target_sample_rate: u32,
    sample_count: usize,
    mel: usize,
    compare: CompareMethod,
}

impl Header {
    fn validate(
        target: i64,
        sample_count: i64,
        mel: i64,
        compare: i64,
        phonemes: usize,
    ) -> Result<Self, LipSyncError> {
        // The low-pass cutoff is target/2 - 500 Hz and must stay positive.
        if !(4_000..=96_000).contains(&target) {
            return Err(invalid(format!(
                "targetSampleRate {target} is outside 4000..=96000"
            )));
        }
        if !(64..=8192).contains(&sample_count) || (sample_count as u64).count_ones() != 1 {
            return Err(invalid(format!(
                "sampleCount {sample_count} must be a power of two in 64..=8192"
            )));
        }
        // The DCT output is read at indices 1..=12.
        if !(MFCC_NUM as i64 + 1..=255).contains(&mel) {
            return Err(invalid(format!(
                "melFilterBankChannels {mel} is outside {}..=255",
                MFCC_NUM + 1
            )));
        }
        if phonemes == 0 || phonemes > MAX_PHONEMES {
            return Err(invalid(format!(
                "{phonemes} phonemes; need 1..={MAX_PHONEMES}"
            )));
        }
        Ok(Self {
            target_sample_rate: target as u32,
            sample_count: sample_count as usize,
            mel: mel as usize,
            compare: CompareMethod::from_code(compare)?,
        })
    }

    fn finish(
        self,
        use_standardization: bool,
        phonemes: Vec<(String, [f32; MFCC_NUM])>,
        means: [f32; MFCC_NUM],
        standard_deviations: [f32; MFCC_NUM],
    ) -> Result<LipSyncProfile, LipSyncError> {
        if let Some(i) = standard_deviations
            .iter()
            .position(|s| !(s.is_finite() && *s > 0.0))
        {
            return Err(invalid(format!(
                "standard deviation {i} is {} (constant calibration data?)",
                standard_deviations[i]
            )));
        }
        if let Some(i) = means.iter().position(|m| !m.is_finite()) {
            return Err(invalid(format!("mean {i} is not finite")));
        }
        let phonemes: Vec<ProfilePhoneme> = phonemes
            .into_iter()
            .map(|(name, mfcc)| ProfilePhoneme {
                vowel: Vowel::from_phoneme_name(&name),
                name,
                mfcc,
            })
            .collect();
        if phonemes
            .iter()
            .any(|p| !p.mfcc.iter().all(|v| v.is_finite()))
        {
            return Err(invalid("averaged MFCC is not finite"));
        }
        if phonemes.iter().all(|p| p.vowel.is_none()) {
            return Err(invalid("no phoneme is named A, I, U, E or O"));
        }
        let profile = LipSyncProfile {
            target_sample_rate: self.target_sample_rate,
            sample_count: self.sample_count,
            mel_filter_bank_channels: self.mel,
            compare_method: self.compare,
            use_standardization,
            phonemes,
            means,
            standard_deviations,
        };
        profile.validate()?;
        Ok(profile)
    }
}

fn coefficients(values: &[f32], what: &str) -> Result<[f32; MFCC_NUM], LipSyncError> {
    let array: [f32; MFCC_NUM] = values.try_into().map_err(|_| {
        invalid(format!(
            "{what} has {} coefficients, expected {MFCC_NUM}",
            values.len()
        ))
    })?;
    if array.iter().any(|v| !v.is_finite()) {
        return Err(invalid(format!("{what} contains a non-finite value")));
    }
    if array.iter().any(|v| v.abs() > MAX_COEFFICIENT) {
        return Err(invalid(format!(
            "{what} exceeds coefficient magnitude limit"
        )));
    }
    Ok(array)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LipSyncError> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| invalid("binary profile is truncated"))?;
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, LipSyncError> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> Result<i32, LipSyncError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn floats(&mut self) -> Result<[f32; MFCC_NUM], LipSyncError> {
        let mut out = [0f32; MFCC_NUM];
        for v in &mut out {
            *v = f32::from_be_bytes(self.take(4)?.try_into().unwrap());
        }
        Ok(out)
    }
}

/// How interleaved multi-channel audio becomes the mono analysis signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMode {
    /// Arithmetic mean of all channels in each frame.
    #[default]
    Average,
    /// First channel only, as uLipSync `OnDataReceived` and the wLipSync
    /// worklet do.
    First,
}

/// Per-voice tuning. Defaults are the uLipSync / wLipSync defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LipSyncSettings {
    /// log10(RMS) at or below which the voice counts as silent.
    pub min_volume: f32,
    /// log10(RMS) at or above which the mouth is fully open.
    pub max_volume: f32,
    /// SmoothDamp time in seconds; 0 disables smoothing.
    pub smoothness: f32,
    /// true: target each vowel's summed score ratio (uLipSync
    /// `usePhonemeBlend`). false: one-hot on the best phoneme.
    pub use_phoneme_blend: bool,
    /// Per-vowel output gain (uLipSync `BlendShapeInfo.maxWeight`), order A I U E O.
    pub vowel_gains: [f32; 5],
    /// Seconds of input between analysis frames (wLipSync `blockSize` 512 @ 48 kHz).
    pub hop_seconds: f32,
    pub channel_mode: ChannelMode,
}

impl Default for LipSyncSettings {
    fn default() -> Self {
        Self {
            min_volume: -2.5,
            max_volume: -1.5,
            smoothness: 0.05,
            use_phoneme_blend: false,
            vowel_gains: [1.0; 5],
            hop_seconds: 512.0 / 48_000.0,
            channel_mode: ChannelMode::Average,
        }
    }
}

impl LipSyncSettings {
    pub fn validate(&self) -> Result<(), LipSyncError> {
        let bad = |m: String| Err(LipSyncError::InvalidSettings(m));
        let vols_ok =
            (-10.0..=1.0).contains(&self.min_volume) && (-10.0..=1.0).contains(&self.max_volume);
        if !vols_ok || self.min_volume >= self.max_volume {
            return bad(format!(
                "need -10 <= min_volume < max_volume <= 1, got {} / {}",
                self.min_volume, self.max_volume
            ));
        }
        if !(0.0..=1.0).contains(&self.smoothness) {
            return bad(format!("smoothness {} is outside 0..=1 s", self.smoothness));
        }
        if let Some(g) = self.vowel_gains.iter().find(|g| !(0.0..=4.0).contains(*g)) {
            return bad(format!("vowel gain {g} is outside 0..=4"));
        }
        if !(MIN_HOP_SECONDS..=1.0).contains(&self.hop_seconds) {
            return bad(format!(
                "hop_seconds {} is outside [{MIN_HOP_SECONDS}, 1]",
                self.hop_seconds
            ));
        }
        Ok(())
    }
}

/// Latest analysis result.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LipSyncOutput {
    /// Mouth weights A I U E O in 0..=1: smoothed vowel weight x smoothed
    /// amplitude x vowel gain, as uLipSyncBlendShape applies them.
    pub vowels: [f32; 5],
    /// Smoothed normalised volume in 0..=1.
    pub amplitude: f32,
    /// RMS of the analysis window (uLipSync `rawVolume`).
    pub raw_rms: f32,
    /// Unsmoothed per-vowel share of the normalised scores (sums to <= 1;
    /// non-vowel phonemes such as "S" take the rest).
    pub vowel_ratios: [f32; 5],
    /// Best-scoring profile phoneme, None while the spectrum is degenerate
    /// (digital silence).
    pub main_phoneme: Option<usize>,
    /// Input samples (per channel) consumed when this frame was analysed.
    pub sample_position: u64,
    /// Analysis frames run since construction or reset.
    pub frames: u64,
}

struct MelBand {
    begin: usize,
    weights: Vec<f32>,
}

/// Streaming analyser for one voice at one input sample rate.
pub struct LipSyncAnalyzer {
    profile: Arc<LipSyncProfile>,
    settings: LipSyncSettings,
    input_rate: u32,
    hop: usize,
    // Ring buffer of the last `input_len` mono samples.
    ring: Vec<f32>,
    write: usize,
    since_frame: usize,
    position: u64,
    // Resampling plan.
    skip: usize,
    df: f32,
    data_len: usize,
    // Preallocated working buffers.
    frame: Vec<f32>,
    data: Vec<f32>,
    lp_taps: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    twiddle: Vec<(f32, f32)>,
    bit_reverse: Vec<usize>,
    spectrum: Vec<f32>,
    mel_bands: Vec<MelBand>,
    mel: Vec<f32>,
    dct: Vec<f32>,
    mfcc: [f32; MFCC_NUM],
    phoneme_norm: Vec<[f32; MFCC_NUM]>,
    scores: Vec<f32>,
    // Smoothing state (uLipSyncBlendShape).
    volume: f32,
    volume_velocity: f32,
    weights: [f32; 5],
    weight_velocity: [f32; 5],
    output: LipSyncOutput,
}

impl LipSyncAnalyzer {
    pub fn new(
        profile: Arc<LipSyncProfile>,
        input_sample_rate: u32,
        settings: LipSyncSettings,
    ) -> Result<Self, LipSyncError> {
        settings.validate()?;
        profile.validate()?;
        let rate = input_sample_rate;
        let target = profile.target_sample_rate;
        let unsupported =
            |reason: String| Err(LipSyncError::UnsupportedSampleRate { rate, reason });
        if !(MIN_INPUT_SAMPLE_RATE..=MAX_INPUT_SAMPLE_RATE).contains(&rate) {
            return unsupported(format!(
                "supported range is {MIN_INPUT_SAMPLE_RATE}..={MAX_INPUT_SAMPLE_RATE}"
            ));
        }
        if rate < target {
            // uLipSync would analyse an unresampled, non-power-of-two frame here.
            return unsupported(format!("below the profile's {target} Hz target rate"));
        }
        let n = profile.sample_count;
        // uLipSync.inputSampleCount and Algorithm.DownSample.
        let ratio = rate as f32 / target as f32;
        let input_len = (n as f32 * ratio).ceil() as usize;
        let (skip, df, data_len) = if rate == target {
            (1, 1.0, input_len)
        } else if rate.is_multiple_of(target) {
            let skip = (rate / target) as usize;
            (skip, skip as f32, input_len / skip)
        } else {
            (
                0,
                ratio,
                (input_len as f32 / ratio).round_ties_even() as usize,
            )
        };
        // Upstream's radix-2 transform is undefined for non-power-of-two
        // resampled lengths. Refuse those plans instead of silently truncating.
        if data_len != n {
            return unsupported(format!("resamples to {data_len} samples instead of {n}"));
        }

        if input_len > MAX_INPUT_WINDOW {
            return unsupported(format!(
                "analysis input window exceeds {MAX_INPUT_WINDOW} samples"
            ));
        }
        // Algorithm.LowPassFilter coefficients.
        let cutoff = ((target / 2) as f32 - LOW_PASS_RANGE_HZ) / rate as f32;
        let range = LOW_PASS_RANGE_HZ / rate as f32;
        let mut taps = (3.1f32 / range).round_ties_even() as usize;
        if (taps + 1).is_multiple_of(2) {
            taps += 1;
        }
        if n.saturating_mul(taps) > MAX_FILTER_PRODUCTS {
            return unsupported(format!(
                "filter plan exceeds {MAX_FILTER_PRODUCTS} multiply-adds per frame"
            ));
        }
        let lp_taps = (0..taps)
            .map(|i| {
                let x = i as f32 - (taps - 1) as f32 / 2.0;
                let ang = 2.0 * std::f32::consts::PI * cutoff * x;
                2.0 * cutoff * ang.sin() / ang
            })
            .collect();

        // Radix-2 FFT tables (wLipSync fft.c).
        let bits = n.trailing_zeros();
        let bit_reverse = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let twiddle = (0..n / 2)
            .map(|j| {
                let theta = -2.0 * std::f32::consts::PI * j as f32 / n as f32;
                (theta.cos(), theta.sin())
            })
            .collect();

        let mel_channels = profile.mel_filter_bank_channels;
        let mel_bands = mel_filter_bank(n, target as f32, mel_channels);
        if mel_bands.iter().any(|b| {
            b.weights.is_empty()
                || b.begin + b.weights.len() > n
                || b.weights.iter().any(|w| !w.is_finite())
        }) {
            return Err(invalid(
                "sampleCount/melFilterBankChannels produce an empty or invalid mel band",
            ));
        }
        // Algorithm.DCT cosine table.
        let a = std::f32::consts::PI / mel_channels as f32;
        let dct = (0..=MFCC_NUM)
            .flat_map(|i| (0..mel_channels).map(move |j| ((j as f32 + 0.5) * i as f32 * a).cos()))
            .collect();

        let phoneme_norm = profile
            .phonemes
            .iter()
            .map(|p| {
                std::array::from_fn(|i| {
                    (p.mfcc[i] - profile.means[i]) / profile.standard_deviations[i]
                })
            })
            .collect();
        let phonemes = profile.phonemes.len();
        Ok(Self {
            hop: hop_samples(settings.hop_seconds, rate),
            settings,
            input_rate: rate,
            ring: vec![0.0; input_len],
            write: 0,
            since_frame: 0,
            position: 0,
            skip,
            df,
            data_len,
            frame: vec![0.0; input_len],
            data: vec![0.0; data_len],
            lp_taps,
            re: vec![0.0; n],
            im: vec![0.0; n],
            twiddle,
            bit_reverse,
            spectrum: vec![0.0; n],
            mel_bands,
            mel: vec![0.0; mel_channels],
            dct,
            mfcc: [0.0; MFCC_NUM],
            phoneme_norm,
            scores: vec![0.0; phonemes],
            volume: 0.0,
            volume_velocity: 0.0,
            weights: [0.0; 5],
            weight_velocity: [0.0; 5],
            output: LipSyncOutput::default(),
            profile,
        })
    }

    pub fn profile(&self) -> &Arc<LipSyncProfile> {
        &self.profile
    }

    pub fn input_sample_rate(&self) -> u32 {
        self.input_rate
    }

    pub fn settings(&self) -> &LipSyncSettings {
        &self.settings
    }

    /// Input samples per analysis window (uLipSync `inputSampleCount`).
    pub fn window_len(&self) -> usize {
        self.ring.len()
    }

    /// Input samples between analysis frames.
    pub fn hop_len(&self) -> usize {
        self.hop
    }

    pub fn set_settings(&mut self, settings: LipSyncSettings) -> Result<(), LipSyncError> {
        settings.validate()?;
        self.hop = hop_samples(settings.hop_seconds, self.input_rate);
        self.since_frame = self.since_frame.min(self.hop - 1);
        self.settings = settings;
        Ok(())
    }

    pub fn output(&self) -> &LipSyncOutput {
        &self.output
    }

    /// Latest MFCC (uLipSync `mfcc`), useful for calibration tooling.
    pub fn mfcc(&self) -> &[f32; MFCC_NUM] {
        &self.mfcc
    }

    /// Latest normalised score per profile phoneme.
    pub fn phoneme_scores(&self) -> &[f32] {
        &self.scores
    }

    /// Clears buffered audio, smoothing and output, e.g. between utterances.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write = 0;
        self.since_frame = 0;
        self.position = 0;
        self.mfcc = [0.0; MFCC_NUM];
        self.scores.fill(0.0);
        self.volume = 0.0;
        self.volume_velocity = 0.0;
        self.weights = [0.0; 5];
        self.weight_velocity = [0.0; 5];
        self.output = LipSyncOutput::default();
    }

    /// Pushes mono samples at the input rate. Returns how many analysis
    /// frames ran. PCM must be normalized to -1..=1. An invalid chunk is
    /// rejected whole, before consuming any samples.
    pub fn push_mono(&mut self, samples: &[f32]) -> Result<usize, LipSyncError> {
        if let Some(index) = samples.iter().position(|s| !s.is_finite()) {
            return Err(LipSyncError::NonFiniteSample { index });
        }
        if let Some(index) = samples.iter().position(|s| s.abs() > 1.0) {
            return Err(LipSyncError::SampleOutOfRange { index });
        }
        Ok(samples
            .iter()
            .map(|&s| self.push_sample(s))
            .filter(|&ran| ran)
            .count())
    }

    /// Pushes interleaved frames, downmixed per [`ChannelMode`].
    pub fn push_interleaved(
        &mut self,
        samples: &[f32],
        channels: usize,
    ) -> Result<usize, LipSyncError> {
        if !(1..=32).contains(&channels) || !samples.len().is_multiple_of(channels) {
            return Err(LipSyncError::InvalidChannels {
                channels,
                len: samples.len(),
            });
        }
        if let Some(index) = samples.iter().position(|s| !s.is_finite()) {
            return Err(LipSyncError::NonFiniteSample { index });
        }
        if let Some(index) = samples.iter().position(|s| s.abs() > 1.0) {
            return Err(LipSyncError::SampleOutOfRange { index });
        }
        let mode = self.settings.channel_mode;
        let mut frames = 0;
        for frame in samples.chunks_exact(channels) {
            let mono = match mode {
                ChannelMode::First => frame[0],
                ChannelMode::Average => frame.iter().sum::<f32>() / channels as f32,
            };
            frames += self.push_sample(mono) as usize;
        }
        Ok(frames)
    }

    fn push_sample(&mut self, sample: f32) -> bool {
        self.ring[self.write] = sample;
        self.write = (self.write + 1) % self.ring.len();
        self.position += 1;
        self.since_frame += 1;
        if self.since_frame < self.hop {
            return false;
        }
        self.since_frame = 0;
        self.analyze();
        true
    }

    fn analyze(&mut self) {
        let len = self.ring.len();
        // CopyRingBuffer: oldest sample first.
        let (head, tail) = self.ring.split_at(self.write);
        self.frame[..tail.len()].copy_from_slice(tail);
        self.frame[tail.len()..].copy_from_slice(head);

        let raw_rms = (self.frame.iter().map(|x| x * x).sum::<f32>() / len as f32).sqrt();

        self.low_pass_and_down_sample();
        let data = &mut self.data;
        // PreEmphasis(0.97)
        for i in (1..data.len()).rev() {
            data[i] -= PRE_EMPHASIS * data[i - 1];
        }
        // HammingWindow
        let last = (data.len() - 1) as f32;
        for (i, x) in data.iter_mut().enumerate() {
            *x *= 0.54 - 0.46 * (2.0 * std::f32::consts::PI * (i as f32 / last)).cos();
        }
        // Normalize(1)
        let max = data.iter().fold(0f32, |m, x| m.max(x.abs()));
        if max >= EPSILON {
            let r = 1.0 / max;
            data.iter_mut().for_each(|x| *x *= r);
        }

        self.fft();
        for (band, out) in self.mel_bands.iter().zip(self.mel.iter_mut()) {
            let bins = &self.spectrum[band.begin..band.begin + band.weights.len()];
            *out = band
                .weights
                .iter()
                .zip(bins)
                .fold(0.0, |sum, (a, s)| sum + a * s);
        }
        // PowerToDb
        self.mel.iter_mut().for_each(|m| *m = 10.0 * m.log10());
        // DCT, keeping cepstrum[1..=12].
        let channels = self.mel.len();
        for (k, out) in self.mfcc.iter_mut().enumerate() {
            let row = &self.dct[(k + 1) * channels..(k + 2) * channels];
            *out = row
                .iter()
                .zip(&self.mel)
                .fold(0.0, |sum, (c, m)| sum + m * c);
        }

        let main = if self.mfcc.iter().all(|v| v.is_finite()) && raw_rms > 0.0 {
            self.score()
        } else {
            self.mfcc.fill(0.0);
            self.scores.fill(0.0);
            None
        };
        self.update_output(raw_rms, main);
    }

    /// Algorithm.LowPassFilter followed by Algorithm.DownSample, evaluating
    /// the filter only at the samples the decimation keeps. uLipSync's filter
    /// adds the convolution to the original signal; that is kept as is.
    fn low_pass_and_down_sample(&mut self) {
        let len = self.frame.len();
        for j in 0..self.data_len {
            let i = if self.skip > 0 {
                j * self.skip
            } else {
                // DownSample2 interpolates between input[i0] and
                // input[min(i0, len - 1)], which is input[i0].
                ((self.df * j as f32).floor() as usize).min(len - 1)
            };
            let mut acc = self.frame[i];
            for (k, b) in self.lp_taps.iter().take(i + 1).enumerate() {
                acc += b * self.frame[i - k];
            }
            self.data[j] = acc;
        }
    }

    fn fft(&mut self) {
        let n = self.re.len();
        for (i, &r) in self.bit_reverse.iter().enumerate() {
            self.re[i] = self.data[r];
            self.im[i] = 0.0;
        }
        let mut half = 1;
        while half < n {
            let stride = n / (half * 2);
            for start in (0..n).step_by(2 * half) {
                for j in 0..half {
                    let (cx, cy) = self.twiddle[j * stride];
                    let (er, ei) = (self.re[start + j], self.im[start + j]);
                    let (or, oi) = (self.re[start + j + half], self.im[start + j + half]);
                    let tr = cx * or - cy * oi;
                    let ti = cx * oi + cy * or;
                    self.re[start + j] = er + tr;
                    self.im[start + j] = ei + ti;
                    self.re[start + j + half] = er - tr;
                    self.im[start + j + half] = ei - ti;
                }
            }
            half <<= 1;
        }
        for ((s, r), i) in self.spectrum.iter_mut().zip(&self.re).zip(&self.im) {
            *s = (r * r + i * i).sqrt();
        }
    }

    /// LipSyncJob.CalcScores + GetVowel.
    fn score(&mut self) -> Option<usize> {
        let profile = &self.profile;
        let n = MFCC_NUM as f32;
        let x: [f32; MFCC_NUM] = std::array::from_fn(|i| {
            (self.mfcc[i] - profile.means[i]) / profile.standard_deviations[i]
        });
        let mut sum = 0f32;
        for (score, y) in self.scores.iter_mut().zip(&self.phoneme_norm) {
            *score = match profile.compare_method {
                CompareMethod::L1Norm => {
                    let d = x.iter().zip(y).fold(0.0, |d, (x, y)| d + (x - y).abs()) / n;
                    10f32.powf(-d)
                }
                CompareMethod::L2Norm => {
                    let d = x.iter().zip(y).fold(0.0, |d, (x, y)| d + (x - y).powf(2.0));
                    10f32.powf(-(d / n).sqrt())
                }
                CompareMethod::CosineSimilarity => {
                    let (mut xn, mut yn, mut prod) = (0f32, 0f32, 0f32);
                    for (x, y) in x.iter().zip(y) {
                        xn += x * x;
                        yn += y * y;
                        prod += x * y;
                    }
                    (prod / (xn.sqrt() * yn.sqrt())).max(0.0).powf(100.0)
                }
            };
            sum += *score;
        }
        // `sum > 0` is false for NaN, so a degenerate (silent) spectrum yields
        // all-zero scores exactly as upstream; upstream would then still name
        // phoneme 0, which is reported here as None.
        if !(sum > 0.0 && sum.is_finite()) {
            self.scores.fill(0.0);
            return None;
        }
        self.scores.iter_mut().for_each(|s| *s /= sum);
        let mut best = 0;
        for (i, s) in self.scores.iter().enumerate() {
            if *s > self.scores[best] {
                best = i;
            }
        }
        Some(best)
    }

    /// uLipSync.UpdateResult + uLipSyncBlendShape.UpdateVolume/UpdateVowels.
    fn update_output(&mut self, raw_rms: f32, main: Option<usize>) {
        let s = self.settings;
        let dt = self.hop as f32 / self.input_rate as f32;
        let mut ratios = [0f32; 5];
        for (p, score) in self.profile.phonemes.iter().zip(&self.scores) {
            if let Some(v) = p.vowel {
                ratios[v as usize] += score;
            }
        }
        let main_vowel = main.and_then(|i| self.profile.phonemes[i].vowel);

        let target_volume = if raw_rms > 0.0 {
            ((raw_rms.log10() - s.min_volume) / (s.max_volume - s.min_volume).max(1e-4))
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.volume = smooth_damp(
            self.volume,
            target_volume,
            &mut self.volume_velocity,
            s.smoothness,
            dt,
        );

        let mut sum = 0f32;
        for v in Vowel::ALL {
            let i = v as usize;
            let target = if s.use_phoneme_blend {
                ratios[i]
            } else {
                (main_vowel == Some(v)) as u8 as f32
            };
            self.weights[i] = smooth_damp(
                self.weights[i],
                target,
                &mut self.weight_velocity[i],
                s.smoothness,
                dt,
            );
            sum += self.weights[i];
        }
        for w in &mut self.weights {
            *w = if sum > 0.0 { *w / sum } else { 0.0 };
        }

        let amplitude = self.volume.clamp(0.0, 1.0);
        self.output = LipSyncOutput {
            vowels: std::array::from_fn(|i| {
                (self.weights[i] * amplitude * s.vowel_gains[i]).clamp(0.0, 1.0)
            }),
            amplitude,
            raw_rms,
            vowel_ratios: ratios,
            main_phoneme: main,
            sample_position: self.position,
            frames: self.output.frames + 1,
        };
    }
}

fn hop_samples(hop_seconds: f32, rate: u32) -> usize {
    ((hop_seconds * rate as f32).round() as usize).max(1)
}

/// Algorithm.MelFilterBank weights, precomputed with the same arithmetic.
fn mel_filter_bank(len: usize, sample_rate: f32, mel_div: usize) -> Vec<MelBand> {
    let to_mel = |hz: f32| 1127.0 * (hz / 700.0 + 1.0).ln();
    let to_hz = |mel: f32| 700.0 * ((mel / 1127.0).exp() - 1.0);
    let f_max = sample_rate / 2.0;
    let mel_max = to_mel(f_max);
    let n_max = len / 2;
    let df = f_max / n_max as f32;
    let d_mel = mel_max / (mel_div + 1) as f32;
    (0..mel_div)
        .map(|n| {
            let f_begin = to_hz(d_mel * n as f32);
            let f_center = to_hz(d_mel * (n + 1) as f32);
            let f_end = to_hz(d_mel * (n + 2) as f32);
            let i_begin = (f_begin / df).ceil() as i64;
            let i_center = (f_center / df).round_ties_even() as i64;
            let i_end = ((f_end / df).floor() as i64).min(n_max as i64);
            let begin = (i_begin + 1).max(0) as usize;
            let weights = (begin as i64..=i_end)
                .map(|i| {
                    let f = df * i as f32;
                    let a = if i < i_center {
                        (f - f_begin) / (f_center - f_begin)
                    } else {
                        (f_end - f) / (f_end - f_center)
                    };
                    a / ((f_end - f_begin) * 0.5)
                })
                .collect();
            MelBand { begin, weights }
        })
        .collect()
}

/// wLipSync `smoothDamp` (critically damped spring, Game Programming Gems 4).
fn smooth_damp(current: f32, target: f32, velocity: &mut f32, smoothness: f32, dt: f32) -> f32 {
    if smoothness <= 0.0 {
        *velocity = 0.0;
        return target;
    }
    let omega = 2.0 / smoothness;
    let x = omega * dt;
    let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (*velocity + omega * change) * dt;
    *velocity = (*velocity - omega * temp) * exp;
    target + (change + temp) * exp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic test-only profile: three phonemes with made-up coefficients.
    /// It exercises the math; it is not a usable classifier.
    fn synthetic_profile_json(compare: u8, standardize: bool) -> String {
        let names = ["A", "I", "S"];
        let mfccs: Vec<String> = names
            .iter()
            .enumerate()
            .map(|(p, name)| {
                let arrays: Vec<String> = (0..2)
                    .map(|s| {
                        let v: Vec<String> = (0..MFCC_NUM)
                            .map(|i| format!("{}", synthetic_coefficient(p, s, i)))
                            .collect();
                        format!("{{\"array\":[{}]}}", v.join(","))
                    })
                    .collect();
                format!(
                    "{{\"name\":\"{name}\",\"mfccCalibrationDataList\":[{}]}}",
                    arrays.join(",")
                )
            })
            .collect();
        format!(
            "{{\"mfccNum\":12,\"mfccDataCount\":2,\"melFilterBankChannels\":30,\"targetSampleRate\":16000,\
             \"sampleCount\":1024,\"useStandardization\":{standardize},\"compareMethod\":{compare},\"mfccs\":[{}]}}",
            mfccs.join(",")
        )
    }

    fn synthetic_coefficient(p: usize, s: usize, i: usize) -> f32 {
        (((p * 31 + s * 7 + i * 13) % 41) as f32 - 20.0) * 2.5 + if i == 0 { 90.0 } else { 0.0 }
    }

    fn profile(compare: u8, standardize: bool) -> Arc<LipSyncProfile> {
        Arc::new(
            LipSyncProfile::from_bytes(synthetic_profile_json(compare, standardize).as_bytes())
                .unwrap(),
        )
    }

    /// Deterministic voiced test signal: 140 Hz harmonics with formant-like
    /// peaks and a little LCG noise. Matches the C reference harness.
    fn signal(rate: u32, len: usize) -> Vec<f32> {
        let mut seed: u32 = 12345;
        (0..len)
            .map(|n| {
                let t = n as f64 / rate as f64;
                let mut v = 0.0;
                for h in 1..=20 {
                    let f = 140.0 * h as f64;
                    let amp = 1.0 / h as f64
                        * (1.0
                            + 3.0 * (-((f - 700.0) / 150.0).powi(2)).exp()
                            + 2.0 * (-((f - 1200.0) / 200.0).powi(2)).exp());
                    v += amp * (2.0 * std::f64::consts::PI * f * t).sin();
                }
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = ((seed >> 16) & 0x7fff) as f64 / 32768.0 - 0.5;
                (0.05 * v + 0.01 * noise) as f32
            })
            .collect()
    }

    fn run_window(compare: u8, standardize: bool, rate: u32) -> LipSyncAnalyzer {
        let settings = LipSyncSettings {
            hop_seconds: 1.0,
            ..Default::default()
        };
        let mut a = LipSyncAnalyzer::new(profile(compare, standardize), rate, settings).unwrap();
        let input = signal(rate, a.window_len());
        a.hop = a.window_len();
        assert_eq!(a.push_mono(&input).unwrap(), 1);
        a
    }

    fn assert_close(actual: &[f32], expected: &[f32], tol: f32, what: &str) {
        assert_eq!(actual.len(), expected.len());
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (a - e).abs() <= tol * e.abs().max(1.0),
                "{what}[{i}]: got {a}, expected {e}"
            );
        }
    }

    // Golden values from the wLipSync C sources (src/algo, src/score at
    // 177f3ac) compiled natively with pt_math.h replaced by libm float
    // functions (uLipSync's exact math), fed the same signal and profile.
    // Regenerate with work/lipsync-golden (see LIPSYNC-NOTICES.txt).
    struct Golden {
        rate: u32,
        compare: u8,
        standardize: bool,
        rms: f32,
        mfcc: [f32; 12],
        scores: [f32; 3],
    }
    // Preserve the native reference harness's decimal output for traceability.
    #[allow(clippy::excessive_precision)]
    const GOLDEN: &[Golden] = &[
        Golden {
            rate: 16000,
            compare: 0,
            standardize: false,
            rms: 0.059100233f32,
            mfcc: [
                15.6984301f32,
                -42.5527725f32,
                -49.255455f32,
                -22.8356895f32,
                -26.9806957f32,
                -12.6797171f32,
                -7.30593681f32,
                -12.812356f32,
                -6.95306015f32,
                -6.01005077f32,
                -20.1008587f32,
                -19.5033512f32,
            ],
            scores: [0.999998987f32, 1.05744368e-06f32, 5.56530126e-12f32],
        },
        Golden {
            rate: 16000,
            compare: 0,
            standardize: true,
            rms: 0.059100233f32,
            mfcc: [
                15.6984301f32,
                -42.5527725f32,
                -49.255455f32,
                -22.8356895f32,
                -26.9806957f32,
                -12.6797171f32,
                -7.30593681f32,
                -12.812356f32,
                -6.95306015f32,
                -6.01005077f32,
                -20.1008587f32,
                -19.5033512f32,
            ],
            scores: [0.423256725f32, 0.345554382f32, 0.231188953f32],
        },
        Golden {
            rate: 16000,
            compare: 1,
            standardize: false,
            rms: 0.059100233f32,
            mfcc: [
                15.6984301f32,
                -42.5527725f32,
                -49.255455f32,
                -22.8356895f32,
                -26.9806957f32,
                -12.6797171f32,
                -7.30593681f32,
                -12.812356f32,
                -6.95306015f32,
                -6.01005077f32,
                -20.1008587f32,
                -19.5033512f32,
            ],
            scores: [1.0f32, 4.81281811e-12f32, 4.57194439e-12f32],
        },
        Golden {
            rate: 16000,
            compare: 1,
            standardize: true,
            rms: 0.059100233f32,
            mfcc: [
                15.6984301f32,
                -42.5527725f32,
                -49.255455f32,
                -22.8356895f32,
                -26.9806957f32,
                -12.6797171f32,
                -7.30593681f32,
                -12.812356f32,
                -6.95306015f32,
                -6.01005077f32,
                -20.1008587f32,
                -19.5033512f32,
            ],
            scores: [0.388458192f32, 0.293648183f32, 0.317893624f32],
        },
        Golden {
            rate: 16000,
            compare: 2,
            standardize: false,
            rms: 0.059100233f32,
            mfcc: [
                15.6984301f32,
                -42.5527725f32,
                -49.255455f32,
                -22.8356895f32,
                -26.9806957f32,
                -12.6797171f32,
                -7.30593681f32,
                -12.812356f32,
                -6.95306015f32,
                -6.01005077f32,
                -20.1008587f32,
                -19.5033512f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 16000,
            compare: 2,
            standardize: true,
            rms: 0.059100233f32,
            mfcc: [
                15.6984301f32,
                -42.5527725f32,
                -49.255455f32,
                -22.8356895f32,
                -26.9806957f32,
                -12.6797171f32,
                -7.30593681f32,
                -12.812356f32,
                -6.95306015f32,
                -6.01005077f32,
                -20.1008587f32,
                -19.5033512f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 24000,
            compare: 0,
            standardize: false,
            rms: 0.0591281429f32,
            mfcc: [
                8.2707119f32,
                -36.1038933f32,
                -59.4747162f32,
                -17.5198765f32,
                -29.6662235f32,
                -16.1785583f32,
                -5.6045208f32,
                -13.9222374f32,
                -9.48683548f32,
                -4.48270655f32,
                -23.4019394f32,
                -20.4175949f32,
            ],
            scores: [0.999998271f32, 1.75871946e-06f32, 1.79619618e-11f32],
        },
        Golden {
            rate: 24000,
            compare: 0,
            standardize: true,
            rms: 0.0591281429f32,
            mfcc: [
                8.2707119f32,
                -36.1038933f32,
                -59.4747162f32,
                -17.5198765f32,
                -29.6662235f32,
                -16.1785583f32,
                -5.6045208f32,
                -13.9222374f32,
                -9.48683548f32,
                -4.48270655f32,
                -23.4019394f32,
                -20.4175949f32,
            ],
            scores: [0.411263913f32, 0.34628433f32, 0.242451772f32],
        },
        Golden {
            rate: 24000,
            compare: 1,
            standardize: false,
            rms: 0.0591281429f32,
            mfcc: [
                8.2707119f32,
                -36.1038933f32,
                -59.4747162f32,
                -17.5198765f32,
                -29.6662235f32,
                -16.1785583f32,
                -5.6045208f32,
                -13.9222374f32,
                -9.48683548f32,
                -4.48270655f32,
                -23.4019394f32,
                -20.4175949f32,
            ],
            scores: [1.0f32, 1.23246387e-11f32, 3.48304996e-11f32],
        },
        Golden {
            rate: 24000,
            compare: 1,
            standardize: true,
            rms: 0.0591281429f32,
            mfcc: [
                8.2707119f32,
                -36.1038933f32,
                -59.4747162f32,
                -17.5198765f32,
                -29.6662235f32,
                -16.1785583f32,
                -5.6045208f32,
                -13.9222374f32,
                -9.48683548f32,
                -4.48270655f32,
                -23.4019394f32,
                -20.4175949f32,
            ],
            scores: [0.360301882f32, 0.292973042f32, 0.346725047f32],
        },
        Golden {
            rate: 24000,
            compare: 2,
            standardize: false,
            rms: 0.0591281429f32,
            mfcc: [
                8.2707119f32,
                -36.1038933f32,
                -59.4747162f32,
                -17.5198765f32,
                -29.6662235f32,
                -16.1785583f32,
                -5.6045208f32,
                -13.9222374f32,
                -9.48683548f32,
                -4.48270655f32,
                -23.4019394f32,
                -20.4175949f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 24000,
            compare: 2,
            standardize: true,
            rms: 0.0591281429f32,
            mfcc: [
                8.2707119f32,
                -36.1038933f32,
                -59.4747162f32,
                -17.5198765f32,
                -29.6662235f32,
                -16.1785583f32,
                -5.6045208f32,
                -13.9222374f32,
                -9.48683548f32,
                -4.48270655f32,
                -23.4019394f32,
                -20.4175949f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 44100,
            compare: 0,
            standardize: false,
            rms: 0.0592065901f32,
            mfcc: [
                15.3633747f32,
                -44.4079666f32,
                -51.4816017f32,
                -24.4898434f32,
                -26.4199715f32,
                -17.6679306f32,
                -4.39170361f32,
                -16.8819084f32,
                -7.00777531f32,
                -6.6254282f32,
                -22.6864357f32,
                -18.8877716f32,
            ],
            scores: [0.99999994f32, 6.7722695e-08f32, 2.36696188e-12f32],
        },
        Golden {
            rate: 44100,
            compare: 0,
            standardize: true,
            rms: 0.0592065901f32,
            mfcc: [
                15.3633747f32,
                -44.4079666f32,
                -51.4816017f32,
                -24.4898434f32,
                -26.4199715f32,
                -17.6679306f32,
                -4.39170361f32,
                -16.8819084f32,
                -7.00777531f32,
                -6.6254282f32,
                -22.6864357f32,
                -18.8877716f32,
            ],
            scores: [0.435108662f32, 0.327244848f32, 0.237646446f32],
        },
        Golden {
            rate: 44100,
            compare: 1,
            standardize: false,
            rms: 0.0592065901f32,
            mfcc: [
                15.3633747f32,
                -44.4079666f32,
                -51.4816017f32,
                -24.4898434f32,
                -26.4199715f32,
                -17.6679306f32,
                -4.39170361f32,
                -16.8819084f32,
                -7.00777531f32,
                -6.6254282f32,
                -22.6864357f32,
                -18.8877716f32,
            ],
            scores: [1.0f32, 1.00066622e-11f32, 8.63028052e-12f32],
        },
        Golden {
            rate: 44100,
            compare: 1,
            standardize: true,
            rms: 0.0592065901f32,
            mfcc: [
                15.3633747f32,
                -44.4079666f32,
                -51.4816017f32,
                -24.4898434f32,
                -26.4199715f32,
                -17.6679306f32,
                -4.39170361f32,
                -16.8819084f32,
                -7.00777531f32,
                -6.6254282f32,
                -22.6864357f32,
                -18.8877716f32,
            ],
            scores: [0.376750857f32, 0.29638657f32, 0.326862574f32],
        },
        Golden {
            rate: 44100,
            compare: 2,
            standardize: false,
            rms: 0.0592065901f32,
            mfcc: [
                15.3633747f32,
                -44.4079666f32,
                -51.4816017f32,
                -24.4898434f32,
                -26.4199715f32,
                -17.6679306f32,
                -4.39170361f32,
                -16.8819084f32,
                -7.00777531f32,
                -6.6254282f32,
                -22.6864357f32,
                -18.8877716f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 44100,
            compare: 2,
            standardize: true,
            rms: 0.0592065901f32,
            mfcc: [
                15.3633747f32,
                -44.4079666f32,
                -51.4816017f32,
                -24.4898434f32,
                -26.4199715f32,
                -17.6679306f32,
                -4.39170361f32,
                -16.8819084f32,
                -7.00777531f32,
                -6.6254282f32,
                -22.6864357f32,
                -18.8877716f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 48000,
            compare: 0,
            standardize: false,
            rms: 0.0591512844f32,
            mfcc: [
                19.8780479f32,
                -46.7311707f32,
                -49.078701f32,
                -24.3414803f32,
                -27.2970123f32,
                -13.8158741f32,
                -8.15286636f32,
                -13.082303f32,
                -7.80673409f32,
                -6.28357601f32,
                -20.490366f32,
                -20.5749893f32,
            ],
            scores: [0.999998748f32, 1.28630495e-06f32, 1.39900747e-11f32],
        },
        Golden {
            rate: 48000,
            compare: 0,
            standardize: true,
            rms: 0.0591512844f32,
            mfcc: [
                19.8780479f32,
                -46.7311707f32,
                -49.078701f32,
                -24.3414803f32,
                -27.2970123f32,
                -13.8158741f32,
                -8.15286636f32,
                -13.082303f32,
                -7.80673409f32,
                -6.28357601f32,
                -20.490366f32,
                -20.5749893f32,
            ],
            scores: [0.417221069f32, 0.344322443f32, 0.238456517f32],
        },
        Golden {
            rate: 48000,
            compare: 1,
            standardize: false,
            rms: 0.0591512844f32,
            mfcc: [
                19.8780479f32,
                -46.7311707f32,
                -49.078701f32,
                -24.3414803f32,
                -27.2970123f32,
                -13.8158741f32,
                -8.15286636f32,
                -13.082303f32,
                -7.80673409f32,
                -6.28357601f32,
                -20.490366f32,
                -20.5749893f32,
            ],
            scores: [1.0f32, 4.40079639e-11f32, 2.4007166e-11f32],
        },
        Golden {
            rate: 48000,
            compare: 1,
            standardize: true,
            rms: 0.0591512844f32,
            mfcc: [
                19.8780479f32,
                -46.7311707f32,
                -49.078701f32,
                -24.3414803f32,
                -27.2970123f32,
                -13.8158741f32,
                -8.15286636f32,
                -13.082303f32,
                -7.80673409f32,
                -6.28357601f32,
                -20.490366f32,
                -20.5749893f32,
            ],
            scores: [0.3727476f32, 0.301978618f32, 0.325273752f32],
        },
        Golden {
            rate: 48000,
            compare: 2,
            standardize: false,
            rms: 0.0591512844f32,
            mfcc: [
                19.8780479f32,
                -46.7311707f32,
                -49.078701f32,
                -24.3414803f32,
                -27.2970123f32,
                -13.8158741f32,
                -8.15286636f32,
                -13.082303f32,
                -7.80673409f32,
                -6.28357601f32,
                -20.490366f32,
                -20.5749893f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 48000,
            compare: 2,
            standardize: true,
            rms: 0.0591512844f32,
            mfcc: [
                19.8780479f32,
                -46.7311707f32,
                -49.078701f32,
                -24.3414803f32,
                -27.2970123f32,
                -13.8158741f32,
                -8.15286636f32,
                -13.082303f32,
                -7.80673409f32,
                -6.28357601f32,
                -20.490366f32,
                -20.5749893f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
        Golden {
            rate: 96000,
            compare: 0,
            standardize: false,
            rms: 0.0591793284f32,
            mfcc: [
                23.7500134f32,
                -46.9149551f32,
                -53.4287949f32,
                -19.4509907f32,
                -28.0762062f32,
                -17.1593628f32,
                -3.3750515f32,
                -15.1762543f32,
                -8.18845177f32,
                -4.05117083f32,
                -23.6086159f32,
                -19.9475822f32,
            ],
            scores: [0.999999583f32, 3.91081983e-07f32, 8.94451787e-12f32],
        },
        Golden {
            rate: 96000,
            compare: 0,
            standardize: true,
            rms: 0.0591793284f32,
            mfcc: [
                23.7500134f32,
                -46.9149551f32,
                -53.4287949f32,
                -19.4509907f32,
                -28.0762062f32,
                -17.1593628f32,
                -3.3750515f32,
                -15.1762543f32,
                -8.18845177f32,
                -4.05117083f32,
                -23.6086159f32,
                -19.9475822f32,
            ],
            scores: [0.421076447f32, 0.336708874f32, 0.24221465f32],
        },
        Golden {
            rate: 96000,
            compare: 1,
            standardize: false,
            rms: 0.0591793284f32,
            mfcc: [
                23.7500134f32,
                -46.9149551f32,
                -53.4287949f32,
                -19.4509907f32,
                -28.0762062f32,
                -17.1593628f32,
                -3.3750515f32,
                -15.1762543f32,
                -8.18845177f32,
                -4.05117083f32,
                -23.6086159f32,
                -19.9475822f32,
            ],
            scores: [1.0f32, 1.48980261e-09f32, 1.14139691e-10f32],
        },
        Golden {
            rate: 96000,
            compare: 1,
            standardize: true,
            rms: 0.0591793284f32,
            mfcc: [
                23.7500134f32,
                -46.9149551f32,
                -53.4287949f32,
                -19.4509907f32,
                -28.0762062f32,
                -17.1593628f32,
                -3.3750515f32,
                -15.1762543f32,
                -8.18845177f32,
                -4.05117083f32,
                -23.6086159f32,
                -19.9475822f32,
            ],
            scores: [0.345726639f32, 0.321061641f32, 0.33321175f32],
        },
        Golden {
            rate: 96000,
            compare: 2,
            standardize: false,
            rms: 0.0591793284f32,
            mfcc: [
                23.7500134f32,
                -46.9149551f32,
                -53.4287949f32,
                -19.4509907f32,
                -28.0762062f32,
                -17.1593628f32,
                -3.3750515f32,
                -15.1762543f32,
                -8.18845177f32,
                -4.05117083f32,
                -23.6086159f32,
                -19.9475822f32,
            ],
            scores: [0.0f32, 1.0f32, 0.0f32],
        },
        Golden {
            rate: 96000,
            compare: 2,
            standardize: true,
            rms: 0.0591793284f32,
            mfcc: [
                23.7500134f32,
                -46.9149551f32,
                -53.4287949f32,
                -19.4509907f32,
                -28.0762062f32,
                -17.1593628f32,
                -3.3750515f32,
                -15.1762543f32,
                -8.18845177f32,
                -4.05117083f32,
                -23.6086159f32,
                -19.9475822f32,
            ],
            scores: [0.0f32, 0.0f32, 0.0f32],
        },
    ];

    #[test]
    fn matches_upstream_golden_mfcc_and_scores() {
        for g in GOLDEN {
            let a = run_window(g.compare, g.standardize, g.rate);
            assert_close(
                a.mfcc(),
                &g.mfcc,
                2e-3,
                &format!("mfcc @{} cmp{}", g.rate, g.compare),
            );
            assert_close(
                a.phoneme_scores(),
                &g.scores,
                2e-3,
                &format!("scores @{} cmp{}", g.rate, g.compare),
            );
            assert!((a.output().raw_rms - g.rms).abs() < 1e-5);
        }
    }

    #[test]
    fn silence_closes_mouth_and_reports_no_phoneme() {
        let mut a =
            LipSyncAnalyzer::new(profile(1, false), 48_000, LipSyncSettings::default()).unwrap();
        let frames = a.push_mono(&vec![0.0; 48_000]).unwrap();
        assert!(frames > 50);
        let out = a.output();
        assert_eq!(out.main_phoneme, None);
        assert_eq!(out.amplitude, 0.0);
        assert_eq!(out.vowels, [0.0; 5]);
        assert!(
            a.mfcc().iter().all(|v| v.is_finite()),
            "degenerate spectrum is explicitly silent"
        );
    }

    #[test]
    fn voiced_signal_opens_mouth() {
        let mut a =
            LipSyncAnalyzer::new(profile(1, false), 48_000, LipSyncSettings::default()).unwrap();
        a.push_mono(&signal(48_000, 24_000)).unwrap();
        let out = *a.output();
        assert!(out.amplitude > 0.5, "{out:?}");
        assert!(out.main_phoneme.is_some());
        assert!(out.vowels.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(out.vowel_ratios.iter().sum::<f32>() <= 1.0 + 1e-5);
    }

    #[test]
    fn chunk_boundaries_do_not_change_results() {
        let input = signal(44_100, 20_000);
        let run = |chunk: usize| {
            let mut a =
                LipSyncAnalyzer::new(profile(2, true), 44_100, LipSyncSettings::default()).unwrap();
            let frames: usize = input.chunks(chunk).map(|c| a.push_mono(c).unwrap()).sum();
            (frames, *a.output(), *a.mfcc())
        };
        let whole = run(input.len());
        for chunk in [1, 7, 128, 441, 4096] {
            assert_eq!(run(chunk), whole, "chunk {chunk}");
        }
    }

    #[test]
    fn supported_rates_resample_to_the_profile_frame() {
        for rate in [
            16_000, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000, 192_000,
        ] {
            let a =
                LipSyncAnalyzer::new(profile(1, false), rate, LipSyncSettings::default()).unwrap();
            assert!(a.data_len == 1024, "{rate}: {}", a.data_len);
        }
        assert_eq!(
            LipSyncAnalyzer::new(profile(1, false), 48_000, LipSyncSettings::default())
                .unwrap()
                .window_len(),
            3072
        );
        assert_eq!(
            LipSyncAnalyzer::new(profile(1, false), 44_100, LipSyncSettings::default())
                .unwrap()
                .window_len(),
            2823
        );
        for rate in [0, 8_000, 11_025, 22_050, 1_000_000] {
            assert!(matches!(
                LipSyncAnalyzer::new(profile(1, false), rate, LipSyncSettings::default()),
                Err(LipSyncError::UnsupportedSampleRate { .. })
            ));
        }
    }

    #[test]
    fn cross_rate_fixture_retains_classification() {
        let classify = |rate: u32| {
            let mut a =
                LipSyncAnalyzer::new(profile(1, false), rate, LipSyncSettings::default()).unwrap();
            a.push_mono(&signal(rate, rate as usize / 2)).unwrap();
            (a.output().main_phoneme, *a.mfcc())
        };
        let (p48, m48) = classify(48_000);
        let (p44, m44) = classify(44_100);
        assert_eq!(p48, p44);
        assert!(p48.is_some());
        // The upstream non-integral decimator uses floor selection. Together
        // with rate-specific LCG noise, numerical MFCC equality is not expected.
        // Independent upstream-kernel vectors cover each rate's numerics.
        assert!(m44.iter().chain(&m48).all(|v| v.is_finite()));
    }

    #[test]
    fn interleaved_downmix_contract() {
        let mono = signal(48_000, 6_000);
        let mut stereo = Vec::new();
        for &s in &mono {
            stereo.extend([s * 2.0, 0.0]);
        }
        let mut a =
            LipSyncAnalyzer::new(profile(1, false), 48_000, LipSyncSettings::default()).unwrap();
        let mut b =
            LipSyncAnalyzer::new(profile(1, false), 48_000, LipSyncSettings::default()).unwrap();
        a.push_mono(&mono).unwrap();
        b.push_interleaved(&stereo, 2).unwrap();
        assert_eq!(a.output(), b.output());
        let first = LipSyncSettings {
            channel_mode: ChannelMode::First,
            ..Default::default()
        };
        let mut c = LipSyncAnalyzer::new(profile(1, false), 48_000, first).unwrap();
        c.push_interleaved(&stereo, 2).unwrap();
        assert!(c.output().raw_rms > a.output().raw_rms * 1.9);
        assert!(matches!(
            b.push_interleaved(&[0.0; 3], 2),
            Err(LipSyncError::InvalidChannels { .. })
        ));
        assert!(matches!(
            b.push_interleaved(&[0.0; 2], 0),
            Err(LipSyncError::InvalidChannels { .. })
        ));
    }

    #[test]
    fn non_finite_input_is_rejected_without_consuming() {
        let mut a =
            LipSyncAnalyzer::new(profile(1, false), 48_000, LipSyncSettings::default()).unwrap();
        a.push_mono(&signal(48_000, 1_000)).unwrap();
        let before = *a.output();
        let pos = a.position;
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut chunk = vec![0.1; 600];
            chunk[300] = bad;
            assert_eq!(
                a.push_mono(&chunk),
                Err(LipSyncError::NonFiniteSample { index: 300 })
            );
            assert_eq!(
                a.push_interleaved(&chunk, 2),
                Err(LipSyncError::NonFiniteSample { index: 300 })
            );
        }
        assert_eq!(a.position, pos);
        assert_eq!(*a.output(), before);
    }

    #[test]
    fn reset_restores_initial_state() {
        let input = signal(48_000, 9_000);
        let mut a =
            LipSyncAnalyzer::new(profile(0, false), 48_000, LipSyncSettings::default()).unwrap();
        a.push_mono(&input).unwrap();
        let first = *a.output();
        a.reset();
        assert_eq!(*a.output(), LipSyncOutput::default());
        a.push_mono(&input).unwrap();
        assert_eq!(*a.output(), first);
    }

    #[test]
    fn settings_shape_output() {
        let input = signal(48_000, 24_000);
        let run = |settings: LipSyncSettings| {
            let mut a = LipSyncAnalyzer::new(profile(1, false), 48_000, settings).unwrap();
            a.push_mono(&input).unwrap();
            *a.output()
        };
        let base = run(LipSyncSettings::default());
        let muted = run(LipSyncSettings {
            vowel_gains: [0.0; 5],
            ..Default::default()
        });
        assert_eq!(muted.vowels, [0.0; 5]);
        assert_eq!(muted.amplitude, base.amplitude);
        let deaf = run(LipSyncSettings {
            min_volume: 0.0,
            max_volume: 0.5,
            ..Default::default()
        });
        assert_eq!(deaf.amplitude, 0.0);
        let raw = run(LipSyncSettings {
            smoothness: 0.0,
            use_phoneme_blend: true,
            ..Default::default()
        });
        let blend: f32 = raw.vowel_ratios.iter().sum();
        if blend > 0.0 {
            for i in 0..5 {
                let expected = raw.vowel_ratios[i] / blend * raw.amplitude;
                assert!((raw.vowels[i] - expected).abs() < 1e-5);
            }
        }
        for bad in [
            LipSyncSettings {
                min_volume: -1.0,
                max_volume: -2.0,
                ..Default::default()
            },
            LipSyncSettings {
                smoothness: f32::NAN,
                ..Default::default()
            },
            LipSyncSettings {
                vowel_gains: [1.0, 1.0, f32::INFINITY, 1.0, 1.0],
                ..Default::default()
            },
            LipSyncSettings {
                hop_seconds: 0.0,
                ..Default::default()
            },
        ] {
            assert!(matches!(
                bad.validate(),
                Err(LipSyncError::InvalidSettings(_))
            ));
        }
    }

    #[test]
    fn profile_derives_upstream_averages_means_and_deviations() {
        let p = profile(1, true);
        assert_eq!(p.phonemes.len(), 3);
        assert_eq!(p.phonemes[0].vowel, Some(Vowel::A));
        assert_eq!(p.phonemes[2].vowel, None);
        let avg0 = (synthetic_coefficient(0, 0, 3) + synthetic_coefficient(0, 1, 3)) / 2.0;
        assert_eq!(p.phonemes[0].mfcc[3], avg0);
        let all: Vec<f32> = (0..3)
            .flat_map(|ph| (0..2).map(move |s| synthetic_coefficient(ph, s, 5)))
            .collect();
        let mean = all.iter().sum::<f32>() / 6.0;
        let sd = (all.iter().map(|v| (v - mean).powf(2.0)).sum::<f32>() / 6.0).sqrt();
        assert!((p.means[5] - mean).abs() < 1e-4 && (p.standard_deviations[5] - sd).abs() < 1e-4);
        let plain = profile(1, false);
        assert_eq!(plain.means, [0.0; MFCC_NUM]);
        assert_eq!(plain.standard_deviations, [1.0; MFCC_NUM]);
    }

    #[test]
    fn calibration_list_is_trimmed_to_newest_entries() {
        let json =
            synthetic_profile_json(1, false).replace("\"mfccDataCount\":2", "\"mfccDataCount\":1");
        let p = LipSyncProfile::from_bytes(json.as_bytes()).unwrap();
        assert_eq!(p.phonemes[1].mfcc[0], synthetic_coefficient(1, 1, 0));
    }

    #[test]
    fn binary_profile_round_trips() {
        let p = profile(2, true);
        let mut bin = b"WLIP".to_vec();
        bin.extend(16_000i32.to_be_bytes());
        bin.extend(1024i32.to_be_bytes());
        bin.extend([30, 2, p.phonemes.len() as u8, 2, 1]);
        let floats = |bin: &mut Vec<u8>, v: &[f32; MFCC_NUM]| {
            v.iter().for_each(|f| bin.extend(f.to_be_bytes()))
        };
        p.phonemes.iter().for_each(|ph| floats(&mut bin, &ph.mfcc));
        floats(&mut bin, &p.means);
        floats(&mut bin, &p.standard_deviations);
        for ph in &p.phonemes {
            bin.push(ph.name.len() as u8);
            bin.extend(ph.name.as_bytes());
        }
        assert_eq!(LipSyncProfile::from_bytes(&bin).unwrap(), *p);
        assert!(LipSyncProfile::from_bytes(&bin[..bin.len() - 1]).is_err());
        let mut trailing = bin.clone();
        trailing.push(0);
        assert!(LipSyncProfile::from_bytes(&trailing).is_err());
    }

    #[test]
    fn malformed_profiles_are_rejected() {
        let good = synthetic_profile_json(1, false);
        let cases = [
            ("not json".to_string(), "JSON"),
            (
                good.replace("\"compareMethod\":1", "\"compareMethod\":3"),
                "compareMethod",
            ),
            (
                good.replace("\"sampleCount\":1024", "\"sampleCount\":1000"),
                "sampleCount",
            ),
            (
                good.replace("\"sampleCount\":1024", "\"sampleCount\":1048576"),
                "sampleCount",
            ),
            (
                good.replace(
                    "\"melFilterBankChannels\":30",
                    "\"melFilterBankChannels\":12",
                ),
                "melFilterBankChannels",
            ),
            (
                good.replace("\"targetSampleRate\":16000", "\"targetSampleRate\":-1"),
                "targetSampleRate",
            ),
            (good.replace("\"mfccNum\":12", "\"mfccNum\":13"), "mfccNum"),
            (
                good.replace("\"mfccDataCount\":2", "\"mfccDataCount\":0"),
                "mfccDataCount",
            ),
            (
                good.replacen("{\"array\":[", "{\"array\":[1,", 1),
                "coefficients",
            ),
            (
                good.replacen("{\"array\":[", "{\"array\":[1e39,", 1),
                "coefficients",
            ),
            (
                good.replace("\"A\"", "\"X\"").replace("\"I\"", "\"Y\""),
                "no phoneme",
            ),
            (
                good.replace(
                    "\"mfccs\":[",
                    "\"mfccs\":[{\"name\":\"O\",\"mfccCalibrationDataList\":[]},",
                ),
                "no calibration",
            ),
            (r#"{"mfccs":[]}"#.to_string(), "phonemes"),
        ];
        for (json, needle) in cases {
            match LipSyncProfile::from_bytes(json.as_bytes()) {
                Err(LipSyncError::InvalidProfile(msg)) => {
                    assert!(msg.contains(needle), "{needle}: {msg}")
                }
                other => panic!("{needle}: expected rejection, got {other:?}"),
            }
        }
        // 1e39 overflows f32; it must be rejected rather than become inf.
        let overflow = good.replacen("{\"array\":[", "{\"array\":[1e39,", 1);
        let overflow = overflow.replacen(&format!("{},", synthetic_coefficient(0, 0, 0)), "", 1);
        assert!(LipSyncProfile::from_bytes(overflow.as_bytes()).is_err());
        let constant = good.replace(
            "\"useStandardization\":false",
            "\"useStandardization\":true",
        );
        let constant = (0..41).fold(constant, |s, k| {
            s.replace(&format!("[{}", (k as f32 - 20.0) * 2.5 + 90.0), "[90")
        });
        let _ = LipSyncProfile::from_bytes(constant.as_bytes());
        assert!(LipSyncProfile::from_bytes(&vec![b' '; MAX_PROFILE_BYTES + 1]).is_err());
    }

    #[test]
    fn caller_constructed_profiles_and_extreme_plans_are_revalidated() {
        let valid = (*profile(1, false)).clone();
        let mut invalids = Vec::new();
        let mut p = valid.clone();
        p.target_sample_rate = 0;
        invalids.push(p);
        let mut p = valid.clone();
        p.sample_count = 0;
        invalids.push(p);
        let mut p = valid.clone();
        p.sample_count = usize::MAX;
        invalids.push(p);
        let mut p = valid.clone();
        p.mel_filter_bank_channels = usize::MAX;
        invalids.push(p);
        let mut p = valid.clone();
        p.phonemes.clear();
        invalids.push(p);
        let mut p = valid.clone();
        p.standard_deviations[0] = 0.0;
        invalids.push(p);
        let mut p = valid.clone();
        p.phonemes[0].mfcc[0] = f32::NAN;
        invalids.push(p);
        for p in invalids {
            assert!(LipSyncAnalyzer::new(Arc::new(p), 48_000, LipSyncSettings::default()).is_err());
        }
        let mut oversized = valid.clone();
        oversized.sample_count = 8192;
        assert!(
            LipSyncAnalyzer::new(Arc::new(oversized), 384_000, LipSyncSettings::default()).is_err()
        );
        let mut a =
            LipSyncAnalyzer::new(Arc::new(valid), 48_000, LipSyncSettings::default()).unwrap();
        assert_eq!(
            a.push_mono(&[0.0, 2.0]),
            Err(LipSyncError::SampleOutOfRange { index: 1 })
        );
        assert_eq!(a.output().sample_position, 0);
        assert!(
            LipSyncSettings {
                hop_seconds: f32::MIN_POSITIVE,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn preallocated_analysis_buffers_remain_stable() {
        let mut a =
            LipSyncAnalyzer::new(profile(1, true), 48_000, LipSyncSettings::default()).unwrap();
        let before = [
            a.ring.as_ptr(),
            a.frame.as_ptr(),
            a.data.as_ptr(),
            a.re.as_ptr(),
            a.im.as_ptr(),
            a.spectrum.as_ptr(),
            a.mel.as_ptr(),
            a.dct.as_ptr(),
        ];
        a.push_mono(&signal(48_000, 48_000)).unwrap();
        a.reset();
        let after = [
            a.ring.as_ptr(),
            a.frame.as_ptr(),
            a.data.as_ptr(),
            a.re.as_ptr(),
            a.im.as_ptr(),
            a.spectrum.as_ptr(),
            a.mel.as_ptr(),
            a.dct.as_ptr(),
        ];
        assert_eq!(before, after);
    }
    /// Opt-in check against a real calibration profile, e.g. wLipSync's
    /// example/profile.json: `LIPSYNC_PROFILE=/path/profile.json cargo test lipsync`.
    #[test]
    fn real_profile_from_env_loads_and_runs() {
        let Ok(path) = std::env::var("LIPSYNC_PROFILE") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let p = Arc::new(LipSyncProfile::from_bytes(&bytes).unwrap());
        let binary_path = std::path::Path::new(&path).with_extension("bin");
        if let Ok(bytes) = std::fs::read(binary_path) {
            let binary = LipSyncProfile::from_bytes(&bytes).unwrap();
            assert_eq!(binary.target_sample_rate, p.target_sample_rate);
            assert_eq!(binary.phonemes.len(), p.phonemes.len());
            for (left, right) in p.phonemes.iter().zip(&binary.phonemes) {
                assert_eq!(left.name, right.name);
                assert_close(
                    &left.mfcc,
                    &right.mfcc,
                    1e-4,
                    "actual upstream binary calibration",
                );
            }
        }
        assert_eq!(p.vowel_coverage(), [true; 5]);
        let mut a = LipSyncAnalyzer::new(p, 48_000, LipSyncSettings::default()).unwrap();
        a.push_mono(&signal(48_000, 48_000)).unwrap();
        assert!(a.output().main_phoneme.is_some());
    }
}
