# Trace Tuner codebase review

Reviewed 2026-10-10 for live guitar/bass tuning and monophonic MIDI tracking. All detector modes remain available until measurements and instrument recordings justify reducing them.

Final validation and Windows bundles completed 2026-10-11. Compilation was limited to two Cargo jobs and two logical CPUs for the final build; local Windows builds now preserve that limit by default.

## Main findings and implemented fixes

| Priority | Finding | Change / status |
| --- | --- | --- |
| High | Direct lag loops caused large audio-thread bursts, exceeding small-buffer deadlines at higher sample rates. | Replaced YIN/MPM/ACF lag accumulation with zero-padded FFT linear autocorrelation. Plans, scratch, and buffers are allocated during initialization. Numerical equivalence is tested against direct sums. |
| High | Every note waited for 4096 samples, regardless of pitch or response mode. | Adaptive 12/28/52/76/104/120 ms windows, analyzed every 4 ms. Fast requires two periods; Stable requires three and three confirmations. |
| High | The 70 Hz lower bound excluded bass E1 and B0; the 1200 Hz upper bound excluded the top fretted guitar E6. | Extended range to 28–2000 Hz. Maximum window duration scales with sample rate. |
| High | Guitar target mapping also remapped MIDI to the nearest open string. A played A4 could emit E4. | MIDI always uses chromatic pitch. Guitar mode continues to select open-string display targets. |
| High | Averaging stereo could cancel an opposite-phase signal or halve a signal connected to only one channel. | Analyze the stronger channel with energy hysteresis; preserve both output channels exactly. |
| High | Short frames could search nearly to their end, creating confident estimates from very little overlap. | Restrict lag search to half the actual frame length. This guarantees at least two periods. |
| High | An abrupt note transition briefly emitted a spurious intermediate MIDI note. | Require 90% confidence when changing an active MIDI note; keep 80% acquisition and same-note tracking. Regression covers an abrupt A2→A3 transition. |
| High | Noise caused YIN to accept an early ripple within a period valley, and MPM to select a noise peak in its zero-lag lobe. | YIN selects the lowest point within the first threshold valley; MPM explicitly skips the positive lobe connected to zero lag. Regression spans bass/guitar frequencies and three sample rates. |
| Medium | Long analysis windows could keep old notes alive after the input became silent. MIDI also used a 120 ms timeout in Fast mode. | Gate on the most recent 12 ms of input. Fast MIDI release now uses 40 ms; Stable retains 120 ms. |
| Medium | Display hold, smoothing, and history duration changed with sample rate / hop size. | Calibrate smoothing and hold to elapsed time; update history at about 12 ms per point independently of 4 ms analysis. |
| Medium | pYIN always decoded one frame, but its Viterbi initialization permits only unvoiced states in the first frame. | Feed three overlapping real frames. Existing pitch and confidence tests now pass. Keep allocation and result destruction within the experimental allocation exemption. |
| Medium | ACF applied the beginning of a maximum-length Hann window to shorter input. | Generate the correct Hann window for the actual frame using a trigonometric recurrence, without allocation or per-sample cosine calls. |
| Medium | DC offsets distorted correlation; invalid samples could contaminate pitch/state. | Remove the frame mean before pre-emphasis, reject non-finite detector input/results through RMS validation, and sanitize only the internal analysis stream. Audio passthrough is unchanged, including NaN bit patterns. |
| Medium | Independent relaxed atomic reads could combine fields from different publications. Reset retained old history. | Versioned atomic snapshots/history with GUI-side retries; audio never waits. Clear history during reset. Concurrent reader/writer regression added. |
| Medium | Changing A4 or display target could mix old tuning errors into the new display. | Reset display smoothing when target configuration or algorithm changes. Preserve independent MIDI tracking. |
| Medium | CI skipped the custom allocation test because default features disabled that test binary. | Add a separate no-default-features normalization test run to CI and release verification. It also checks allocations in the complete process callback. |
| Medium | Build scripts removed working binaries before a build succeeded and depended on the caller's directory. | Build from the script directory; verify both bundles, snapshot them, back up existing binaries, then copy. No recursive deletes. Windows uses a PowerShell implementation. |
| Low | New Rust versions warn on inferred GUI float types and an upstream VST3 macro. | Explicit GUI float types. Scope the upstream macro lint allowance to its generated export module; keep strict linting for repository code. |

