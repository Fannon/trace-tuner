use super::*;
use std::{cell::Cell, f32::consts::TAU, time::Instant};

#[derive(Default)]
struct Host {
    events: Vec<(usize, NoteEvent<()>)>,
    offset: usize,
    latency: Cell<u32>,
}

impl InitContext<TraceTuner> for Host {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute(&self, _: ()) {}
    fn set_latency_samples(&self, samples: u32) {
        self.latency.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}

impl ProcessContext<TraceTuner> for Host {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute_background(&self, _: ()) {}
    fn execute_gui(&self, _: ()) {}
    fn transport(&self) -> &Transport {
        unreachable!("tuner does not need transport")
    }
    fn next_event(&mut self) -> Option<NoteEvent<()>> {
        None
    }
    fn send_event(&mut self, event: NoteEvent<()>) {
        self.events
            .push((self.offset + event.timing() as usize, event));
    }
    fn set_latency_samples(&self, samples: u32) {
        self.latency.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}

fn setup(
    rate: f32,
    algorithm: DetectionAlgorithm,
    response: ResponseMode,
    guitar: bool,
) -> (TraceTuner, Host) {
    let mut plugin = TraceTuner::default();
    let params = Arc::get_mut(&mut plugin.params).unwrap();
    params.algorithm = EnumParam::new("Algorithm", algorithm);
    params.response = EnumParam::new("Response", response);
    params.mode = EnumParam::new(
        "Mode",
        if guitar {
            TunerMode::Guitar
        } else {
            TunerMode::Chromatic
        },
    );
    let mut host = Host {
        events: Vec::with_capacity(256),
        latency: Cell::new(u32::MAX),
        ..Host::default()
    };
    assert!(plugin.initialize(
        &TraceTuner::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: rate,
            min_buffer_size: None,
            max_buffer_size: 512,
            process_mode: ProcessMode::Realtime,
        },
        &mut host
    ));
    assert_eq!(host.latency.get(), 0);
    (plugin, host)
}

fn tone(rate: f32, frequency: f32, seconds: f32, pluck: bool) -> Vec<f32> {
    (0..(rate * seconds) as usize)
        .map(|i| {
            let t = i as f32 / rate;
            let phase = TAU * frequency * t;
            if pluck {
                // Deterministic attack, decay, harmonics and a small noise component.
                let attack = (t / 0.002).min(1.0);
                let noise = ((i * 7919 % 101) as f32 / 101.0 - 0.5) * 0.002;
                attack
                    * (-t * 2.0).exp()
                    * (0.25 * phase.sin()
                        + 0.12 * (2.0 * phase).sin()
                        + 0.07 * (3.0 * phase).sin()
                        + noise)
            } else {
                0.4 * phase.sin()
            }
        })
        .collect()
}

fn feed(
    plugin: &mut TraceTuner,
    host: &mut Host,
    samples: &[f32],
    blocks: &[usize],
    stereo: u8,
) -> Vec<f64> {
    let mut timings = Vec::new();
    let mut position = 0;
    let mut block_index = 0;
    while position < samples.len() {
        let length = blocks[block_index % blocks.len()].min(samples.len() - position);
        let mut left = samples[position..position + length].to_vec();
        let mut right: Vec<f32> = left
            .iter()
            .map(|&sample| if stereo == 1 { -sample } else { sample })
            .collect();
        if stereo == 2 {
            left.fill(0.0);
        }
        let before_left: Vec<_> = left.iter().map(|sample| sample.to_bits()).collect();
        let before_right: Vec<_> = right.iter().map(|sample| sample.to_bits()).collect();
        let mut buffer = Buffer::default();
        // Both slices remain live and equally sized until process returns.
        unsafe {
            buffer.set_slices(length, |channels| {
                channels.push(&mut left);
                channels.push(&mut right);
            });
        }
        let mut auxiliary = AuxiliaryBuffers {
            inputs: &mut [],
            outputs: &mut [],
        };
        let event_start = host.events.len();
        let start = Instant::now();
        #[cfg(not(feature = "rt-alloc-check"))]
        allocation_check::begin();
        plugin.process(&mut buffer, &mut auxiliary, host);
        #[cfg(not(feature = "rt-alloc-check"))]
        assert_eq!(allocation_check::end(), 0, "process allocated");
        timings.push(start.elapsed().as_secs_f64() * 1_000.0);
        drop(buffer);
        assert!(host.events[event_start..]
            .iter()
            .all(|(_, event)| event.timing() < length as u32));
        assert!(left
            .iter()
            .zip(&before_left)
            .all(|(sample, bits)| sample.to_bits() == *bits));
        assert!(right
            .iter()
            .zip(&before_right)
            .all(|(sample, bits)| sample.to_bits() == *bits));
        position += length;
        host.offset += length;
        block_index += 1;
    }
    timings
}

fn note_ons(host: &Host) -> Vec<(usize, u8)> {
    host.events
        .iter()
        .filter_map(|(time, event)| match event {
            NoteEvent::NoteOn { note, .. } => Some((*time, *note)),
            _ => None,
        })
        .collect()
}

#[test]
fn guitar_targets_do_not_change_fretted_midi_pitch() {
    let (mut plugin, mut host) = setup(48_000.0, DetectionAlgorithm::Yin, ResponseMode::Fast, true);
    feed(
        &mut plugin,
        &mut host,
        &tone(48_000.0, 440.0, 0.15, false),
        &[128],
        0,
    );
    assert_eq!(note_ons(&host), vec![(575, 69)]);
    assert_eq!(plugin.shared_state.snapshot().midi_note, 64); // nearest open string for display
}

#[test]
fn changing_reference_pitch_does_not_blend_old_tuning_errors() {
    let (mut plugin, mut host) = setup(
        48_000.0,
        DetectionAlgorithm::Yin,
        ResponseMode::Stable,
        false,
    );
    let samples = tone(48_000.0, 440.0, 0.15, false);
    feed(&mut plugin, &mut host, &samples, &[128], 0);
    Arc::get_mut(&mut plugin.params).unwrap().reference_pitch = FloatParam::new(
        "A4",
        450.0,
        FloatRange::Linear {
            min: 430.0,
            max: 450.0,
        },
    );
    feed(&mut plugin, &mut host, &samples, &[128], 0);
    let snapshot = plugin.shared_state.snapshot();
    assert!(snapshot.active);
    assert!((snapshot.cents - core::cents_between(snapshot.frequency_hz, 450.0)).abs() < 0.1);
    assert_eq!(snapshot.target_frequency_hz, 450.0);
    assert_eq!(note_ons(&host).len(), 1);
}

#[test]
fn stereo_phase_and_single_connected_channel_do_not_prevent_tracking() {
    for stereo in [0, 1, 2] {
        let (mut plugin, mut host) =
            setup(48_000.0, DetectionAlgorithm::Mpm, ResponseMode::Fast, false);
        feed(
            &mut plugin,
            &mut host,
            &tone(48_000.0, 110.0, 0.2, false),
            &[128],
            stereo,
        );
        let notes = note_ons(&host);
        assert_eq!(notes.len(), 1, "stereo={stereo}: {notes:?}");
        assert_eq!(notes[0].1, 45);
        assert!(notes[0].0 < 48_000 / 30);
    }
}

#[cfg(feature = "analysis-normalization")]
#[test]
fn quiet_bass_and_guitar_acquire_without_changing_audio() {
    for frequency in [41.2034, 82.4069, 440.0] {
        let (mut plugin, mut host) =
            setup(48_000.0, DetectionAlgorithm::Yin, ResponseMode::Fast, false);
        let mut samples = tone(48_000.0, frequency, 0.2, false);
        for sample in &mut samples {
            *sample *= 0.0125;
        } // 0.005 peak
        feed(&mut plugin, &mut host, &samples, &[128], 0);
        let notes = note_ons(&host);
        assert_eq!(notes.len(), 1, "{frequency}: {notes:?}");
        assert_eq!(
            notes[0].1,
            core::chromatic_note_match(frequency, 440.0)
                .unwrap()
                .midi_note
        );
    }
}

#[test]
fn live_yin_and_mpm_acquire_bass_and_guitar_at_consistent_sample_rates() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for algorithm in [DetectionAlgorithm::Yin, DetectionAlgorithm::Mpm] {
            for (frequency, note, limit_ms) in [
                (30.8677, 23, 110.0),
                (41.2034, 28, 60.0),
                (82.4069, 40, 60.0),
                (440.0, 69, 20.0),
                (1318.51, 88, 20.0),
            ] {
                let (mut plugin, mut host) = setup(rate, algorithm, ResponseMode::Fast, false);
                feed(
                    &mut plugin,
                    &mut host,
                    &tone(rate, frequency, 0.2, true),
                    &[128],
                    0,
                );
                let notes = note_ons(&host);
                assert_eq!(
                    notes.len(),
                    1,
                    "{algorithm:?} {rate} {frequency}: {notes:?}"
                );
                assert_eq!(notes[0].1, note, "{algorithm:?} {rate} {frequency}");
                assert!(
                    notes[0].0 as f32 * 1_000.0 / rate <= limit_ms,
                    "{algorithm:?} {frequency}: {} ms",
                    notes[0].0 as f32 * 1_000.0 / rate
                );
            }
        }
    }
}

