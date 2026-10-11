# Trace Tuner

Trace Tuner is a compact tuner plugin for musicians and producers. It listens to incoming audio, shows the detected pitch and tuning drift, and can emit the detected note as MIDI for simple audio-to-note workflows.

## Features

- Real-time monophonic pitch detection for guitar, bass, voice, and other clear single-note sources (28–2000 Hz)
- Chromatic tuning mode plus a guitar-string target mode
- Stable and Fast response modes
- Switchable pitch detectors: YIN, MPM, and ACF
- Minimal confidence meter
- Analysis-only input normalization to improve quiet-note detection
- Optional pYIN comparison detector behind the `pyin-detector` build feature
- Tuning history graph
- A4 reference pitch from 430 Hz to 450 Hz
- Audio passthrough: the plugin does not alter the incoming signal
- MIDI note output from the actual chromatic pitch, independent of the tuning target mode
- CLAP and VST3 plugin formats

## Installation

Trace Tuner is intended for modern DAWs that support CLAP or VST3 audio effects.

1. Download a [release for your platform](https://github.com/Fannon/trace-tuner/releases).
2. Install `TraceTuner.clap` and/or `TraceTuner.vst3` into your normal plugin folder.
3. Rescan plugins in your DAW.
4. Insert Trace Tuner on an audio track with a monophonic source.

## Usage

- Play one note at a time. Trace Tuner is not designed for chords or dense polyphonic material.
- Use `Chromatic` for general tuning, including bass.
- Use `Guitar` when you want the display to snap to standard guitar string targets.
- Use `Stable` for sustained tuning checks. It smooths the display and holds through short confidence dropouts as a note rings out.
- Use `Fast` for faster tracking of bends, vibrato, slides, and quick note changes. It reacts sooner, but can jump more on ambiguous input.
- Use `YIN` as the default detector, `MPM` for a harmonic-rich musical comparison, and `ACF` as a simple autocorrelation baseline.
- YIN, MPM, and ACF use FFT-based linear autocorrelation with preallocated plans and buffers. Analysis updates every 4 ms and tries windows of 12, 28, 52, 76, 104, and 120 ms, choosing the shortest confident result. Fast requires at least two periods; Stable requires three periods and three confirmations.
- Audio passes through with zero added delay. Pitch and MIDI acquisition still require several waveform periods. Measured synthetic plucks with YIN/MPM in Fast mode acquired A4 in 12 ms, guitar E2 in 28 ms, bass E1 in 52 ms, and bass B0 in 76 ms at 48/96 kHz. Real instruments, noise, and host routing can increase these times.
- Stereo analysis uses the stronger channel with hysteresis, avoiding cancellation from opposite-phase signals. Audio on both channels remains unchanged.
- Input normalization is enabled by default for the internal detection buffer only. It can boost quiet analysis windows up to a capped gain, but it does not alter the audio output.
- Build with `--features pyin-detector` if you need the experimental pYIN comparison detector. It is not enabled by default because the current crate-based implementation allocates during detection.
- Adjust `A4` if your session or instrument uses a reference other than 440 Hz.

The green zone represents roughly +/-10 cents. +/-5 cents is very tight; +/-10 cents is commonly acceptable for practical tuning, depending on instrument and context.

## MIDI Output

Trace Tuner emits one active MIDI note based on the detected chromatic pitch. Guitar mode changes the display target and does not remap fretted notes to open strings.

- CLAP supports note output directly.
- VST3 host support for MIDI/note output from audio effects varies, so routing depends on the DAW.
- MIDI output uses a stricter confidence gate than the visual display to avoid extra note chatter.
- Stable mode waits for three confirmed frames before switching notes.
- Fast mode can switch after one confirmed frame.
- Changing an active MIDI note requires at least 90% detector confidence to suppress ambiguous transition frames; initial acquisition uses 80%.
- MIDI releases after 40 ms of missing detection in Fast mode and 120 ms in Stable mode. The recent-input silence gate can add up to roughly 12 ms, plus analysis scheduling.
- Output uses fixed velocity and discrete notes; pitch bend and polyphonic tracking are not implemented.

## Release Notes

See [CHANGELOG.md](CHANGELOG.md) for release history and the changes in v1.2.0.

The core tuner path is working, but expect host-specific rough edges.

Known limitations:

- Monophonic sources only
- No standalone app
- Fixed-size editor for now
- No built-in calibration workflow beyond the A4 parameter
- Tuning confidence depends strongly on input level, background noise, and note sustain
- The optional pYIN detector is useful for comparison, but the current crate-based implementation allocates during detection and is heavier than the hand-written YIN, MPM, and ACF detectors.

Trace Tuner was developed with AI assistance. Code, behavior, and release artifacts are still reviewed and maintained by the project author.

## Development

Build release plugins with the GUI enabled:

```sh
cargo xtask bundle trace_tuner --release --features gui
```

To disable analysis normalization for comparison builds:

```sh
cargo xtask bundle trace_tuner --release --no-default-features --features gui
```

To include the experimental pYIN detector:

```sh
cargo xtask bundle trace_tuner --release --features gui,pyin-detector
```

For local drop-in builds with timestamped snapshots:

```sh
bash build.sh
```

On Windows:

```bat
build.bat
```

The scripts run from their own directory, build before copying to `bin/`, and preserve previous artifacts in `tmp/`. Each successful build creates a unique timestamped snapshot. Windows uses `build.ps1` through the batch wrapper; an MSVC build requires the Visual Studio C++ build tools.

Local build scripts default to two Cargo jobs. The Windows script also restricts the build process and its children to two logical CPUs, then restores the caller's affinity and job setting.

On Windows during local testing, the easiest drop-in artifact is:

```text
bin/TraceTuner.clap
```

Verify before release:

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Measure detector cost using the original 4096-sample window at 48 and 96 kHz:

```sh
cargo test --release --test detector_performance -- --ignored --nocapture
```

Check that the production detectors allocate nothing during analysis, including the first call and algorithm switches:

```sh
cargo test --no-default-features --features analysis-normalization --test detector_allocations
```

Measure complete callback cost and MIDI acquisition, and compare detectors across synthetic instrument signals:

```sh
cargo test --release --no-default-features --features analysis-normalization --lib measure_live_onset_and_callback_cost -- --ignored --nocapture
cargo test --release --test detector_comparison -- --ignored --nocapture
cargo test --release --no-default-features --features analysis-normalization
```

The last command also checks allocations inside the complete streaming callback. Timing tests print diagnostics instead of asserting machine-dependent deadlines. See [the codebase review](docs/codebase-review.md) for findings, measurements, limitations, and the mode evaluation plan.

## Credits

Trace Tuner is built with:

- [NIH-plug](https://github.com/robbert-vdh/nih-plug) for CLAP/VST3 plugin infrastructure.
- [nih-plug-egui](https://github.com/robbert-vdh/nih-plug) and egui for the editor UI.
- [atomic_float](https://crates.io/crates/atomic_float) for lock-free audio-to-UI state sharing.
- [pyin](https://github.com/Sytronik/pyin-rs), based on librosa's pYIN implementation, for the experimental pYIN detector.

The included YIN, MPM, and ACF implementations are maintained in this repository. pYIN is based on Mauch and Dixon, "pYIN: A fundamental frequency estimator using probabilistic threshold distributions", 2014.
