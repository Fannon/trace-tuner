# Trace Tuner

Trace Tuner is a minimal Rust tuner plugin for CLAP and VST3. It passes audio through unchanged, analyzes a mono sum of the input, shows compact tuning feedback, and can emit the detected target note as MIDI.

Author/vendor: Simon Heimler

## Features

- CLAP and VST3 exports through NIH-plug
- Chromatic and standard guitar target modes
- Stable and Reactive response modes
- A4 reference pitch parameter from 430 Hz to 450 Hz
- Monophonic YIN-style pitch detection for guitar, voice, and simple pitched sources
- Optional compact egui UI with note, frequency, cents, history graph, and fine-tune meter
- One-note MIDI output state machine

## Build

Install Rust stable and use Cargo from the repository root.

```sh
cargo test
cargo build --release
cargo xtask bundle trace_tuner --release
```

The UI is behind the `gui` feature because egui/baseview requires native windowing packages on Linux.

```sh
cargo build --release --features gui
cargo xtask bundle trace_tuner --release --features gui
```

On Debian/Ubuntu systems, the GUI build may require:

```sh
sudo apt install libx11-xcb-dev libx11-dev libxcb1-dev libgl-dev
```

## Artifacts

NIH-plug writes plugin bundles under:

```text
target/bundled/
```

Expected outputs after bundling:

- `target/bundled/trace_tuner.clap`
- `target/bundled/trace_tuner.vst3`

Direct Cargo builds also produce the platform dynamic library under `target/release/`, but hosts normally expect the bundled CLAP/VST3 layout.

## MIDI Behavior

Trace Tuner tracks one active emitted note.

- Velocity: fixed at `100 / 127`
- Channel: `0`
- Note-on: emitted when a confident target note becomes active
- Note changes: note-off for the previous note is emitted before note-on for the new note at the same processing timestamp
- Silence timeout: about `120 ms` of silence or low confidence before note-off
- Confidence gate: pitch confidence must be at least `0.80`
- Stable mode: requires three confirmed frames before MIDI note changes
- Reactive mode: allows note changes after one confirmed frame

CLAP supports note output directly. VST3 hosts vary in how they expose MIDI/note output from audio effects, so routing may depend on the host.

## Notes

The audio processing path does not modify samples. Analysis buffers are allocated up front and reused during processing.
