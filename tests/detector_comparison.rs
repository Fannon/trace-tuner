use std::f32::consts::TAU;
use trace_tuner::core::{cents_between, DetectionAlgorithm, PitchDetector};

fn signal(rate: f32, frequency: f32, profile: usize) -> Vec<f32> {
    (0..(rate * 0.12) as usize)
        .map(|i| {
            let phase = TAU * frequency * i as f32 / rate;
            let mut hash = (i as u32)
                .wrapping_mul(747_796_405)
                .wrapping_add(2_891_336_453);
            hash = ((hash >> ((hash >> 28) + 4)) ^ hash).wrapping_mul(277_803_737);
            let noise = ((hash >> 22) ^ hash) as f32 / u32::MAX as f32 - 0.5;
            match profile {
                0 => 0.4 * phase.sin(),
                1 => 0.25 * phase.sin() + 0.12 * (2.0 * phase).sin() + 0.07 * (3.0 * phase).sin(),
                2 => 0.08 * phase.sin() + 0.3 * (2.0 * phase).sin() + 0.12 * (3.0 * phase).sin(),
                3 => 0.3 * (2.0 * phase).sin() + 0.12 * (3.0 * phase).sin(),
                4 => (0.8 * phase.sin()).clamp(-0.2, 0.2),
                5 => 0.25 * phase.sin() + 0.08 * noise,
                6 => 0.3 * phase.sin() + 0.5, // DC offset
                7 => 0.005 * phase.sin(),     // below acquisition level without normalization
                _ => unreachable!(),
            }
        })
        .collect()
}

#[test]
fn dc_offset_and_non_finite_input_are_handled() {
    for algorithm in [
        DetectionAlgorithm::Yin,
        DetectionAlgorithm::Mpm,
        DetectionAlgorithm::Acf,
    ] {
        let mut detector = PitchDetector::new(48_000.0, 5_760);
        detector.set_algorithm(algorithm);
        let pitch = detector.detect(&signal(48_000.0, 110.0, 6)).unwrap();
        assert!(
            cents_between(pitch.frequency_hz, 110.0).abs() < 15.0,
            "{algorithm:?}: {pitch:?}"
        );
        assert!(detector.detect(&[f32::NAN; 64]).is_none());
        assert!(detector.detect(&[f32::INFINITY; 64]).is_none());
        assert!(detector.detect(&[f32::MAX; 64]).is_none());
    }
}

#[test]
fn noisy_instrument_pitch_does_not_lock_to_ripples_or_initial_lobes() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for algorithm in [DetectionAlgorithm::Yin, DetectionAlgorithm::Mpm] {
            let mut detector = PitchDetector::new(rate, (rate * 0.12) as usize);
            detector.set_algorithm(algorithm);
            for frequency in [30.8677, 41.2034, 82.4069, 110.0, 440.0, 1318.51] {
                let pitch = detector.detect(&signal(rate, frequency, 5)).unwrap();
                let error = cents_between(pitch.frequency_hz, frequency).abs();
                assert!(
                    error < 15.0,
                    "{algorithm:?}, {rate}, {frequency}: {pitch:?}, {error} cents"
                );
            }
        }
    }
}

#[test]
#[ignore]
fn compare_modes_on_synthetic_instrument_signals() {
    let profiles = [
        "sine",
        "harmonics",
        "strong_second",
        "missing_fundamental",
        "clipped",
        "noise",
        "dc_offset",
        "quiet",
    ];
    for algorithm in [
        DetectionAlgorithm::Yin,
        DetectionAlgorithm::Mpm,
        DetectionAlgorithm::Acf,
    ] {
        for (profile, name) in profiles.iter().enumerate() {
            let mut accurate = 0;
            let mut wrong = 0;
            let mut rejected = 0;
            let mut max_error = 0.0_f32;
            for rate in [44_100.0, 48_000.0, 96_000.0] {
                let mut detector = PitchDetector::new(rate, (rate * 0.12) as usize);
                detector.set_algorithm(algorithm);
                for frequency in [30.8677, 41.2034, 82.4069, 110.0, 440.0, 1318.51] {
                    match detector
                        .detect(&signal(rate, frequency, profile))
                        .filter(|pitch| pitch.confidence >= 0.8)
                    {
                        Some(pitch) => {
                            let error = cents_between(pitch.frequency_hz, frequency).abs();
                            max_error = max_error.max(error);
                            if error <= 15.0 {
                                accurate += 1;
                            } else {
                                wrong += 1;
                            }
                        }
                        None => rejected += 1,
                    }
                }
            }
            println!("{algorithm:?},{name},accurate={accurate},wrong={wrong},rejected={rejected},max_accepted_error_cents={max_error:.2}");
        }
    }
}

#[cfg(feature = "pyin-detector")]
#[test]
#[ignore]
fn experimental_pyin_cost() {
    let rate = 48_000.0;
    let samples = signal(rate, 110.0, 1);
    let mut detector = PitchDetector::new(rate, samples.len());
    detector.set_algorithm(DetectionAlgorithm::Pyin);
    for _ in 0..3 {
        let start = std::time::Instant::now();
        let estimate = detector.detect(&samples);
        println!(
            "pYIN 120 ms window: {:.3} ms per call, {estimate:?}",
            start.elapsed().as_secs_f64() * 1_000.0
        );
    }
}
