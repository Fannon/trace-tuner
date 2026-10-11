use atomic_float::AtomicF32;
use nih_plug::prelude::*;
#[cfg(feature = "gui")]
use nih_plug_egui::EguiState;
use std::sync::{
    atomic::{fence, AtomicBool, AtomicU8, AtomicUsize, Ordering},
    Arc,
};

mod autocorrelation;
pub mod core;
#[cfg(test)]
mod stream_tests;
#[cfg(feature = "gui")]
mod ui;

use core::{
    map_frequency, DetectionAlgorithm, DetectionSnapshot, MidiDecision, MidiState, PitchDetector,
    ResponseMode, ResponseSmoother, TunerMode, ACQUIRE_CONFIDENCE,
};

const ANALYSIS_WINDOWS_MS: [f32; 6] = [12.0, 28.0, 52.0, 76.0, 104.0, 120.0];
const ANALYSIS_HOP_MS: f32 = 4.0;
const ANALYSIS_FLOOR_RMS: f32 = 0.0025;
const MIDI_SWITCH_CONFIDENCE: f32 = 0.90;
#[cfg(feature = "analysis-normalization")]
const ANALYSIS_NORMALIZATION_FLOOR_RMS: f32 = 0.0025;
#[cfg(feature = "analysis-normalization")]
const ANALYSIS_NORMALIZATION_TARGET_RMS: f32 = 0.05;
#[cfg(feature = "analysis-normalization")]
const ANALYSIS_NORMALIZATION_MAX_GAIN: f32 = 8.0;
pub const HISTORY_LEN: usize = 160;

pub struct TraceTuner {
    params: Arc<TraceTunerParams>,
    detector: PitchDetector,
    smoother: ResponseSmoother,
    midi_state: MidiState,
    analysis_window: Vec<f32>,
    analysis_scratch: Vec<f32>,
    write_pos: usize,
    filled_samples: usize,
    samples_since_analysis: usize,
    sample_rate: f32,
    window_lengths: [usize; 6],
    hop_samples: usize,
    analysis_channel: usize,
    history_samples: usize,
    last_display_config: Option<(TunerMode, f32, DetectionAlgorithm)>,
    shared_state: Arc<SharedTunerState>,
}

#[derive(Params)]
pub struct TraceTunerParams {
    #[cfg(feature = "gui")]
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "algorithm"]
    pub algorithm: EnumParam<DetectionAlgorithm>,

    #[id = "mode"]
    pub mode: EnumParam<TunerMode>,

    #[id = "response"]
    pub response: EnumParam<ResponseMode>,

    #[id = "a4"]
    pub reference_pitch: FloatParam,
}

pub struct SharedTunerState {
    version: AtomicUsize,
    active: AtomicBool,
    held: AtomicBool,
    frequency_hz: AtomicF32,
    confidence: AtomicF32,
    rms: AtomicF32,
    midi_note: AtomicU8,
    target_frequency_hz: AtomicF32,
    cents: AtomicF32,
    history: [AtomicF32; HISTORY_LEN],
    history_confidence: [AtomicF32; HISTORY_LEN],
    history_midi_note: [AtomicU8; HISTORY_LEN],
    history_held: [AtomicBool; HISTORY_LEN],
    history_write_pos: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryPoint {
    pub cents: f32,
    pub confidence: f32,
    pub midi_note: u8,
    pub held: bool,
}

impl Default for SharedTunerState {
    fn default() -> Self {
        Self {
            version: AtomicUsize::new(0),
            active: AtomicBool::new(false),
            held: AtomicBool::new(false),
            frequency_hz: AtomicF32::new(0.0),
            confidence: AtomicF32::new(0.0),
            rms: AtomicF32::new(0.0),
            midi_note: AtomicU8::new(0),
            target_frequency_hz: AtomicF32::new(0.0),
            cents: AtomicF32::new(0.0),
            history: std::array::from_fn(|_| AtomicF32::new(f32::NAN)),
            history_confidence: std::array::from_fn(|_| AtomicF32::new(f32::NAN)),
            history_midi_note: std::array::from_fn(|_| AtomicU8::new(0)),
            history_held: std::array::from_fn(|_| AtomicBool::new(false)),
            history_write_pos: AtomicUsize::new(0),
        }
    }
}

impl SharedTunerState {
    pub fn snapshot(&self) -> DetectionSnapshot {
        self.read_consistent(|| DetectionSnapshot {
            active: self.active.load(Ordering::Relaxed),
            held: self.held.load(Ordering::Relaxed),
            frequency_hz: self.frequency_hz.load(Ordering::Relaxed),
            confidence: self.confidence.load(Ordering::Relaxed),
            rms: self.rms.load(Ordering::Relaxed),
            midi_note: self.midi_note.load(Ordering::Relaxed),
            target_frequency_hz: self.target_frequency_hz.load(Ordering::Relaxed),
            cents: self.cents.load(Ordering::Relaxed),
        })
    }

