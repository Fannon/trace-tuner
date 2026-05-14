use nih_plug::prelude::Enum;

pub const MIDI_VELOCITY: f32 = 100.0 / 127.0;
pub const SILENCE_TIMEOUT_MS: f32 = 120.0;

const ACQUIRE_CONFIDENCE: f32 = 0.80;
const HOLD_CONFIDENCE: f32 = 0.60;
const STABLE_DISPLAY_HOLD_FRAMES: u8 = 36;
const FAST_DISPLAY_HOLD_FRAMES: u8 = 4;

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
    1200.0 * (frequency_hz / target_frequency_hz).log2()
}

pub fn tuning_color(cents: f32) -> TuningColor {
    match cents.abs() {
        cents if cents <= 5.0 => TuningColor::Green,
        cents if cents <= 15.0 => TuningColor::Yellow,
        _ => TuningColor::OrangeRed,
    }
}

pub struct YinDetector {
    sample_rate: f32,
    min_frequency_hz: f32,
    max_frequency_hz: f32,
    threshold: f32,
    difference: Vec<f32>,
    cumulative_mean: Vec<f32>,
}

impl YinDetector {
    pub fn new(sample_rate: f32, max_window_samples: usize) -> Self {
        let tau_len = max_window_samples / 2 + 1;
        Self {
            sample_rate,
            min_frequency_hz: 70.0,
            max_frequency_hz: 1_200.0,
            threshold: 0.14,
            difference: vec![0.0; tau_len],
            cumulative_mean: vec![0.0; tau_len],
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32, max_window_samples: usize) {
        self.sample_rate = sample_rate;
        let tau_len = max_window_samples / 2 + 1;
        self.difference.resize(tau_len, 0.0);
        self.cumulative_mean.resize(tau_len, 0.0);
    }

    pub fn detect(&mut self, samples: &[f32]) -> Option<PitchEstimate> {
        if samples.len() < 32 || self.sample_rate <= 0.0 {
            return None;
        }

        let rms = root_mean_square(samples);
        if rms < 0.01 {
            return None;
        }

        let max_tau = ((self.sample_rate / self.min_frequency_hz) as usize)
            .min(samples.len().saturating_sub(2))
            .min(self.difference.len().saturating_sub(1));
        let min_tau = ((self.sample_rate / self.max_frequency_hz) as usize).max(2);
        if max_tau <= min_tau + 2 {
            return None;
        }

        self.difference[0] = 0.0;
        for tau in 1..=max_tau {
            let mut sum = 0.0;
            for i in 0..(samples.len() - tau) {
                let delta = samples[i] - samples[i + tau];
                sum += delta * delta;
            }
            self.difference[tau] = sum;
        }

        self.cumulative_mean[0] = 1.0;
        let mut running_sum = 0.0;
        for tau in 1..=max_tau {
            running_sum += self.difference[tau];
            self.cumulative_mean[tau] = if running_sum > 0.0 {
                self.difference[tau] * tau as f32 / running_sum
            } else {
                1.0
            };
        }

        let mut tau = min_tau;
        while tau <= max_tau {
            if self.cumulative_mean[tau] < self.threshold {
                while tau < max_tau && self.cumulative_mean[tau + 1] < self.cumulative_mean[tau] {
                    tau += 1;
                }

                let better_tau = parabolic_interpolation(&self.cumulative_mean, tau, max_tau);
                let frequency_hz = self.sample_rate / better_tau;
                let confidence = (1.0 - self.cumulative_mean[tau]).clamp(0.0, 1.0);
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
}

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
                return self.current;
            }
        }

        self.current = DetectionSnapshot::idle();
        self.missing_count = 0;
        self.current
    }
}

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

fn silence_timeout_samples(sample_rate: f32) -> u32 {
    (sample_rate * SILENCE_TIMEOUT_MS / 1_000.0)
        .round()
        .max(1.0) as u32
}

fn root_mean_square(samples: &[f32]) -> f32 {
    let energy = samples.iter().map(|sample| sample * sample).sum::<f32>();
    (energy / samples.len() as f32).sqrt()
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
        tau as f32 + 0.5 * (left - right) / denominator
    }
}

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
    fn stable_display_keeps_lower_confidence_same_note() {
        let mut stable = ResponseSmoother::new(ResponseMode::Stable);
        stable.update(Some(active_snapshot(69, 0.0)));

        let updated = stable.update(Some(active_snapshot_with_confidence(69, 8.0, 0.65)));

        assert!(updated.active);
        assert_eq!(updated.midi_note, 69);
        assert!(updated.cents > 0.0);
    }

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
    fn yin_detects_monophonic_sine() {
        let sample_rate = 48_000.0;
        let mut detector = YinDetector::new(sample_rate, 2_048);
        let mut samples = [0.0; 2_048];
        for (index, sample) in samples.iter_mut().enumerate() {
            *sample = (TAU * 440.0 * index as f32 / sample_rate).sin() * 0.4;
        }

        let pitch = detector.detect(&samples).unwrap();
        assert!((pitch.frequency_hz - 440.0).abs() < 1.0);
        assert!(pitch.confidence > 0.8);
    }
}
