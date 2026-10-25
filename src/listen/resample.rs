//! Causal polyphase windowed-sinc rate conversion. Coefficients and history
//! are allocated once; the microphone callback does no allocation or trig.
const BASE_TAPS: usize = 128;
const PHASES: usize = 64;
pub(super) type Resampler = Polyphase<16_000, 4>;

pub(crate) struct Polyphase<const TARGET: u64, const MAX_OUTPUT: usize> {
    rate: u64,
    phase: u64,
    history: Vec<f32>,
    cursor: usize,
    coefficients: Vec<Vec<f32>>,
}
impl<const TARGET: u64, const MAX_OUTPUT: usize> Polyphase<TARGET, MAX_OUTPUT> {
    pub(crate) fn new(rate: u32) -> Self {
        assert!(
            (4000..=384_000).contains(&rate),
            "unsupported microphone rate"
        );
        Self::with_rate_bounds(rate, 4000, 384_000)
    }
    /// Playback speed changes the effective clock without changing the native
    /// microphone format contract enforced by `new`.
    pub(crate) fn for_playback(rate: u32) -> Self {
        Self::with_rate_bounds(rate, 2000, 768_000)
    }
    fn with_rate_bounds(rate: u32, minimum: u32, maximum: u32) -> Self {
        assert!((minimum..=maximum).contains(&rate));
        assert!((4000..=48_000).contains(&TARGET));
        assert!(MAX_OUTPUT >= TARGET.div_ceil(minimum as u64) as usize);
        let taps = BASE_TAPS * rate.div_ceil(48_000) as usize;
        let coefficients = if rate == TARGET as u32 {
            Vec::new()
        } else {
            // Retain 87.5% of target Nyquist, with a transition
            // band before Nyquist. At lower source rates protect their Nyquist.
            let cutoff = 0.4375 * (rate.min(TARGET as u32) as f64) / rate as f64;
            (0..PHASES)
                .map(|phase| {
                    let mut coefficients = vec![0.0; taps];
                    let center = (taps - 1) as f64 / 2.0 + phase as f64 / PHASES as f64;
                    let mut sum = 0.0;
                    for (tap, coefficient) in coefficients.iter_mut().enumerate() {
                        let offset = tap as f64 - center;
                        let x = 2.0 * std::f64::consts::PI * cutoff * offset;
                        let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                        let angle = 2.0 * std::f64::consts::PI * tap as f64 / (taps - 1) as f64;
                        let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
                        let value = 2.0 * cutoff * sinc * window;
                        *coefficient = value as f32;
                        sum += value;
                    }
                    for coefficient in &mut coefficients {
                        *coefficient /= sum as f32;
                    }
                    coefficients
                })
                .collect()
        };
        Self {
            rate: rate as u64,
            phase: 0,
            history: vec![0.0; taps],
            cursor: 0,
            coefficients,
        }
    }
    /// Conservative output count for a batch plus the causal filter tail.
    pub(crate) fn output_bound_with_tail(&self, input: usize) -> usize {
        ((input + self.tail_input_frames()) as u64 * TARGET).div_ceil(self.rate) as usize
    }
    /// Zero inputs needed to retain the causal filter tail at end of stream.
    pub(crate) fn tail_input_frames(&self) -> usize {
        if self.rate == TARGET {
            0
        } else {
            self.history.len() - 1
        }
    }
    /// Output capacity covers target/minimum-input-rate, checked at construction.
    pub(crate) fn push(&mut self, sample: f32) -> ([f32; MAX_OUTPUT], usize) {
        if self.rate == TARGET {
            let mut output = [0.0; MAX_OUTPUT];
            output[0] = sample;
            return (output, 1);
        }
        self.history[self.cursor] = sample;
        self.cursor += 1;
        if self.cursor == self.history.len() {
            self.cursor = 0;
        }
        self.phase += TARGET;
        let mut output = [0.0; MAX_OUTPUT];
        let mut count = 0;
        while self.phase >= self.rate {
            self.phase -= self.rate;
            // Remaining phase is the desired fractional delay behind this input.
            let phase = ((self.phase * PHASES as u64) / TARGET) as usize;
            let coefficients = &self.coefficients[phase];
            let value: f32 = self.history[..self.cursor]
                .iter()
                .rev()
                .chain(self.history[self.cursor..].iter().rev())
                .zip(coefficients)
                .map(|(sample, coefficient)| sample * coefficient)
                .sum();
            output[count] = value.clamp(-1.0, 1.0);
            count += 1;
        }
        (output, count)
    }
}
