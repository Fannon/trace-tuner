use nih_plug::prelude::Enum;
use std::f32::consts::PI;

pub const MIDI_VELOCITY: f32 = 100.0 / 127.0;
pub const SILENCE_TIMEOUT_MS: f32 = 120.0;

pub(crate) const ACQUIRE_CONFIDENCE: f32 = 0.80;
pub(crate) const HOLD_CONFIDENCE: f32 = 0.60;
const STABLE_DISPLAY_HOLD_FRAMES: u8 = 96;
const FAST_DISPLAY_HOLD_FRAMES: u8 = 12;

const PRE_EMPHASIS_ALPHA: f32 = 0.30;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const GUITAR_STRING_NOTES: [u8; 6] = [40, 45, 50, 55, 59, 64];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TunerMode {
    #[id = "chromatic"]
    Chromatic,
    #[id = "guitar"]
    Guitar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ResponseMode {
    #[id = "stable"]
    Stable,
    #[id = "fast"]
    Fast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum DetectionAlgorithm {
    #[id = "yin"]
    #[name = "YIN"]
    Yin,
    #[id = "mpm"]
    #[name = "MPM"]
    Mpm,
    #[id = "acf"]
    #[name = "ACF"]
    Acf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    pub frequency_hz: f32,
    pub confidence: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteMatch {
    pub midi_note: u8,
    pub target_frequency_hz: f32,
    pub cents: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectionSnapshot {
    pub active: bool,
    pub frequency_hz: f32,
    pub confidence: f32,
    pub rms: f32,
    pub midi_note: u8,
    pub target_frequency_hz: f32,
    pub cents: f32,
}

impl DetectionSnapshot {
    pub const fn idle() -> Self {
        Self {
            active: false,
            frequency_hz: 0.0,
            confidence: 0.0,
            rms: 0.0,
            midi_note: 0,
            target_frequency_hz: 0.0,
            cents: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TuningColor {
    Green,
    Yellow,
    OrangeRed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiDecision {
    None,
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    NoteChange { off: u8, on: u8, velocity: f32 },
}

pub fn midi_note_name(midi_note: u8) -> String {
    let name = NOTE_NAMES[midi_note as usize % 12];
    let octave = midi_note as i16 / 12 - 1;
    format!("{name}{octave}")
}

pub fn midi_note_frequency(midi_note: u8, reference_pitch_hz: f32) -> f32 {
    reference_pitch_hz * 2.0_f32.powf((midi_note as f32 - 69.0) / 12.0)
}

pub fn chromatic_note_match(frequency_hz: f32, reference_pitch_hz: f32) -> Option<NoteMatch> {
    if frequency_hz <= 0.0 || reference_pitch_hz <= 0.0 {
        return None;
    }

    let note = (69.0 + 12.0 * (frequency_hz / reference_pitch_hz).log2()).round();
    if !(0.0..=127.0).contains(&note) {
        return None;
    }

    let midi_note = note as u8;
    let target_frequency_hz = midi_note_frequency(midi_note, reference_pitch_hz);
    Some(NoteMatch {
        midi_note,
        target_frequency_hz,
        cents: cents_between(frequency_hz, target_frequency_hz),
    })
}

pub fn guitar_note_match(frequency_hz: f32, reference_pitch_hz: f32) -> Option<NoteMatch> {
    if frequency_hz <= 0.0 || reference_pitch_hz <= 0.0 {
        return None;
    }

    GUITAR_STRING_NOTES
        .iter()
        .copied()
        .map(|midi_note| {
            let target_frequency_hz = midi_note_frequency(midi_note, reference_pitch_hz);
            NoteMatch {
                midi_note,
                target_frequency_hz,
                cents: cents_between(frequency_hz, target_frequency_hz),
            }
        })
        .min_by(|a, b| a.cents.abs().total_cmp(&b.cents.abs()))
}

pub fn map_frequency(
    frequency_hz: f32,
    reference_pitch_hz: f32,
    mode: TunerMode,
) -> Option<NoteMatch> {
    match mode {
        TunerMode::Chromatic => chromatic_note_match(frequency_hz, reference_pitch_hz),
        TunerMode::Guitar => guitar_note_match(frequency_hz, reference_pitch_hz),
    }
}

pub fn cents_between(frequency_hz: f32, target_frequency_hz: f32) -> f32 {
    if target_frequency_hz <= 0.0 || frequency_hz <= 0.0 {
        return 0.0;
    }
    1200.0 * (frequency_hz / target_frequency_hz).log2()
}

pub fn tuning_color(cents: f32) -> TuningColor {
    match cents.abs() {
        cents if cents <= 5.0 => TuningColor::Green,
        cents if cents <= 15.0 => TuningColor::Yellow,
        _ => TuningColor::OrangeRed,
    }
}

// ---------------------------------------------------------------------------
// PitchDetector – dispatch to YIN / MPM / ACF with pre-emphasis
// ---------------------------------------------------------------------------

pub struct PitchDetector {
    algorithm: DetectionAlgorithm,
    sample_rate: f32,
    min_frequency_hz: f32,
    max_frequency_hz: f32,
    threshold: f32,
    window_size: usize,

    // --- shared ---
    pre_emphasized: Vec<f32>,
    prev_input: f32,

    // --- YIN ---
    yin_difference: Vec<f32>,
    yin_cumulative_mean: Vec<f32>,

    // --- MPM ---
    mpm_acf: Vec<f32>,
    mpm_nsdf: Vec<f32>,

    // --- ACF ---
    acf_windowed: Vec<f32>,
    hann_window: Vec<f32>,
}

impl PitchDetector {
    pub fn new(sample_rate: f32, max_window_samples: usize) -> Self {
        let tau_len = max_window_samples / 2 + 1;
        Self {
            algorithm: DetectionAlgorithm::Yin,
            sample_rate,
            min_frequency_hz: 70.0,
            max_frequency_hz: 1_200.0,
            threshold: 0.14,
            window_size: max_window_samples,
            pre_emphasized: vec![0.0; max_window_samples],
            prev_input: 0.0,
            yin_difference: vec![0.0; tau_len],
            yin_cumulative_mean: vec![0.0; tau_len],
            mpm_acf: vec![0.0; tau_len],
            mpm_nsdf: vec![0.0; tau_len],
            acf_windowed: vec![0.0; max_window_samples],
            hann_window: Self::make_hann_window(max_window_samples),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32, max_window_samples: usize) {
        self.sample_rate = sample_rate;
        self.window_size = max_window_samples;
        let tau_len = max_window_samples / 2 + 1;
        self.yin_difference.resize(tau_len, 0.0);
        self.yin_cumulative_mean.resize(tau_len, 0.0);
        self.mpm_acf.resize(tau_len, 0.0);
        self.mpm_nsdf.resize(tau_len, 0.0);
        self.pre_emphasized.resize(max_window_samples, 0.0);
        self.acf_windowed.resize(max_window_samples, 0.0);
        self.hann_window.resize(max_window_samples, 0.0);
        self.hann_window = Self::make_hann_window(max_window_samples);
        self.prev_input = 0.0;
    }

    pub fn reset(&mut self) {
        self.prev_input = 0.0;
        self.pre_emphasized.fill(0.0);
        self.acf_windowed.fill(0.0);
        self.yin_difference.fill(0.0);
        self.yin_cumulative_mean.fill(0.0);
        self.mpm_acf.fill(0.0);
        self.mpm_nsdf.fill(0.0);
    }

    fn make_hann_window(n: usize) -> Vec<f32> {
        if n <= 1 {
            return vec![1.0; n];
        }
        (0..n)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (n - 1) as f32).cos()))
            .collect()
    }

    pub fn set_algorithm(&mut self, algorithm: DetectionAlgorithm) {
        self.algorithm = algorithm;
    }

    pub fn algorithm(&self) -> DetectionAlgorithm {
        self.algorithm
    }

    // ------------------------------------------------------------------
    // Public entry point – applies pre-emphasis then dispatches
    // ------------------------------------------------------------------
    pub fn detect(&mut self, samples: &[f32]) -> Option<PitchEstimate> {
        if samples.len() < 32 || self.sample_rate <= 0.0 {
            return None;
        }
        if samples.len() > self.pre_emphasized.len() {
            return None;
        }

        let (rms, n) = self.apply_pre_emphasis(samples);
        if rms < 0.01 {
            return None;
        }

        match self.algorithm {
            DetectionAlgorithm::Yin => self.detect_yin(rms, n),
            DetectionAlgorithm::Mpm => self.detect_mpm(rms, n),
            DetectionAlgorithm::Acf => self.detect_acf(rms, n),
        }
    }

    // ------------------------------------------------------------------
    // Pre-emphasis: y[n] = x[n] - alpha * x[n-1]   (first-order HPF)
    // Returns (RMS of the filtered signal, number of samples processed).
    // ------------------------------------------------------------------
    fn apply_pre_emphasis(&mut self, samples: &[f32]) -> (f32, usize) {
        let mut prev = self.prev_input;
        let mut energy = 0.0;
        for (i, sample) in samples.iter().enumerate() {
            let filtered = sample - PRE_EMPHASIS_ALPHA * prev;
            prev = *sample;
            self.pre_emphasized[i] = filtered;
            energy += filtered * filtered;
        }
        self.prev_input = prev;
        ((energy / samples.len() as f32).sqrt(), samples.len())
    }

    // ------------------------------------------------------------------
    // Tau bounds shared by YIN and MPM
    // ------------------------------------------------------------------
    fn tau_range(&self, window_len: usize) -> Option<(usize, usize)> {
        let max_tau = ((self.sample_rate / self.min_frequency_hz) as usize)
            .min(window_len.saturating_sub(2))
            .min(self.yin_difference.len().saturating_sub(1));
        let min_tau = ((self.sample_rate / self.max_frequency_hz) as usize).max(2);
        if max_tau <= min_tau + 2 {
            None
        } else {
            Some((min_tau, max_tau))
        }
    }

    // ==================================================================
    // YIN  (Yet another INtegrator)
    //
    // Squared difference function with cumulative-mean normalisation.
    // Finds the first dip below a threshold in the normalised curve.
    // Confidence = 1 - cmndf[tau].
    // Fast, well-tested, good for clean monophonic signals.
    // Weakness: amplitude changes distort the difference function;
    // confidence is threshold-dependent.
    // ==================================================================
    fn detect_yin(&mut self, rms: f32, n: usize) -> Option<PitchEstimate> {
        let (min_tau, max_tau) = self.tau_range(n)?;

        self.yin_difference[0] = 0.0;
        for tau in 1..=max_tau {
            let window_limit = n - tau;
            let a = &self.pre_emphasized[..window_limit];
            let b = &self.pre_emphasized[tau..tau + window_limit];
            let mut sum = 0.0;
            for i in 0..window_limit {
                let delta = a[i] - b[i];
                sum += delta * delta;
            }
            self.yin_difference[tau] = sum;
        }

        self.yin_cumulative_mean[0] = 1.0;
        let mut running_sum = 0.0;
        for tau in 1..=max_tau {
            running_sum += self.yin_difference[tau];
            self.yin_cumulative_mean[tau] = if running_sum > 0.0 {
                self.yin_difference[tau] * tau as f32 / running_sum
            } else {
                1.0
            };
        }

        let mut tau = min_tau;
        while tau <= max_tau {
            if self.yin_cumulative_mean[tau] < self.threshold {
                while tau < max_tau
                    && self.yin_cumulative_mean[tau + 1] < self.yin_cumulative_mean[tau]
                {
                    tau += 1;
                }
                let better_tau = parabolic_interpolation(&self.yin_cumulative_mean, tau, max_tau);
                let frequency_hz = self.sample_rate / better_tau;
                if frequency_hz < self.min_frequency_hz || frequency_hz > self.max_frequency_hz {
                    tau += 1;
                    continue;
                }
                let confidence = (1.0 - self.yin_cumulative_mean[tau]).clamp(0.0, 1.0);
                return Some(PitchEstimate {
                    frequency_hz,
                    confidence,
                    rms,
                });
            }
            tau += 1;
        }

        None
    }

    // ==================================================================
    // MPM  (McLeod Pitch Method)
    //
    // Uses the Normalised Squared Difference Function:
    //   nsdf[τ] = 2·r[τ] / (r[0] + r_back[τ])
    // where r[τ] is the autocorrelation of the windowed signal.
    // This produces a naturally normalised 0-1 confidence at the peak.
    //
    // Benefits vs YIN:
    //   • Clipping-resistant – the normalisation compensates for
    //     amplitude variation across the window.
    //   • Confidence is the peak height itself – more intuitive.
    //   • No cumulative-mean trick needed.
    //   • Parabolic interpolation on the nsdf peak refines pitch.
    //   • Uses first-peak-above-threshold search to avoid octave errors
    //     on clean periodic signals where subharmonic NSDF peaks can
    //     equal the fundamental peak.
    // ==================================================================
    fn detect_mpm(&mut self, rms: f32, n: usize) -> Option<PitchEstimate> {
        let (min_tau, max_tau) = self.tau_range(n)?;

        self.mpm_acf[0] = 0.0;
        for i in 0..n {
            self.mpm_acf[0] += self.pre_emphasized[i] * self.pre_emphasized[i];
        }
        if self.mpm_acf[0] < 1e-10 {
            return None;
        }

        for tau in 1..=max_tau {
            let window_limit = n - tau;
            let a = &self.pre_emphasized[..window_limit];
            let b = &self.pre_emphasized[tau..tau + window_limit];
            let mut r = 0.0;
            let mut r_front = 0.0;
            let mut r_back = 0.0;
            for i in 0..window_limit {
                r += a[i] * b[i];
                r_front += a[i] * a[i];
                r_back += b[i] * b[i];
            }
            self.mpm_acf[tau] = r;
            self.mpm_nsdf[tau] = if r_front > 1e-10 && r_back > 1e-10 {
                (2.0 * r / (r_front + r_back)).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }

        // Find the first local peak above threshold in the valid tau range.
        // First-peak avoids octave errors where subharmonic NSDF peaks can
        // equal the fundamental peak on clean periodic signals.
        let mpm_threshold = 0.50;
        for tau in min_tau + 1..max_tau {
            if self.mpm_nsdf[tau] > mpm_threshold
                && self.mpm_nsdf[tau] > self.mpm_nsdf[tau - 1]
                && self.mpm_nsdf[tau] >= self.mpm_nsdf[tau + 1]
            {
                let better_tau = parabolic_interpolation(&self.mpm_nsdf, tau, max_tau);
                let frequency_hz = self.sample_rate / better_tau;
                if frequency_hz < self.min_frequency_hz || frequency_hz > self.max_frequency_hz {
                    continue;
                }
                let confidence = self.mpm_nsdf[tau].clamp(0.0, 1.0);
                return Some(PitchEstimate {
                    frequency_hz,
                    confidence,
                    rms,
                });
            }
        }

        None
    }

    // ==================================================================
    // ACF  (raw autocorrelation with Hann window)
    //
    // 1.  Apply Hann window to pre-emphasised samples.
    // 2.  Compute raw autocorrelation: r[τ] = Σ x[i] · x[i+τ].
    // 3.  Normalise to r[0] for a 0-1 confidence baseline.
    // 4.  Find the *first* local peak above a threshold.
    // 5.  Parabolic interpolation refines the peak.
    //
    // Benefits vs YIN / MPM:
    //   • Uses raw autocorrelation (no difference/normalisation tricks).
    //   • Simplest possible time-domain method — different failure modes.
    //   • The first-peak rule avoids sub-harmonic (octave) errors
    //     inherent in global-maximum searches.
    //   • Hann window reduces spectral leakage, improving peak
    //     separation for closely-spaced harmonics.
    // ==================================================================
    fn detect_acf(&mut self, rms: f32, n: usize) -> Option<PitchEstimate> {
        let (min_tau, max_tau) = self.tau_range(n)?;

        // Hann window the pre-emphasised samples
        for i in 0..n {
            self.acf_windowed[i] = self.pre_emphasized[i] * self.hann_window[i];
        }

        // Raw autocorrelation
        self.mpm_acf[0] = 0.0;
        for i in 0..n {
            self.mpm_acf[0] += self.acf_windowed[i] * self.acf_windowed[i];
        }
        let r0 = self.mpm_acf[0];
        if r0 < 1e-10 {
            return None;
        }

        for tau in 1..=max_tau {
            let window_limit = n - tau;
            let mut r = 0.0;
            for i in 0..window_limit {
                r += self.acf_windowed[i] * self.acf_windowed[i + tau];
            }
            self.mpm_acf[tau] = r;
        }

        // Find first local peak above threshold in normalised ACF
        let threshold = 0.25;
        for tau in min_tau + 1..max_tau {
            let norm = self.mpm_acf[tau] / r0;
            if norm > threshold
                && self.mpm_acf[tau] > self.mpm_acf[tau - 1]
                && self.mpm_acf[tau] > self.mpm_acf[tau + 1]
            {
                let better_tau = parabolic_interpolation(&self.mpm_acf, tau, max_tau);
                let frequency_hz = self.sample_rate / better_tau;
                if frequency_hz < self.min_frequency_hz || frequency_hz > self.max_frequency_hz {
                    continue;
                }
                let confidence = norm.clamp(0.0, 1.0);
                return Some(PitchEstimate {
                    frequency_hz,
                    confidence,
                    rms,
                });
            }
        }

        None
    }
}

// =====================================================================
// Response smoother  (Stable / Fast display hold)
// =====================================================================

pub struct ResponseSmoother {
    mode: ResponseMode,
    current: DetectionSnapshot,
    candidate_note: Option<u8>,
    candidate_count: u8,
    missing_count: u8,
}

impl ResponseSmoother {
    pub const fn new(mode: ResponseMode) -> Self {
        Self {
            mode,
            current: DetectionSnapshot::idle(),
            candidate_note: None,
            candidate_count: 0,
            missing_count: 0,
        }
    }

    pub fn set_mode(&mut self, mode: ResponseMode) {
        if self.mode != mode {
            self.mode = mode;
            self.candidate_note = None;
            self.candidate_count = 0;
            self.missing_count = 0;
        }
    }

    pub fn reset(&mut self) {
        self.current = DetectionSnapshot::idle();
        self.candidate_note = None;
        self.candidate_count = 0;
        self.missing_count = 0;
    }

    pub fn update(&mut self, next: Option<DetectionSnapshot>) -> DetectionSnapshot {
        let Some(next) = next else {
            return self.update_missing();
        };

        if next.confidence < self.required_confidence(next.midi_note) {
            return self.update_missing();
        }

        let required = match self.mode {
            ResponseMode::Stable => 3,
            ResponseMode::Fast => 1,
        };

        if !self.current.active || next.midi_note == self.current.midi_note {
            self.accept(next);
            return self.current;
        }

        if self.candidate_note == Some(next.midi_note) {
            self.candidate_count = self.candidate_count.saturating_add(1);
        } else {
            self.candidate_note = Some(next.midi_note);
            self.candidate_count = 1;
        }

        if self.candidate_count >= required {
            self.accept(next);
        }

        self.current
    }

    fn accept(&mut self, next: DetectionSnapshot) {
        let weight = match self.mode {
            ResponseMode::Stable => 0.25,
            ResponseMode::Fast => 0.65,
        };

        if self.current.active && self.current.midi_note == next.midi_note {
            self.current.frequency_hz =
                self.current.frequency_hz * (1.0 - weight) + next.frequency_hz * weight;
            self.current.cents = self.current.cents * (1.0 - weight) + next.cents * weight;
            self.current.confidence =
                self.current.confidence * (1.0 - weight) + next.confidence * weight;
            self.current.rms = self.current.rms * (1.0 - weight) + next.rms * weight;
            self.current.target_frequency_hz = next.target_frequency_hz;
        } else {
            self.current = next;
        }

        self.candidate_note = None;
        self.candidate_count = 0;
        self.missing_count = 0;
    }

    fn required_confidence(&self, midi_note: u8) -> f32 {
        if self.mode == ResponseMode::Stable
            && self.current.active
            && self.current.midi_note == midi_note
        {
            HOLD_CONFIDENCE
        } else {
            ACQUIRE_CONFIDENCE
        }
    }

    fn update_missing(&mut self) -> DetectionSnapshot {
        self.candidate_note = None;
        self.candidate_count = 0;

        if self.current.active {
            self.missing_count = self.missing_count.saturating_add(1);
            let hold_frames = match self.mode {
                ResponseMode::Stable => STABLE_DISPLAY_HOLD_FRAMES,
                ResponseMode::Fast => FAST_DISPLAY_HOLD_FRAMES,
            };
            if self.missing_count < hold_frames {
                let held_progress = self.missing_count as f32 / hold_frames as f32;
                self.current.confidence = HOLD_CONFIDENCE * (1.0 - held_progress);
                return self.current;
            }
        }

        self.current = DetectionSnapshot::idle();
        self.missing_count = 0;
        self.current
    }
}

// =====================================================================
// MIDI state machine
// =====================================================================

pub struct MidiState {
    active_note: Option<u8>,
    silence_samples: u32,
    silence_timeout_samples: u32,
    candidate_note: Option<u8>,
    candidate_count: u8,
}

impl MidiState {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            active_note: None,
            silence_samples: 0,
            silence_timeout_samples: silence_timeout_samples(sample_rate),
            candidate_note: None,
            candidate_count: 0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.silence_timeout_samples = silence_timeout_samples(sample_rate);
    }

    pub fn reset(&mut self) {
        self.active_note = None;
        self.silence_samples = 0;
        self.candidate_note = None;
        self.candidate_count = 0;
    }

    pub fn update(
        &mut self,
        detection: Option<DetectionSnapshot>,
        mode: ResponseMode,
        elapsed_samples: u32,
    ) -> MidiDecision {
        let required = match mode {
            ResponseMode::Stable => 3,
            ResponseMode::Fast => 1,
        };

        let Some(detection) = detection.filter(|d| d.active) else {
            self.candidate_note = None;
            self.candidate_count = 0;
            if self.active_note.is_some() {
                self.silence_samples = self.silence_samples.saturating_add(elapsed_samples);
            }
            if self.silence_samples >= self.silence_timeout_samples {
                self.silence_samples = 0;
                return self
                    .active_note
                    .take()
                    .map_or(MidiDecision::None, |note| MidiDecision::NoteOff { note });
            }
            return MidiDecision::None;
        };

        self.silence_samples = 0;
        let note = detection.midi_note;
        if self.active_note == Some(note) {
            self.candidate_note = None;
            self.candidate_count = 0;
            return MidiDecision::None;
        }

        if self.candidate_note == Some(note) {
            self.candidate_count = self.candidate_count.saturating_add(1);
        } else {
            self.candidate_note = Some(note);
            self.candidate_count = 1;
        }

        if self.candidate_count < required {
            return MidiDecision::None;
        }

        self.candidate_note = None;
        self.candidate_count = 0;
        match self.active_note.replace(note) {
            Some(previous) => MidiDecision::NoteChange {
                off: previous,
                on: note,
                velocity: MIDI_VELOCITY,
            },
            None => MidiDecision::NoteOn {
                note,
                velocity: MIDI_VELOCITY,
            },
        }
    }
}

// =====================================================================
// Utilities
// =====================================================================

fn silence_timeout_samples(sample_rate: f32) -> u32 {
    (sample_rate * SILENCE_TIMEOUT_MS / 1_000.0)
        .round()
        .max(1.0) as u32
}

fn parabolic_interpolation(values: &[f32], tau: usize, max_tau: usize) -> f32 {
    if tau == 0 || tau >= max_tau {
        return tau as f32;
    }

    let left = values[tau - 1];
    let center = values[tau];
    let right = values[tau + 1];
    let denominator = left - 2.0 * center + right;
    if denominator.abs() < f32::EPSILON {
        tau as f32
    } else {
        let offset = 0.5 * (left - right) / denominator;
        // Clamp to [-1, 1] to avoid wild extrapolation on very flat peaks.
        let offset = offset.clamp(-1.0, 1.0);
        tau as f32 + offset
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn active_snapshot(midi_note: u8, cents: f32) -> DetectionSnapshot {
        active_snapshot_with_confidence(midi_note, cents, 0.9)
    }

    fn active_snapshot_with_confidence(
        midi_note: u8,
        cents: f32,
        confidence: f32,
    ) -> DetectionSnapshot {
        DetectionSnapshot {
            active: true,
            frequency_hz: midi_note_frequency(midi_note, 440.0),
            confidence,
            rms: 0.2,
            midi_note,
            target_frequency_hz: midi_note_frequency(midi_note, 440.0),
            cents,
        }
    }

    fn sine_wave(freq_hz: f32, sample_rate: f32, length: usize) -> Vec<f32> {
        (0..length)
            .map(|i| (TAU * freq_hz * i as f32 / sample_rate).sin() * 0.4)
            .collect()
    }

    fn detect_freq(alg: DetectionAlgorithm, freq_hz: f32, sample_rate: f32) -> f32 {
        let mut detector = PitchDetector::new(sample_rate, 2_048);
        detector.set_algorithm(alg);
        let samples = sine_wave(freq_hz, sample_rate, 2_048);
        detector.detect(&samples).unwrap().frequency_hz
    }

    // --- note mapping ---

    #[test]
    fn maps_a4_to_zero_cents() {
        let matched = chromatic_note_match(440.0, 440.0).unwrap();
        assert_eq!(matched.midi_note, 69);
        assert_eq!(midi_note_name(matched.midi_note), "A4");
        assert!(matched.cents.abs() < 0.01);
    }

    #[test]
    fn maps_e5_reference_check() {
        let matched = chromatic_note_match(659.255, 440.0).unwrap();
        assert_eq!(matched.midi_note, 76);
        assert_eq!(midi_note_name(matched.midi_note), "E5");
        assert!(matched.cents.abs() < 0.01);

        let slightly_flat = chromatic_note_match(658.7, 440.0).unwrap();
        assert_eq!(slightly_flat.midi_note, 76);
        assert!((slightly_flat.cents + 1.46).abs() < 0.1);
    }

    #[test]
    fn maps_guitar_mode_to_nearest_string() {
        let matched = guitar_note_match(111.0, 440.0).unwrap();
        assert_eq!(matched.midi_note, 45);
        assert_eq!(midi_note_name(matched.midi_note), "A2");
    }

    #[test]
    fn color_thresholds_follow_deviation() {
        assert_eq!(tuning_color(4.9), TuningColor::Green);
        assert_eq!(tuning_color(-10.0), TuningColor::Yellow);
        assert_eq!(tuning_color(23.0), TuningColor::OrangeRed);
    }

    // --- pitch detectors ---

    #[test]
    fn yin_detects_440hz() {
        let freq = detect_freq(DetectionAlgorithm::Yin, 440.0, 48_000.0);
        assert!((freq - 440.0).abs() < 1.0);
    }

    #[test]
    fn mpm_detects_440hz() {
        let freq = detect_freq(DetectionAlgorithm::Mpm, 440.0, 48_000.0);
        assert!((freq - 440.0).abs() < 2.0);
    }

    #[test]
    fn acf_detects_440hz() {
        let freq = detect_freq(DetectionAlgorithm::Acf, 440.0, 48_000.0);
        assert!((freq - 440.0).abs() < 3.0, "acf detected: {freq}");
    }

    #[test]
    fn acf_detects_high_frequency() {
        let freq = detect_freq(DetectionAlgorithm::Acf, 1_000.0, 48_000.0);
        assert!((freq - 1_000.0).abs() < 6.0);
    }

    #[test]
    fn acf_confidence_on_clean_sine() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Acf);
        let samples = sine_wave(440.0, 48_000.0, 2_048);
        let pitch = detector.detect(&samples).unwrap();
        assert!(pitch.confidence > 0.7);
    }

    #[test]
    fn yin_detects_high_frequency() {
        let freq = detect_freq(DetectionAlgorithm::Yin, 1_000.0, 48_000.0);
        assert!((freq - 1_000.0).abs() < 5.0);
    }

    #[test]
    fn mpm_detects_high_frequency() {
        let freq = detect_freq(DetectionAlgorithm::Mpm, 1_000.0, 48_000.0);
        assert!((freq - 1_000.0).abs() < 6.0, "detected: {freq}");
    }

    #[test]
    fn yin_confidence_on_clean_sine() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Yin);
        let samples = sine_wave(440.0, 48_000.0, 2_048);
        let pitch = detector.detect(&samples).unwrap();
        assert!(pitch.confidence > 0.8);
    }

    #[test]
    fn mpm_confidence_on_clean_sine() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Mpm);
        let samples = sine_wave(440.0, 48_000.0, 2_048);
        let pitch = detector.detect(&samples).unwrap();
        assert!(pitch.confidence > 0.8);
    }

    #[test]
    fn yin_set_sample_rate_reconfigures_buffers() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_sample_rate(44_100.0, 1_024);
        let samples = vec![0.0; 1_024];
        assert!(detector.detect(&samples).is_none());
    }

    #[test]
    fn mpm_set_sample_rate_reconfigures_buffers() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Mpm);
        detector.set_sample_rate(44_100.0, 1_024);
        let samples = vec![0.0; 1_024];
        assert!(detector.detect(&samples).is_none());
    }

    #[test]
    fn acf_set_sample_rate_reconfigures_buffers() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Acf);
        detector.set_sample_rate(44_100.0, 1_024);
        let samples = vec![0.0; 1_024];
        assert!(detector.detect(&samples).is_none());
    }

    #[test]
    fn all_algorithms_reject_short_buffer() {
        let algs = [
            DetectionAlgorithm::Yin,
            DetectionAlgorithm::Mpm,
            DetectionAlgorithm::Acf,
        ];
        for alg in algs {
            let mut detector = PitchDetector::new(48_000.0, 2_048);
            detector.set_algorithm(alg);
            assert!(detector.detect(&[0.5; 16]).is_none());
        }
    }

    #[test]
    fn all_algorithms_reject_oversized_buffer() {
        let mut detector = PitchDetector::new(48_000.0, 512);
        let samples = vec![0.0; 1_024];
        assert!(detector.detect(&samples).is_none());
    }

    #[test]
    fn algorithm_switching_at_runtime() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        let samples = sine_wave(440.0, 48_000.0, 2_048);

        detector.set_algorithm(DetectionAlgorithm::Yin);
        let yin = detector.detect(&samples).unwrap();

        detector.set_algorithm(DetectionAlgorithm::Mpm);
        let mpm = detector.detect(&samples).unwrap();

        detector.set_algorithm(DetectionAlgorithm::Acf);
        let acf = detector.detect(&samples).unwrap();

        assert!((yin.frequency_hz - 440.0).abs() < 1.0);
        assert!((mpm.frequency_hz - 440.0).abs() < 2.0);
        assert!((acf.frequency_hz - 440.0).abs() < 3.0);
    }

    #[test]
    fn detector_reset_clears_pre_emphasis_state() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Yin);
        let samples = sine_wave(440.0, 48_000.0, 2_048);
        let _ = detector.detect(&samples);

        detector.reset();
        // After reset, silence should return None
        assert!(detector.detect(&[0.0; 2_048]).is_none());
        // And prev_input should be 0, so a new sine should still detect correctly
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 1.0);
    }

    #[test]
    fn pre_emphasis_state_carried_across_calls() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Yin);

        let samples1 = sine_wave(440.0, 48_000.0, 1_024);
        let samples2: Vec<f32> = (0..1_024)
            .map(|i| (TAU * 440.0 * (i + 1_024) as f32 / 48_000.0).sin() * 0.4)
            .collect();

        let _ = detector.detect(&samples1);
        let pitch = detector.detect(&samples2).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 1.0);
    }

    #[test]
    fn yin_detects_low_e_string() {
        let freq = detect_freq(DetectionAlgorithm::Yin, 82.41, 48_000.0);
        assert!((freq - 82.41).abs() < 2.0);
    }

    #[test]
    fn mpm_detects_low_e_string_with_larger_window() {
        // MPM needs ~4+ periods in the window for reliable low-frequency detection.
        // With 2048 samples @ 48kHz (~3.5 periods for 82Hz) the NSDF peak can be
        // swamped by shorter-lag correlation. 4096 samples gives ~4.7 periods
        // and restores the fundamental peak.
        let mut detector = PitchDetector::new(48_000.0, 4_096);
        detector.set_algorithm(DetectionAlgorithm::Mpm);
        let samples = sine_wave(82.41, 48_000.0, 4_096);
        let freq = detector.detect(&samples).unwrap().frequency_hz;
        assert!((freq - 82.41).abs() < 3.0);
    }

    #[test]
    fn acf_detects_low_e_string() {
        let freq = detect_freq(DetectionAlgorithm::Acf, 82.41, 48_000.0);
        assert!((freq - 82.41).abs() < 4.0, "acf low E: {freq}");
    }

    fn sawtooth_wave(freq_hz: f32, sample_rate: f32, length: usize) -> Vec<f32> {
        let period = sample_rate / freq_hz;
        (0..length)
            .map(|i| {
                let phase = (i as f32 / period) % 1.0;
                (phase * 2.0 - 1.0) * 0.4
            })
            .collect()
    }

    fn noisy_sine_wave(
        freq_hz: f32,
        sample_rate: f32,
        length: usize,
        noise_level: f32,
    ) -> Vec<f32> {
        let mut samples = sine_wave(freq_hz, sample_rate, length);
        for (i, sample) in samples.iter_mut().enumerate() {
            // Simple LCG pseudo-random noise, deterministic across runs
            let noise =
                ((i.wrapping_mul(1103515245).wrapping_add(12345) & 0x7fffffff) as f32
                    / 0x7fffffff as f32)
                    * 2.0
                    - 1.0;
            *sample += noise * noise_level;
        }
        samples
    }

    #[test]
    fn yin_detects_sawtooth_440hz() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Yin);
        let samples = sawtooth_wave(440.0, 48_000.0, 2_048);
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 2.0);
    }

    #[test]
    fn mpm_detects_sawtooth_440hz() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Mpm);
        let samples = sawtooth_wave(440.0, 48_000.0, 2_048);
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 2.0);
    }

    #[test]
    fn acf_detects_sawtooth_440hz() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Acf);
        let samples = sawtooth_wave(440.0, 48_000.0, 2_048);
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 3.0, "acf sawtooth: {freq}", freq = pitch.frequency_hz);
    }

    #[test]
    fn yin_detects_noisy_sine() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Yin);
        let samples = noisy_sine_wave(440.0, 48_000.0, 2_048, 0.01);
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 2.0);
    }

    #[test]
    fn mpm_detects_noisy_sine() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Mpm);
        let samples = noisy_sine_wave(440.0, 48_000.0, 2_048, 0.01);
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 3.0);
    }

    #[test]
    fn acf_detects_noisy_sine() {
        let mut detector = PitchDetector::new(48_000.0, 2_048);
        detector.set_algorithm(DetectionAlgorithm::Acf);
        let samples = noisy_sine_wave(440.0, 48_000.0, 2_048, 0.01);
        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 4.0, "acf noisy: {freq}", freq = pitch.frequency_hz);
    }

    // --- response smoother ---

    #[test]
    fn fast_changes_note_faster_than_stable() {
        let mut stable = ResponseSmoother::new(ResponseMode::Stable);
        let mut fast = ResponseSmoother::new(ResponseMode::Fast);

        stable.update(Some(active_snapshot(69, 0.0)));
        fast.update(Some(active_snapshot(69, 0.0)));

        let stable_after_one = stable.update(Some(active_snapshot(71, 0.0)));
        let fast_after_one = fast.update(Some(active_snapshot(71, 0.0)));

        assert_eq!(stable_after_one.midi_note, 69);
        assert_eq!(fast_after_one.midi_note, 71);

        stable.update(Some(active_snapshot(71, 0.0)));
        let stable_after_three = stable.update(Some(active_snapshot(71, 0.0)));
        assert_eq!(stable_after_three.midi_note, 71);
    }

    #[test]
    fn stable_smoothing_reduces_jitter_more_than_fast() {
        let mut stable = ResponseSmoother::new(ResponseMode::Stable);
        let mut fast = ResponseSmoother::new(ResponseMode::Fast);

        stable.update(Some(active_snapshot(69, 0.0)));
        fast.update(Some(active_snapshot(69, 0.0)));

        let stable_update = stable.update(Some(active_snapshot(69, 20.0)));
        let fast_update = fast.update(Some(active_snapshot(69, 20.0)));

        assert!(stable_update.cents < fast_update.cents);
        assert!((stable_update.cents - 5.0).abs() < 0.01);
        assert!((fast_update.cents - 13.0).abs() < 0.01);
    }

    #[test]
    fn stable_display_holds_through_brief_missing_detection() {
        let mut stable = ResponseSmoother::new(ResponseMode::Stable);
        stable.update(Some(active_snapshot(69, 0.0)));

        for _ in 0..(STABLE_DISPLAY_HOLD_FRAMES - 1) {
            let held = stable.update(None);
            assert!(held.active);
            assert_eq!(held.midi_note, 69);
        }

        assert!(!stable.update(None).active);
    }

    #[test]
    fn held_display_fades_confidence_while_note_rings_out() {
        let mut stable = ResponseSmoother::new(ResponseMode::Stable);
        stable.update(Some(active_snapshot(69, 0.0)));

        let held = stable.update(None);

        assert!(held.active);
        assert_eq!(held.midi_note, 69);
        assert!(held.confidence < HOLD_CONFIDENCE);
    }

    #[test]
    fn stable_display_keeps_lower_confidence_same_note() {
        let mut stable = ResponseSmoother::new(ResponseMode::Stable);
        stable.update(Some(active_snapshot(69, 0.0)));

        let updated = stable.update(Some(active_snapshot_with_confidence(69, 8.0, 0.65)));

        assert!(updated.active);
        assert_eq!(updated.midi_note, 69);
        assert!(updated.cents > 0.0);
    }

    // --- midi ---

    #[test]
    fn midi_state_tracks_one_active_note() {
        let mut midi = MidiState::new(48_000.0);
        let a4 = active_snapshot(69, 0.0);
        let b4 = active_snapshot(71, 0.0);

        assert_eq!(
            midi.update(Some(a4), ResponseMode::Fast, 128),
            MidiDecision::NoteOn {
                note: 69,
                velocity: MIDI_VELOCITY
            }
        );
        assert_eq!(
            midi.update(Some(b4), ResponseMode::Fast, 128),
            MidiDecision::NoteChange {
                off: 69,
                on: 71,
                velocity: MIDI_VELOCITY
            }
        );
        assert_eq!(
            midi.update(None, ResponseMode::Fast, 5_760),
            MidiDecision::NoteOff { note: 71 }
        );
    }

    #[test]
    fn chromatic_with_non_440_reference() {
        let matched = chromatic_note_match(445.0, 445.0).unwrap();
        assert_eq!(matched.midi_note, 69);
        assert!(matched.cents.abs() < 0.01);

        let a3 = chromatic_note_match(222.5, 445.0).unwrap();
        assert_eq!(a3.midi_note, 57);
        assert_eq!(midi_note_name(a3.midi_note), "A3");
    }

    #[test]
    fn chromatic_rejects_zero_or_negative_frequency() {
        assert!(chromatic_note_match(0.0, 440.0).is_none());
        assert!(chromatic_note_match(-10.0, 440.0).is_none());
        assert!(chromatic_note_match(440.0, 0.0).is_none());
        assert!(chromatic_note_match(440.0, -1.0).is_none());
    }

    #[test]
    fn guitar_rejects_zero_or_negative_frequency() {
        assert!(guitar_note_match(0.0, 440.0).is_none());
        assert!(guitar_note_match(440.0, -1.0).is_none());
    }

    #[test]
    fn tuning_color_exact_boundaries() {
        assert_eq!(tuning_color(5.0), TuningColor::Green);
        assert_eq!(tuning_color(-5.0), TuningColor::Green);
        assert_eq!(tuning_color(5.01), TuningColor::Yellow);
        assert_eq!(tuning_color(15.0), TuningColor::Yellow);
        assert_eq!(tuning_color(-15.0), TuningColor::Yellow);
        assert_eq!(tuning_color(15.01), TuningColor::OrangeRed);
    }

    #[test]
    fn midi_note_name_coverage() {
        assert_eq!(midi_note_name(60), "C4");
        assert_eq!(midi_note_name(61), "C#4");
        assert_eq!(midi_note_name(56), "G#3");
        assert_eq!(midi_note_name(59), "B3");
        assert_eq!(midi_note_name(64), "E4");
        assert_eq!(midi_note_name(0), "C-1");
        assert_eq!(midi_note_name(127), "G9");
    }

    #[test]
    fn cents_between_unison_and_octave() {
        assert!((cents_between(440.0, 440.0)).abs() < 0.001);
        assert!((cents_between(880.0, 440.0) - 1_200.0).abs() < 0.01);
        assert!((cents_between(220.0, 440.0) + 1_200.0).abs() < 0.01);
    }

    #[test]
    fn map_frequency_dispatches_by_mode() {
        let chromatic = map_frequency(440.0, 440.0, TunerMode::Chromatic).unwrap();
        assert_eq!(chromatic.midi_note, 69);

        let guitar = map_frequency(111.0, 440.0, TunerMode::Guitar).unwrap();
        assert_eq!(guitar.midi_note, 45);
        assert_eq!(midi_note_name(guitar.midi_note), "A2");
    }

    #[test]
    fn guitar_mode_exact_midpoint() {
        let midpoint = 123.47;
        let matched = guitar_note_match(midpoint, 440.0).unwrap();
        assert!(matched.midi_note == 45 || matched.midi_note == 50);
        assert!(
            (midi_note_name(matched.midi_note) == "A2")
                || (midi_note_name(matched.midi_note) == "D3")
        );
    }

    #[test]
    fn fast_display_holds_through_brief_missing() {
        let mut fast = ResponseSmoother::new(ResponseMode::Fast);
        fast.update(Some(active_snapshot(69, 0.0)));

        for _ in 0..(FAST_DISPLAY_HOLD_FRAMES - 1) {
            let held = fast.update(None);
            assert!(held.active);
            assert_eq!(held.midi_note, 69);
        }

        assert!(!fast.update(None).active);
    }

    #[test]
    fn smoother_mode_switch_resets_candidate() {
        let mut smoother = ResponseSmoother::new(ResponseMode::Stable);
        smoother.update(Some(active_snapshot(69, 0.0)));

        smoother.update(Some(active_snapshot(71, 0.0)));
        assert_eq!(
            smoother.update(Some(active_snapshot(71, 0.0))).midi_note,
            69
        );

        smoother.set_mode(ResponseMode::Fast);
        let result = smoother.update(Some(active_snapshot(71, 0.0)));
        assert_eq!(result.midi_note, 71);
    }

    #[test]
    fn smoother_reset_forces_new_acquisition() {
        let mut smoother = ResponseSmoother::new(ResponseMode::Stable);
        smoother.update(Some(active_snapshot(69, 0.0)));
        assert_eq!(
            smoother.update(Some(active_snapshot(69, 5.0))).midi_note,
            69
        );

        smoother.reset();
        let after_reset = smoother.update(Some(active_snapshot(71, 10.0)));
        assert!(after_reset.active);
        assert_eq!(after_reset.midi_note, 71);
    }

    #[test]
    fn fast_mode_rejects_moderate_confidence() {
        let mut fast = ResponseSmoother::new(ResponseMode::Fast);
        fast.update(Some(active_snapshot(69, 0.0)));

        let result = fast.update(Some(active_snapshot_with_confidence(69, 12.0, 0.65)));
        assert!(result.active);
        assert_eq!(result.midi_note, 69);
        assert!(result.confidence < 0.65);
    }

    #[test]
    fn midi_stable_requires_three_confirmations() {
        let mut midi = MidiState::new(48_000.0);
        let a4 = active_snapshot(69, 0.0);
        let b4 = active_snapshot(71, 0.0);

        assert_eq!(
            midi.update(Some(a4), ResponseMode::Stable, 512),
            MidiDecision::None
        );
        assert_eq!(
            midi.update(Some(a4), ResponseMode::Stable, 512),
            MidiDecision::None
        );
        assert_eq!(
            midi.update(Some(a4), ResponseMode::Stable, 512),
            MidiDecision::NoteOn {
                note: 69,
                velocity: MIDI_VELOCITY
            }
        );

        assert_eq!(
            midi.update(Some(b4), ResponseMode::Stable, 512),
            MidiDecision::None
        );
        assert_eq!(
            midi.update(Some(b4), ResponseMode::Stable, 512),
            MidiDecision::None
        );
        assert_eq!(
            midi.update(Some(b4), ResponseMode::Stable, 512),
            MidiDecision::NoteChange {
                off: 69,
                on: 71,
                velocity: MIDI_VELOCITY
            }
        );
    }

    #[test]
    fn midi_same_note_stays_idle() {
        let mut midi = MidiState::new(48_000.0);
        let a4 = active_snapshot(69, 0.0);

        assert_eq!(
            midi.update(Some(a4), ResponseMode::Fast, 128),
            MidiDecision::NoteOn {
                note: 69,
                velocity: MIDI_VELOCITY
            }
        );
        assert_eq!(
            midi.update(Some(a4), ResponseMode::Fast, 128),
            MidiDecision::None
        );
        assert_eq!(
            midi.update(Some(a4), ResponseMode::Fast, 128),
            MidiDecision::None
        );
    }

    #[test]
    fn midi_reset_clears_state() {
        let mut midi = MidiState::new(48_000.0);
        midi.update(Some(active_snapshot(69, 0.0)), ResponseMode::Fast, 128);

        midi.reset();
        assert_eq!(
            midi.update(Some(active_snapshot(69, 0.0)), ResponseMode::Fast, 128),
            MidiDecision::NoteOn {
                note: 69,
                velocity: MIDI_VELOCITY
            }
        );
    }

    #[test]
    fn midi_silence_below_timeout_returns_none() {
        let mut midi = MidiState::new(48_000.0);
        midi.update(Some(active_snapshot(69, 0.0)), ResponseMode::Fast, 128);

        let timeout = silence_timeout_samples(48_000.0);
        assert_eq!(
            midi.update(None, ResponseMode::Fast, timeout / 2),
            MidiDecision::None
        );
        assert_eq!(
            midi.update(None, ResponseMode::Fast, timeout / 2 - 1),
            MidiDecision::None
        );
    }

    #[test]
    fn silence_timeout_different_sample_rates() {
        let timeout_48k = silence_timeout_samples(48_000.0);
        let timeout_96k = silence_timeout_samples(96_000.0);
        assert_eq!(timeout_96k, timeout_48k * 2);
    }
}