#[test]
fn midi_timing_is_independent_of_callback_partitioning_and_releases_after_silence() {
    let mut samples = tone(48_000.0, 110.0, 0.2, false);
    samples.extend(tone(48_000.0, 220.0, 0.2, false));
    samples.resize(samples.len() + 12_000, 0.0);
    let mut expected = None;
    for blocks in [&[128][..], &[32, 64, 17, 256][..], &[512][..]] {
        let (mut plugin, mut host) =
            setup(48_000.0, DetectionAlgorithm::Yin, ResponseMode::Fast, false);
        feed(&mut plugin, &mut host, &samples, blocks, 0);
        let events: Vec<_> = host
            .events
            .iter()
            .map(|(time, event)| {
                (
                    *time,
                    match event {
                        NoteEvent::NoteOn { note, .. } => (true, *note),
                        NoteEvent::NoteOff { note, .. } => (false, *note),
                        _ => unreachable!(),
                    },
                )
            })
            .collect();
        assert_eq!(
            events.iter().map(|(_, note)| *note).collect::<Vec<_>>(),
            vec![(true, 45), (false, 45), (true, 57), (false, 57)]
        );
        assert!(
            events.last().unwrap().0 < (48_000.0 * 0.456) as usize,
            "Fast MIDI release exceeded 56 ms after silence"
        );
        if let Some(expected) = &expected {
            assert_eq!(&events, expected);
        } else {
            expected = Some(events);
        }
    }
}