    pub fn history(&self) -> [HistoryPoint; HISTORY_LEN] {
        self.read_consistent(|| {
            let write_pos = self.history_write_pos.load(Ordering::Relaxed);
            std::array::from_fn(|index| {
                let source = (write_pos + index) % HISTORY_LEN;
                HistoryPoint {
                    cents: self.history[source].load(Ordering::Relaxed),
                    confidence: self.history_confidence[source].load(Ordering::Relaxed),
                    midi_note: self.history_midi_note[source].load(Ordering::Relaxed),
                    held: self.history_held[source].load(Ordering::Relaxed),
                }
            })
        })
    }

    // Only the GUI retries. The audio writer never locks or waits for readers.
    fn read_consistent<T>(&self, read: impl Fn() -> T) -> T {
        loop {
            let version = self.version.load(Ordering::Acquire);
            if !version.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let value = read();
            fence(Ordering::Acquire);
            if self.version.load(Ordering::Relaxed) == version {
                return value;
            }
        }
    }

    fn publish(&self, snapshot: DetectionSnapshot) {
        self.publish_with_history(snapshot, true);
    }

    fn publish_with_history(&self, snapshot: DetectionSnapshot, append_history: bool) {
        self.version.fetch_add(1, Ordering::AcqRel);
        self.active.store(snapshot.active, Ordering::Relaxed);
        self.held.store(snapshot.held, Ordering::Relaxed);
        self.frequency_hz
            .store(snapshot.frequency_hz, Ordering::Relaxed);
        self.confidence
            .store(snapshot.confidence, Ordering::Relaxed);
        self.rms.store(snapshot.rms, Ordering::Relaxed);
        self.midi_note.store(snapshot.midi_note, Ordering::Relaxed);
        self.target_frequency_hz
            .store(snapshot.target_frequency_hz, Ordering::Relaxed);
        self.cents.store(snapshot.cents, Ordering::Relaxed);

        if !append_history {
            self.version.fetch_add(1, Ordering::Release);
            return;
        }

        let write_pos = self.history_write_pos.load(Ordering::Relaxed);
        self.history[write_pos].store(
            if snapshot.active {
                snapshot.cents
            } else {
                f32::NAN
            },
            Ordering::Relaxed,
        );
        self.history_confidence[write_pos].store(
            if snapshot.active {
                snapshot.confidence
            } else {
                f32::NAN
            },
            Ordering::Relaxed,
        );
        self.history_midi_note[write_pos].store(
            if snapshot.active {
                snapshot.midi_note
            } else {
                0
            },
            Ordering::Relaxed,
        );
        self.history_held[write_pos].store(snapshot.active && snapshot.held, Ordering::Relaxed);
        self.history_write_pos
            .store((write_pos + 1) % HISTORY_LEN, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Release);
    }