The audio callback performs no file access, logging, mutex acquisition, FFT planning, buffer resizing, or heap allocation in the production YIN/MPM/ACF paths. MIDI is emitted at the analysis sample's offset within the current host buffer. Host callbacks and the experimental pYIN path require separate consideration.

## Latency measurements

Audio passthrough adds **zero samples** of delay, explicitly reported to the host. Pitch recognition is a separate delay: it needs enough waveform periods to distinguish the fundamental from harmonics. A guitar E2 period is about 12.1 ms; bass E1 is 24.3 ms; bass B0 is 32.4 ms.

The original fixed window imposed a minimum startup wait of 85.3 ms at 48 kHz and 42.7 ms at 96 kHz. Three-frame Stable confirmation increased those waits to about 106.7 and 53.3 ms respectively. Bass notes below 70 Hz could not acquire at all.

Measured time from the start of a deterministic synthetic pluck to its first MIDI NoteOn:

| Note | Frequency | YIN Fast | MPM Fast | YIN / MPM Stable | ACF Fast |
| --- | ---: | ---: | ---: | ---: | ---: |
| Bass B0 | 30.87 Hz | 76 ms | 76 ms | 112 ms | No acquisition |
| Bass E1 | 41.20 Hz | 52 ms | 52 ms | 84 ms | No acquisition |
| Guitar E2 | 82.41 Hz | 28 ms | 28 ms | 60 ms | 76 ms |
| A2 | 110 Hz | 28 ms | 28 ms | 36 ms | 52 ms |
| A4 | 440 Hz | 12 ms | 12 ms | 20 ms | 28 ms |
| Guitar E6 | 1318.51 Hz | 12 ms | 12 ms | 20 ms | 12 ms |

These timings matched at 48 and 96 kHz with 128-sample callbacks. Tests also cover 44.1 kHz and irregular callback sizes. The pluck has an attack ramp, decay, harmonics, and a small deterministic noise component. It is not a recording of a real instrument. Host MIDI delivery, synth response, audio-interface latency, and GUI presentation add their own delays. The editor requests refresh every 16 ms in Fast mode and 33 ms in Stable mode.

The Fast release regression requires the MIDI NoteOff within 56 ms after actual input silence. The 40 ms timeout begins when recent-input analysis becomes missing, not exactly at the final nonzero sample. Display hold is independent: approximately 128 ms Fast / 1024 ms Stable.

## CPU and real-time behavior

The original detector-only 4096-sample benchmark at 48 kHz averaged approximately 1.55 ms YIN, 1.58 ms MPM, and 1.70 ms ACF per detection. FFT implementations average roughly 0.04 ms on the same machine and window: approximately a 40× reduction. At 96 kHz, original means were approximately 2.98/3.22/3.08 ms, above the 1.333 ms deadline for a 128-sample audio buffer.

The complete adaptive callback has more work on low or ambiguous input because it may try several windows. Representative 96 kHz measurements with 128-sample buffers:

| Input / mode | Mean callback | 99th percentile callback |
| --- | ---: | ---: |
| A4, YIN Fast | ~0.007 ms | ~0.023 ms |
| Guitar E2, YIN Fast | ~0.02 ms | ~0.08 ms |
| Bass B0, YIN Stable | ~0.12 ms | ~0.42 ms |
| Bass E1, ACF Fast, no acquisition | ~0.15 ms | ~0.59 ms |
| Nonperiodic noise, YIN Stable | ~0.17 ms | ~0.67 ms |