#[test]
fn non_finite_audio_passes_through_and_reset_clears_detection() {
    let (mut plugin, mut host) =
        setup(48_000.0, DetectionAlgorithm::Yin, ResponseMode::Fast, false);
    feed(&mut plugin, &mut host, &[f32::NAN; 1_024], &[128], 0);
    assert!(host.events.is_empty());
    feed(
        &mut plugin,
        &mut host,
        &tone(48_000.0, 440.0, 0.15, false),
        &[128],
        0,
    );
    assert!(plugin.shared_state.snapshot().active);
    plugin.reset();
    assert!(!plugin.shared_state.snapshot().active);
    assert!(plugin
        .shared_state
        .history()
        .iter()
        .all(|point| point.cents.is_nan()));
}

#[test]
fn gui_reads_coherent_snapshots_during_audio_publication() {
    let state = Arc::new(SharedTunerState::default());
    let writer = state.clone();
    let worker = std::thread::spawn(move || {
        for index in 1..20_000 {
            writer.publish(DetectionSnapshot {
                active: true,
                held: index & 1 == 0,
                frequency_hz: index as f32,
                confidence: 0.9,
                rms: 0.1,
                midi_note: 69,
                target_frequency_hz: 440.0,
                cents: -(index as f32),
            });
        }
    });
    while !worker.is_finished() {
        let snapshot = state.snapshot();
        if snapshot.active {
            assert_eq!(snapshot.frequency_hz, -snapshot.cents);
            assert_eq!(
                snapshot.held,
                (snapshot.frequency_hz as usize).is_multiple_of(2)
            );
        }
        let history = state.history();
        for point in history.iter().filter(|point| point.cents.is_finite()) {
            assert_eq!(point.held, ((-point.cents) as usize).is_multiple_of(2));
            assert_eq!(point.midi_note, 69);
        }
    }
    worker.join().unwrap();
}