    fn clear_history(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
        for index in 0..HISTORY_LEN {
            self.history[index].store(f32::NAN, Ordering::Relaxed);
            self.history_confidence[index].store(f32::NAN, Ordering::Relaxed);
            self.history_midi_note[index].store(0, Ordering::Relaxed);
            self.history_held[index].store(false, Ordering::Relaxed);
        }
        self.history_write_pos.store(0, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Release);
    }
}

impl Default for TraceTuner {
    fn default() -> Self {
        let sample_rate = 48_000.0;
        let window_lengths = analysis_lengths(sample_rate);
        let max_window = window_lengths[5];
        Self {
            params: Arc::new(TraceTunerParams::default()),
            detector: PitchDetector::new(sample_rate, max_window),
            smoother: ResponseSmoother::new(ResponseMode::Stable),
            midi_state: MidiState::new(sample_rate),
            analysis_window: vec![0.0; max_window],
            analysis_scratch: vec![0.0; max_window],
            write_pos: 0,
            filled_samples: 0,
            samples_since_analysis: 0,
            sample_rate,
            window_lengths,
            hop_samples: (sample_rate * ANALYSIS_HOP_MS / 1_000.0).round() as usize,
            analysis_channel: 0,
            history_samples: 0,
            last_display_config: None,
            shared_state: Arc::new(SharedTunerState::default()),
        }
    }
}

impl Default for TraceTunerParams {
    fn default() -> Self {
        Self {
            #[cfg(feature = "gui")]
            editor_state: EguiState::from_size(430, 300),
            algorithm: EnumParam::new("Algorithm", DetectionAlgorithm::Yin),
            mode: EnumParam::new("Mode", TunerMode::Chromatic),
            response: EnumParam::new("Response", ResponseMode::Stable),
            reference_pitch: FloatParam::new(
                "A4",
                440.0,
                FloatRange::Linear {
                    min: 430.0,
                    max: 450.0,
                },
            )
            .with_unit(" Hz")
            .with_step_size(0.1)
            .with_value_to_string(formatters::v2s_f32_rounded(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
        }
    }
}

impl Plugin for TraceTuner {
    const NAME: &'static str = "Trace Tuner";
    const VENDOR: &'static str = "Simon Heimler";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    #[cfg(feature = "gui")]
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        ui::create_editor(self.params.clone(), self.shared_state.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        if !buffer_config.sample_rate.is_finite() || buffer_config.sample_rate < 8_000.0 {
            return false;
        }
        self.sample_rate = buffer_config.sample_rate;
        self.window_lengths = analysis_lengths(self.sample_rate);
        let max_window = self.window_lengths[5];
        self.hop_samples = (self.sample_rate * ANALYSIS_HOP_MS / 1_000.0).round() as usize;
        self.analysis_window.resize(max_window, 0.0);
        self.analysis_scratch.resize(max_window, 0.0);
        self.detector
            .set_sample_rate(buffer_config.sample_rate, max_window);
        self.midi_state.set_sample_rate(buffer_config.sample_rate);
        self.smoother
            .set_analysis_interval(self.hop_samples as f32 * 1_000.0 / self.sample_rate);
        context.set_latency_samples(0);
        self.reset();
        true
    }

    fn reset(&mut self) {
        self.analysis_window.fill(0.0);
        self.analysis_scratch.fill(0.0);
        self.write_pos = 0;
        self.filled_samples = 0;
        self.samples_since_analysis = 0;
        self.analysis_channel = 0;
        self.history_samples = 0;
        self.last_display_config = None;
        self.smoother.reset();
        self.midi_state.reset();
        self.detector.reset();
        self.shared_state.publish(DetectionSnapshot::idle());
        self.shared_state.clear_history();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let channel_count = buffer.channels();
        let sample_count = buffer.samples();
        if channel_count == 0 || sample_count == 0 {
            return ProcessStatus::Normal;
        }

        let mode = self.params.mode.value();
        let response = self.params.response.value();
        let reference_pitch_hz = self.params.reference_pitch.value();
        let algorithm = self.params.algorithm.value();
        let display_config = (mode, reference_pitch_hz, algorithm);
        if self.last_display_config != Some(display_config) {
            self.smoother.reset();
            self.last_display_config = Some(display_config);
        }
        self.smoother.set_mode(response);
        self.detector.set_algorithm(algorithm);

        let channels = buffer.as_slice_immutable();
        // Keep the stronger channel with hysteresis. Averaging stereo cancels
        // opposite-phase signals and attenuates a single connected instrument.
        self.analysis_channel = self.analysis_channel.min(channel_count - 1);
        let mut selected_energy = channel_energy(channels[self.analysis_channel]);
        for (index, channel) in channels.iter().enumerate() {
            let energy = channel_energy(channel);
            if energy > selected_energy * 2.0 {
                self.analysis_channel = index;
                selected_energy = energy;
            }
        }
        for (sample_index, &sample) in channels[self.analysis_channel].iter().enumerate() {
            let mono = if sample.is_finite() { sample } else { 0.0 };

            self.analysis_window[self.write_pos] = mono;
            self.write_pos = (self.write_pos + 1) % self.analysis_window.len();
            self.filled_samples = (self.filled_samples + 1).min(self.analysis_window.len());
            self.samples_since_analysis += 1;

            if self.filled_samples >= self.window_lengths[0]
                && self.samples_since_analysis >= self.hop_samples
            {
                let elapsed_samples = self.samples_since_analysis as u32;
                self.samples_since_analysis = 0;
                let (pitch, input_rms) = self.analyze(response);
                let detected = pitch.and_then(|pitch| {
                    map_frequency(pitch.frequency_hz, reference_pitch_hz, mode).map(|note| {
                        DetectionSnapshot {
                            active: true,
                            held: false,
                            frequency_hz: pitch.frequency_hz,
                            confidence: pitch.confidence,
                            rms: pitch.rms,
                            midi_note: note.midi_note,
                            target_frequency_hz: note.target_frequency_hz,
                            cents: note.cents,
                        }
                    })
                });

                let mut smoothed = self.smoother.update(detected);
                smoothed.rms = input_rms;
                self.history_samples += elapsed_samples as usize;
                let history_interval = self.window_lengths[0];
                let append_history = self.history_samples >= history_interval;
                if append_history {
                    self.history_samples %= history_interval;
                }
                self.shared_state
                    .publish_with_history(smoothed, append_history);
                // Tuning targets affect the display, not the MIDI note played.
                let detection = detected
                    .filter(|snapshot| snapshot.confidence >= ACQUIRE_CONFIDENCE)
                    .and_then(|mut snapshot| {
                        let note = map_frequency(
                            snapshot.frequency_hz,
                            reference_pitch_hz,
                            TunerMode::Chromatic,
                        )?;
                        snapshot.midi_note = note.midi_note;
                        snapshot.target_frequency_hz = note.target_frequency_hz;
                        snapshot.cents = note.cents;
                        if self
                            .midi_state
                            .active_note()
                            .is_some_and(|active| active != note.midi_note)
                            && snapshot.confidence < MIDI_SWITCH_CONFIDENCE
                        {
                            return None;
                        }
                        Some(snapshot)
                    });
                let decision = self.midi_state.update(detection, response, elapsed_samples);
                self.emit_midi(context, decision, sample_index as u32);
            }
        }

        ProcessStatus::Normal
    }
}

impl TraceTuner {
    fn copy_analysis_window(&mut self, length: usize) {
        let capacity = self.analysis_window.len();
        let start = (self.write_pos + capacity - length) % capacity;
        let first = length.min(capacity - start);
        self.analysis_scratch[..first].copy_from_slice(&self.analysis_window[start..start + first]);
        self.analysis_scratch[first..length]
            .copy_from_slice(&self.analysis_window[..length - first]);
    }

    fn analyze(&mut self, response: ResponseMode) -> (Option<core::PitchEstimate>, f32) {
        self.copy_analysis_window(self.window_lengths[0]);
        let input_rms = analysis_rms(&self.analysis_scratch[..self.window_lengths[0]]);
        // Old samples in a long bass window must not prolong notes after silence.
        if input_rms < ANALYSIS_FLOOR_RMS {
            return (None, input_rms);
        }
        let mut best = None;
        for length in self.window_lengths {
            if length > self.filled_samples {
                break;
            }
            self.copy_analysis_window(length);
            let samples = &mut self.analysis_scratch[..length];
            #[cfg(feature = "analysis-normalization")]
            {
                let rms = if length == self.window_lengths[0] {
                    input_rms
                } else {
                    analysis_rms(samples)
                };
                normalize_analysis_window(samples, rms);
            }
            if let Some(pitch) = self.detector.detect(samples) {
                // Stable uses three periods; Fast can acquire with two.
                let periods = length as f32 * pitch.frequency_hz / self.sample_rate;
                let required = if response == ResponseMode::Stable {
                    3.0
                } else {
                    2.0
                };
                if periods < required {
                    continue;
                }
                if pitch.confidence >= ACQUIRE_CONFIDENCE {
                    return (Some(pitch), input_rms);
                }
                if best.is_none_or(|previous: core::PitchEstimate| {
                    pitch.confidence > previous.confidence
                }) {
                    best = Some(pitch);
                }
            }
        }
        (best, input_rms)
    }

    fn emit_midi(
        &mut self,
        context: &mut impl ProcessContext<Self>,
        decision: MidiDecision,
        timing: u32,
    ) {
        match decision {
            MidiDecision::None => {}
            MidiDecision::NoteOn { note, velocity } => {
                context.send_event(NoteEvent::NoteOn {
                    timing,
                    voice_id: None,
                    channel: 0,
                    note,
                    velocity,
                });
            }
            MidiDecision::NoteOff { note } => {
                context.send_event(NoteEvent::NoteOff {
                    timing,
                    voice_id: None,
                    channel: 0,
                    note,
                    velocity: 0.0,
                });
            }
            MidiDecision::NoteChange { off, on, velocity } => {
                context.send_event(NoteEvent::NoteOff {
                    timing,
                    voice_id: None,
                    channel: 0,
                    note: off,
                    velocity: 0.0,
                });
                context.send_event(NoteEvent::NoteOn {
                    timing,
                    voice_id: None,
                    channel: 0,
                    note: on,
                    velocity,
                });
            }
        }
    }
}

#[cfg(feature = "analysis-normalization")]
fn normalize_analysis_window(samples: &mut [f32], rms: f32) -> f32 {
    if rms < ANALYSIS_NORMALIZATION_FLOOR_RMS {
        return 1.0;
    }

    let gain =
        (ANALYSIS_NORMALIZATION_TARGET_RMS / rms).clamp(1.0, ANALYSIS_NORMALIZATION_MAX_GAIN);
    if gain > 1.0 {
        for sample in samples {
            *sample *= gain;
        }
    }
    gain
}

fn analysis_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let energy = samples.iter().map(|sample| sample * sample).sum::<f32>();
    (energy / samples.len() as f32).sqrt()
}

fn analysis_lengths(sample_rate: f32) -> [usize; 6] {
    ANALYSIS_WINDOWS_MS.map(|ms| (sample_rate * ms / 1_000.0).round().max(32.0) as usize)
}

fn channel_energy(samples: &[f32]) -> f64 {
    samples
        .iter()
        .filter(|sample| sample.is_finite())
        .map(|&sample| f64::from(sample).powi(2))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::midi_note_frequency;

    fn snapshot(midi_note: u8, cents: f32, held: bool) -> DetectionSnapshot {
        DetectionSnapshot {
            active: true,
            held,
            frequency_hz: midi_note_frequency(midi_note, 440.0),
            confidence: 0.75,
            rms: 0.1,
            midi_note,
            target_frequency_hz: midi_note_frequency(midi_note, 440.0),
            cents,
        }
    }

    #[cfg(feature = "analysis-normalization")]
    #[test]
    fn analysis_normalization_boosts_quiet_signal_with_cap() {
        let mut samples = [0.005_f32; 128];

        let rms = analysis_rms(&samples);
        let gain = normalize_analysis_window(&mut samples, rms);

        assert_eq!(gain, ANALYSIS_NORMALIZATION_MAX_GAIN);
        assert!(samples.iter().all(|sample| (*sample - 0.04).abs() < 0.001));
    }

    #[cfg(feature = "analysis-normalization")]
    #[test]
    fn analysis_normalization_leaves_near_silence_unboosted() {
        let mut samples = [0.001_f32; 128];

        let rms = analysis_rms(&samples);
        let gain = normalize_analysis_window(&mut samples, rms);

        assert_eq!(gain, 1.0);
        assert!(samples
            .iter()
            .all(|sample| (*sample - 0.001).abs() < 0.0001));
    }

    #[cfg(feature = "analysis-normalization")]
    #[test]
    fn analysis_normalization_does_not_attenuate_loud_signal() {
        let mut samples = [0.2_f32; 128];

        let rms = analysis_rms(&samples);
        let gain = normalize_analysis_window(&mut samples, rms);

        assert_eq!(gain, 1.0);
        assert!(samples.iter().all(|sample| (*sample - 0.2).abs() < 0.0001));
    }

    #[test]
    fn shared_state_history_keeps_note_and_held_metadata() {
        let state = SharedTunerState::default();

        state.publish(snapshot(69, 4.0, false));
        state.publish(snapshot(69, 4.0, true));
        state.publish(snapshot(71, -8.0, false));

        let history = state.history();
        let recent = &history[HISTORY_LEN - 3..];

        assert_eq!(recent[0].midi_note, 69);
        assert!(!recent[0].held);
        assert_eq!(recent[1].midi_note, 69);
        assert!(recent[1].held);
        assert_eq!(recent[2].midi_note, 71);
        assert!(!recent[2].held);
    }

    #[test]
    fn shared_state_snapshot_reports_held_status() {
        let state = SharedTunerState::default();

        state.publish(snapshot(69, 0.0, true));

        let snapshot = state.snapshot();
        assert!(snapshot.active);
        assert!(snapshot.held);
        assert_eq!(snapshot.midi_note, 69);
    }
}

impl ClapPlugin for TraceTuner {
    const CLAP_ID: &'static str = "com.simonheimler.trace-tuner";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A minimal monophonic tuner with MIDI note output");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for TraceTuner {
    const VST3_CLASS_ID: [u8; 16] = *b"TraceTunerPlug01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_clap!(TraceTuner);
// The upstream VST3 macro currently triggers this future-compatibility lint.
// Scope the exception to the generated export, keeping our own code linted.
#[allow(semicolon_in_expressions_from_non_local_macros)]
mod vst3_exports {
    use super::*;
    nih_export_vst3!(TraceTuner);
}