These are wall-clock diagnostics from local release builds, including callbacks with and without an analysis event. They are not worst-case scheduling guarantees or a Bitwig playback test. Other plugins, the GUI, operating-system scheduling, very small buffers, and higher sample rates can consume the remaining deadline. Increasing the number of adaptive windows trades some CPU for faster low-note acquisition.

Allocation checks cover first calls, detector switching, all adaptive frame lengths, initialization at 48/96 kHz, reset followed by detection, and complete streaming callbacks. Buffers and mock-host MIDI capacity are allocated before the measured callback.

## Detector choices

**YIN and MPM are the current two strongest live candidates.** Both acquired every tested pluck in the table and tracked strong second harmonics, missing fundamentals, clipping, and DC-offset cases in the comparison harness. MPM had smaller interpolation error on the long synthetic periodic frames. This is evidence to test it alongside YIN, not enough to declare it universally better on actual instruments.

Each comparison profile covers six frequencies at three sample rates using a 120 ms frame. An accepted estimate counts as accurate within 15 cents. On the sine-plus-nonperiodic-noise profile, the peak-search fixes improved YIN from 7/18 to 18/18 accurate estimates (maximum error 7.47 cents) and MPM from 16/18 to 18/18 (6.23 cents). ACF remained at 6/18, with 12 incorrect accepted estimates. On the strong-second-harmonic and missing-fundamental profiles, YIN/MPM each achieved 18/18, while ACF accepted only 3/18 and rejected the rest. These are deterministic synthetic cases, not general accuracy percentages.

The quiet profile bypasses plugin normalization and intentionally falls below raw detector acquisition confidence. Separate complete-callback tests verify quiet guitar/bass acquisition with the normal capped normalization enabled. Nonperiodic noise alone generated no MIDI in the streaming regressions.

**ACF is retained as a comparison baseline.** Hann-window attenuation makes its raw correlation confidence depend strongly on the number of periods present. The shared 80% acquisition threshold rejects bass E1/B0 even in a 120 ms window and forces longer acquisition windows on guitar. Lowering only its threshold might improve acquisition while increasing incorrect pitches; it needs separate calibration and recording-based validation before becoming a live recommendation.

**pYIN remains optional and experimental.** Its crate implementation allocates during detection and builds/decodes a dense transition model. The three-frame repair makes it functional, but does not make it suitable for small-buffer live use. Do not enable it in the normal live build. A future offline/robust mode would need to demonstrate accuracy gains that justify its cost, or replace the decoder with incremental preallocated state.

The repaired pYIN 120 ms / 48 kHz comparison took 88.3, 91.8, and 90.1 ms per call on a harmonic A2 signal. Each call individually exceeds a 128-sample / 48 kHz callback deadline (~2.67 ms) by more than 30×. It also requires the full 120 ms input window before it can produce a result in this wrapper. The normal GUI release does not enable this feature.

Keep `Fast` and `Stable`: these address a useful tracking-versus-tuning tradeoff. Keep `Chromatic` and `Guitar`: these are display target choices, independent of the detector. Bass uses Chromatic; Guitar targets standard six-string open notes. Fresh instances still default to YIN + Stable, preserving the existing default behavior. For live MIDI evaluation, select Fast explicitly.

## Remaining improvements, in order

