# Changelog

## [1.2.0] - 2026-10-11

Trace Tuner 1.2.0 improves live guitar/bass tuning and monophonic MIDI tracking with smaller analysis bursts, faster pitch acquisition, and several tracking fixes.

### Performance and latency

- Replace direct lag loops in YIN, MPM, and ACF with FFT-based linear autocorrelation using preallocated plans and buffers. Local 48 kHz benchmarks reduced detector cost from roughly 1.5–1.7 ms to 0.04–0.05 ms per detection.
- Replace the fixed 4096-sample startup wait with adaptive 12/28/52/76/104/120 ms windows and analysis updates every 4 ms. Fast requires two periods; Stable requires three periods and three confirmations.
- Measured synthetic plucks with YIN/MPM in Fast mode acquire A4 in 12 ms, guitar E2 in 28 ms, bass E1 in 52 ms, and bass B0 in 76 ms at 48/96 kHz. Real instruments, noise, and host routing can increase these times.
- Audio passthrough remains unchanged with zero added audio latency. Pitch recognition still needs several waveform periods.
- Shorten Fast MIDI release from 120 ms to 40 ms of missing detection and use recent-input silence gating so long windows do not prolong old notes. Stable retains its 120 ms timeout.
- Refresh the display every 16 ms in Fast mode and 33 ms in Stable mode; keep smoothing, hold, and history durations consistent across sample rates.

### Tracking fixes

- Extend detection to 28–2000 Hz for standard four/five-string bass and high fretted guitar notes. Use Chromatic mode for bass.
- Emit the actual chromatic MIDI pitch in Guitar mode instead of the nearest open-string target. Open-string targets continue to affect the tuning display.
- Analyze the stronger stereo channel with hysteresis, avoiding opposite-phase cancellation and attenuation from an unconnected channel.
- Improve noisy pitch searches: YIN selects the minimum within the first threshold valley; MPM ignores the positive correlation lobe connected to zero lag.
- Require stronger confidence to switch an active MIDI note, suppressing spurious intermediate notes during abrupt pitch changes.
- Limit lag searches to half the actual window length; apply the correct Hann window to shorter ACF frames.
- Remove DC offsets before pre-emphasis and reject invalid detector input while preserving original audio samples.
- Publish coherent display snapshots and history, clear history on reset, and reset display smoothing after algorithm, target, or reference-pitch changes.
- Repair experimental pYIN by supplying multiple real frames. It remains optional and excluded from normal release builds: the crate allocates during detection and local calls took roughly 90 ms, making it unsuitable for low-latency live use.

### Development and packaging

- Add detector accuracy, noise, streaming MIDI, stereo, state-coherence, allocation, and performance checks. CI includes complete-callback allocation validation.
- Preserve previous local plugin binaries when builds fail, create recoverable snapshots, and default local compilation to two Cargo jobs. Windows builds also limit CPU affinity to two logical CPUs.
- Add a detailed [codebase review](https://github.com/Fannon/trace-tuner/blob/v1.2.0/docs/codebase-review.md) with measurements and remaining improvements.
- Include README, changelog, and license in platform archives; publish SHA-256 checksums alongside downloads. GitHub release notes come from this changelog.

### Mode choices and limitations

All detector modes remain available for evaluation. YIN and MPM are the current strongest live candidates; ACF remains a comparison baseline. Fresh instances retain YIN + Stable defaults; select Fast when evaluating live MIDI tracking.

Tracking is monophonic, with fixed MIDI velocity and discrete notes. Pitch bend and polyphonic tracking are not implemented. Synthetic measurements do not replace testing with recorded instruments or actual Bitwig playback.

Download the archive for your platform and install its CLAP and/or VST3 bundle, then rescan your DAW. Normal downloads include the GUI and YIN/MPM/ACF, with experimental pYIN disabled.

**Full changelog:** [v1.1.3…v1.2.0](https://github.com/Fannon/trace-tuner/compare/v1.1.3...v1.2.0).

[1.2.0]: https://github.com/Fannon/trace-tuner/releases/tag/v1.2.0
