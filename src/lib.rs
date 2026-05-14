use atomic_float::AtomicF32;
use nih_plug::prelude::*;
#[cfg(feature = "gui")]
use nih_plug_egui::EguiState;
use std::sync::{
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    Arc,
};

pub mod core;
#[cfg(feature = "gui")]
mod ui;

use core::{
    map_frequency, DetectionSnapshot, MidiDecision, MidiState, ResponseMode, ResponseSmoother,
    TunerMode, YinDetector, ACQUIRE_CONFIDENCE,
};

const ANALYSIS_WINDOW_SAMPLES: usize = 2_048;
const ANALYSIS_HOP_SAMPLES: usize = 512;
pub const HISTORY_LEN: usize = 160;

pub struct TraceTuner {
    params: Arc<TraceTunerParams>,
    detector: YinDetector,
    smoother: ResponseSmoother,
    midi_state: MidiState,
    analysis_window: Vec<f32>,
    analysis_scratch: Vec<f32>,
    write_pos: usize,
    filled_samples: usize,
    samples_since_analysis: usize,
    sample_rate: f32,
    shared_state: Arc<SharedTunerState>,
}

#[derive(Params)]
pub struct TraceTunerParams {
    #[cfg(feature = "gui")]
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "mode"]
    pub mode: EnumParam<TunerMode>,

    #[id = "response"]
    pub response: EnumParam<ResponseMode>,

    #[id = "a4"]
    pub reference_pitch: FloatParam,
}

pub struct SharedTunerState {
    active: AtomicBool,
    frequency_hz: AtomicF32,
    confidence: AtomicF32,
    rms: AtomicF32,
    midi_note: AtomicU8,
    target_frequency_hz: AtomicF32,
    cents: AtomicF32,
    history: [AtomicF32; HISTORY_LEN],
    history_confidence: [AtomicF32; HISTORY_LEN],
    history_write_pos: AtomicUsize,
}

impl Default for SharedTunerState {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            frequency_hz: AtomicF32::new(0.0),
            confidence: AtomicF32::new(0.0),
            rms: AtomicF32::new(0.0),
            midi_note: AtomicU8::new(0),
            target_frequency_hz: AtomicF32::new(0.0),
            cents: AtomicF32::new(0.0),
            history: std::array::from_fn(|_| AtomicF32::new(f32::NAN)),
            history_confidence: std::array::from_fn(|_| AtomicF32::new(f32::NAN)),
            history_write_pos: AtomicUsize::new(0),
        }
    }
}

impl SharedTunerState {
    pub fn snapshot(&self) -> DetectionSnapshot {
        DetectionSnapshot {
            active: self.active.load(Ordering::Relaxed),
            frequency_hz: self.frequency_hz.load(Ordering::Relaxed),
            confidence: self.confidence.load(Ordering::Relaxed),
            rms: self.rms.load(Ordering::Relaxed),
            midi_note: self.midi_note.load(Ordering::Relaxed),
            target_frequency_hz: self.target_frequency_hz.load(Ordering::Relaxed),
            cents: self.cents.load(Ordering::Relaxed),
        }
    }

    pub fn history(&self) -> [(f32, f32); HISTORY_LEN] {
        let write_pos = self.history_write_pos.load(Ordering::Relaxed);
        std::array::from_fn(|index| {
            let source = (write_pos + index) % HISTORY_LEN;
            (
                self.history[source].load(Ordering::Relaxed),
                self.history_confidence[source].load(Ordering::Relaxed),
            )
        })
    }

    fn publish(&self, snapshot: DetectionSnapshot) {
        self.active.store(snapshot.active, Ordering::Relaxed);
        self.frequency_hz
            .store(snapshot.frequency_hz, Ordering::Relaxed);
        self.confidence
            .store(snapshot.confidence, Ordering::Relaxed);
        self.rms.store(snapshot.rms, Ordering::Relaxed);
        self.midi_note.store(snapshot.midi_note, Ordering::Relaxed);
        self.target_frequency_hz
            .store(snapshot.target_frequency_hz, Ordering::Relaxed);
        self.cents.store(snapshot.cents, Ordering::Relaxed);

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
        self.history_write_pos
            .store((write_pos + 1) % HISTORY_LEN, Ordering::Relaxed);
    }
}