1. **Recorded guitar/bass corpus and actual Bitwig runs.** Capture DI recordings spanning clean sustained notes, hard picks, soft attacks, dead notes, slides, vibrato, muted transitions, fret buzz, hum, and different pickup/playing positions. Compare onset/transition/release latency, wrong first notes, octave errors, rejected notes, cents error, and CPU tails. Include five-string bass and the intended lowest tuning. The synthetic harness is useful for regression but cannot establish instrument accuracy.
2. **Frequency-dependent confidence and octave protection.** Confidence is a detector heuristic, not a calibrated probability. Calibrate thresholds separately for each detector and short-frame length. Test weak fundamentals before adding a prior-note constraint or YIN/MPM agreement check: either can hide legitimate octave jumps. The current single-frame Fast switching remains susceptible to very confident false pitches on difficult material.
3. **Reduce work on uncertain input and high sample rates.** Consider a fixed analysis sample rate with a proper antialiasing filter, or tracking-informed window selection with an explicit reacquisition fallback. Measure filter delay and interpolation error before adopting either. Current adaptive analysis can run up to six FFTs every 4 ms; CPU/memory grow with host sample rate. Performance validation here focuses on 44.1/48/96 kHz, not 192/384 kHz.
4. **MIDI expressiveness.** Add pitch bend and configurable velocity/release only if the tracking workflow needs them. Current output is monophonic, fixed velocity, and discrete note numbers; it does not reproduce continuous guitar bends. A target/string display is not a substitute for MIDI pitch tracking.
5. **Mode/state compatibility before pruning.** Stable enum IDs are retained, but the optional pYIN variant changes the number/order of normalized algorithm values. Host automation may interpret values differently between feature builds. Design a compatibility strategy and saved-session/automation tests before removing/reordering modes or changing their feature availability.
6. **Host and release coverage.** Run Bitwig at the screenshot's 48 kHz/128 samples, then 64/32 samples and 96 kHz, with UI open/closed and multiple instances. Existing CI validates VST3 on Windows; add equivalent CLAP validation and saved-state/MIDI routing checks. Test macOS/Linux builds in CI. The local Windows build used a separate GNU toolchain because MSVC C++ build tools were unavailable; native loading checks do not replace host validation.
7. **Maintenance.** Deduplicate CI/release verification when it becomes cumbersome; choose a reproducible supported Rust toolchain and explicit dependency-update policy. The upstream VST3 macro and `proc-macro-error2` still have future-compiler compatibility work. Build-script snapshots recover failed copies, but copying two bundles is not an atomic deployment and host file locks can still prevent replacement.

There are no real instrument recordings or project-level host benchmarks in this repository. Polyphonic tracking, tunings below 28 Hz, and automatic selection between independent stereo instruments are outside the tested behavior. Channel selection uses callback energy with hysteresis; switching channels with different signals/phases can introduce an analysis discontinuity, while leaving audio untouched.

## Reproduce the checks

Completed locally: formatting and whitespace checks, strict all-target/all-feature Clippy, 82 passing all-feature regression tests, and 84 passing checks in the separate production/allocation run including its diagnostic benchmarks. GUI-enabled CLAP/VST3 bundles passed native Windows loading and export checks; copied `bin/` binaries match the bundled SHA-256 hashes. The normal bundles exclude experimental pYIN. Existing binaries were preserved under `tmp/before_latency_audit_20261011_061723_101/`; new snapshots are under `tmp/release_20261011_061723_101/`.

Bitwig playback and macOS/Linux host validation remain unverified locally. Native loading verifies dependency resolution and entry points, not host processing or MIDI routing.

```sh
cargo fmt --check
cargo test --release --all-features
cargo clippy --release --all-targets --all-features -- -D warnings
cargo test --release --no-default-features --features analysis-normalization
cargo test --release --no-default-features --features analysis-normalization --lib measure_live_onset_and_callback_cost -- --ignored --nocapture
cargo test --release --test detector_comparison -- --ignored --nocapture
cargo test --release --test detector_performance -- --ignored --nocapture
cargo test --release --features pyin-detector --test detector_comparison experimental_pyin_cost -- --ignored --nocapture
```

Strict linting allows only the known upstream VST3 macro warning, scoped in source to generated exports. Timing tests are ignored by default and deliberately have no machine-dependent pass/fail deadline. Allocation checks use `--no-default-features` because NIH-plug owns the allocator with `rt-alloc-check` enabled; GNU Windows also requires release mode for that upstream feature.

Relevant implementation: [streaming and shared state](../src/lib.rs), [detectors and MIDI](../src/core.rs), [FFT correlation](../src/autocorrelation.rs), [process regressions and measurement harness](../src/stream_tests.rs), [detector comparison](../tests/detector_comparison.rs). Local measurement and validation logs are under ignored `tmp/audit-*.txt`.