#[test]
fn non_periodic_noise_does_not_generate_midi() {
    let samples = noise_samples(24_000);
    for algorithm in [
        DetectionAlgorithm::Yin,
        DetectionAlgorithm::Mpm,
        DetectionAlgorithm::Acf,
    ] {
        let (mut plugin, mut host) = setup(48_000.0, algorithm, ResponseMode::Fast, false);
        feed(&mut plugin, &mut host, &samples, &[128], 0);
        assert!(
            host.events.is_empty(),
            "{algorithm:?} generated notes on noise: {:?}",
            host.events
        );
    }
}

fn noise_samples(length: usize) -> Vec<f32> {
    (0..length as u32)
        .map(|i| {
            let mut hash = i.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
            hash = ((hash >> ((hash >> 28) + 4)) ^ hash).wrapping_mul(277_803_737);
            (((hash >> 22) ^ hash) as f32 / u32::MAX as f32 - 0.5) * 0.2
        })
        .collect()
}

#[test]
#[ignore]
fn measure_live_onset_and_callback_cost() {
    for rate in [48_000.0, 96_000.0] {
        for algorithm in [
            DetectionAlgorithm::Yin,
            DetectionAlgorithm::Mpm,
            DetectionAlgorithm::Acf,
        ] {
            for response in [ResponseMode::Fast, ResponseMode::Stable] {
                for frequency in [30.8677, 41.2034, 82.4069, 110.0, 440.0, 1318.51] {
                    let (mut plugin, mut host) = setup(rate, algorithm, response, false);
                    let samples = tone(rate, frequency, 0.5, true);
                    let mut times = feed(&mut plugin, &mut host, &samples, &[128], 0);
                    let mean = times.iter().sum::<f64>() / times.len() as f64;
                    times.sort_by(f64::total_cmp);
                    let notes = note_ons(&host);
                    println!("{rate:.0},{algorithm:?},{response:?},{frequency:.4},onset_ms={:.3},notes={notes:?},mean_ms={mean:.4},p99_ms={:.4},max_ms={:.4}",
                        notes.first().map_or(f32::NAN, |(time, _)| (*time + 1) as f32 * 1_000.0 / rate), times[(times.len() * 99 / 100).min(times.len()-1)], times[times.len()-1]);
                }
                let (mut plugin, mut host) = setup(rate, algorithm, response, false);
                let samples = noise_samples((rate * 0.5) as usize);
                let mut times = feed(&mut plugin, &mut host, &samples, &[128], 0);
                let mean = times.iter().sum::<f64>() / times.len() as f64;
                times.sort_by(f64::total_cmp);
                println!("{rate:.0},{algorithm:?},{response:?},noise,notes={},mean_ms={mean:.4},p99_ms={:.4},max_ms={:.4}", host.events.len(), times[times.len()*99/100], times[times.len()-1]);
            }
        }
    }
}

#[cfg(not(feature = "rt-alloc-check"))]
mod allocation_check {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };
    thread_local! {
        static TRACKING: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }
    struct Allocator;
    fn record() {
        if TRACKING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNT.try_with(|n| n.set(n.get() + 1));
        }
    }
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record();
            System.alloc(layout)
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record();
            System.alloc_zeroed(layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            record();
            System.realloc(ptr, layout, size)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout);
        }
    }
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;
    pub(super) fn begin() {
        COUNT.with(|n| n.set(0));
        TRACKING.with(|t| t.set(true));
    }
    pub(super) fn end() -> usize {
        TRACKING.with(|t| t.set(false));
        COUNT.with(Cell::get)
    }
}
