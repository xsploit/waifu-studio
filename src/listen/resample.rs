//! Causal polyphase windowed-sinc conversion to 16 kHz. Coefficients and history
//! are allocated once; the microphone callback does no allocation or trig.
const BASE_TAPS: usize = 128;
const PHASES: usize = 64;
const TARGET: u64 = 16_000;

pub(super) struct Resampler {
    rate: u64,
    phase: u64,
    history: Vec<f32>,
    cursor: usize,
    coefficients: Vec<Vec<f32>>,
}
impl Resampler {
    pub(super) fn new(rate: u32) -> Self {
        assert!(
            (4000..=384_000).contains(&rate),
            "unsupported microphone rate"
        );
        let taps = BASE_TAPS * rate.div_ceil(48_000) as usize;
        let coefficients = if rate == TARGET as u32 {
            Vec::new()
        } else {
            // Retain speech below 7 kHz at the target rate, with a transition
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
    /// At most four outputs per input (the supported input minimum is 4 kHz).
    pub(super) fn push(&mut self, sample: f32) -> ([f32; 4], usize) {
        if self.rate == TARGET {
            return ([sample, 0.0, 0.0, 0.0], 1);
        }
        self.history[self.cursor] = sample;
        self.cursor += 1;
        if self.cursor == self.history.len() {
            self.cursor = 0;
        }
        self.phase += TARGET;
        let mut output = [0.0; 4];
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
