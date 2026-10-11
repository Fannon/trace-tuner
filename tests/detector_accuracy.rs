use std::f32::consts::TAU;
use trace_tuner::core::{cents_between, DetectionAlgorithm, PitchDetector};

#[test]
fn legacy_4096_window_tracks_pitch_across_rates_and_algorithms() {
    for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
        for frequency in [82.41, 110.0, 220.0, 440.0, 880.0, 1_100.0] {
            // MPM requires enough periods in the fixed 4096-sample window.
            if sample_rate == 96_000.0 && frequency < 220.0 {
                continue;
            }
            let samples: Vec<_> = (0..4_096)
                .map(|i| (TAU * frequency * i as f32 / sample_rate).sin() * 0.4)
                .collect();
            let mut detector = PitchDetector::new(sample_rate, samples.len());
            for algorithm in [
                DetectionAlgorithm::Yin,
                DetectionAlgorithm::Mpm,
                DetectionAlgorithm::Acf,
            ] {
                detector.set_algorithm(algorithm);
                let estimate = detector.detect(&samples).unwrap_or_else(|| {
                    panic!("{algorithm:?}: no pitch for {frequency} Hz at {sample_rate} Hz")
                });
                let error = cents_between(estimate.frequency_hz, frequency).abs();
                assert!(
                    error < 15.0,
                    "{algorithm:?}: {error} cents error for {frequency} Hz at {sample_rate} Hz"
                );
            }
        }
    }
}

#[test]
fn detector_reinitialization_resizes_fft_storage() {
    let mut detector = PitchDetector::new(48_000.0, 2_048);
    for n in [4_096, 1_024, 2_048] {
        detector.set_sample_rate(44_100.0, n);
        detector.reset();
        let samples: Vec<_> = (0..n)
            .map(|i| (TAU * 440.0 * i as f32 / 44_100.0).sin() * 0.4)
            .collect();
        let estimate = detector.detect(&samples).unwrap();
        assert!(cents_between(estimate.frequency_hz, 440.0).abs() < 1.0);
    }
}
