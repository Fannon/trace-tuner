use nih_plug::prelude::*;
#[cfg(feature = "gui")]
use nih_plug_egui::EguiState;
use std::sync::Arc;

pub mod core;

use core::{ResponseMode, TunerMode};

pub struct TraceTuner {
    params: Arc<TraceTunerParams>,
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

impl Default for TraceTuner {
    fn default() -> Self {
        Self {
            params: Arc::new(TraceTunerParams::default()),
        }
    }
}

impl Default for TraceTunerParams {
    fn default() -> Self {
        Self {
            #[cfg(feature = "gui")]
            editor_state: EguiState::from_size(400, 300),
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

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
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