impl Default for TraceTuner {
    fn default() -> Self {
        let sample_rate = 48_000.0;
        Self {
            params: Arc::new(TraceTunerParams::default()),
            detector: YinDetector::new(sample_rate, ANALYSIS_WINDOW_SAMPLES),
            smoother: ResponseSmoother::new(ResponseMode::Stable),
            midi_state: MidiState::new(sample_rate),
            analysis_window: vec![0.0; ANALYSIS_WINDOW_SAMPLES],
            analysis_scratch: vec![0.0; ANALYSIS_WINDOW_SAMPLES],
            write_pos: 0,
            filled_samples: 0,
            samples_since_analysis: 0,
            sample_rate,
            shared_state: Arc::new(SharedTunerState::default()),
        }
    }
}

impl Default for TraceTunerParams {
    fn default() -> Self {
        Self {
            #[cfg(feature = "gui")]
            editor_state: EguiState::from_size(430, 255),
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
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.detector
            .set_sample_rate(buffer_config.sample_rate, ANALYSIS_WINDOW_SAMPLES);
        self.midi_state.set_sample_rate(buffer_config.sample_rate);
        self.reset();
        true
    }

    fn reset(&mut self) {
        self.analysis_window.fill(0.0);
        self.analysis_scratch.fill(0.0);
        self.write_pos = 0;
        self.filled_samples = 0;
        self.samples_since_analysis = 0;
        self.smoother.reset();
        self.midi_state.reset();
        self.shared_state.publish(DetectionSnapshot::idle());
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
        self.smoother.set_mode(response);

        let channels = buffer.as_slice_immutable();
        for sample_index in 0..sample_count {
            let mut mono = 0.0;
            for channel in channels {
                mono += channel[sample_index];
            }
            mono /= channel_count as f32;

            self.analysis_window[self.write_pos] = mono;
            self.write_pos = (self.write_pos + 1) % ANALYSIS_WINDOW_SAMPLES;
            self.filled_samples = (self.filled_samples + 1).min(ANALYSIS_WINDOW_SAMPLES);
            self.samples_since_analysis += 1;

            if self.filled_samples == ANALYSIS_WINDOW_SAMPLES
                && self.samples_since_analysis >= ANALYSIS_HOP_SAMPLES
            {
                let elapsed_samples = self.samples_since_analysis as u32;
                self.samples_since_analysis = 0;
                self.copy_analysis_window();

                let detected = self
                    .detector
                    .detect(&self.analysis_scratch)
                    .and_then(|pitch| {
                        map_frequency(pitch.frequency_hz, reference_pitch_hz, mode).map(|note| {
                            DetectionSnapshot {
                                active: true,
                                frequency_hz: pitch.frequency_hz,
                                confidence: pitch.confidence,
                                rms: pitch.rms,
                                midi_note: note.midi_note,
                                target_frequency_hz: note.target_frequency_hz,
                                cents: note.cents,
                            }
                        })
                    });

                let smoothed = self.smoother.update(detected);
                self.shared_state.publish(smoothed);
                let detection = detected.filter(|snapshot| snapshot.confidence >= ACQUIRE_CONFIDENCE);
                let decision = self.midi_state.update(detection, response, elapsed_samples);
                self.emit_midi(context, decision, sample_index as u32);
            }
        }

        ProcessStatus::Normal
    }
}

impl TraceTuner {
    fn copy_analysis_window(&mut self) {
        let first = ANALYSIS_WINDOW_SAMPLES - self.write_pos;
        self.analysis_scratch[..first].copy_from_slice(&self.analysis_window[self.write_pos..]);
        self.analysis_scratch[first..].copy_from_slice(&self.analysis_window[..self.write_pos]);
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
nih_export_vst3!(TraceTuner);
