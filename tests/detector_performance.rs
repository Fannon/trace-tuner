use std::{f32::consts::TAU, hint::black_box, time::Instant};
use trace_tuner::core::{DetectionAlgorithm, PitchDetector};

// Run with: cargo test --release --test detector_performance -- --ignored --nocapture
// Timing is diagnostic; wall-clock limits would be unreliable on shared CI machines.
#[test]
#[ignore]
fn production_window_timings() {
    for sample_rate in [48_000.0, 96_000.0] {
        let samples: Vec<_> = (0..4_096)
            .map(|i| (TAU * 82.41 * i as f32 / sample_rate).sin() * 0.4)
            .collect();
        for algorithm in [
            DetectionAlgorithm::Yin,
            DetectionAlgorithm::Mpm,
            DetectionAlgorithm::Acf,
        ] {
            let mut detector = PitchDetector::new(sample_rate, samples.len());
            detector.set_algorithm(algorithm);
            for _ in 0..20 {
                black_box(detector.detect(black_box(&samples)));
            }
            let mut timings = Vec::with_capacity(1_000);
            for _ in 0..1_000 {
                let start = Instant::now();
                black_box(detector.detect(black_box(&samples)));
                timings.push(start.elapsed().as_secs_f64() * 1_000.0);
            }
            let mean = timings.iter().sum::<f64>() / timings.len() as f64;
            timings.sort_by(f64::total_cmp);
            println!(
                "{algorithm:?} at {sample_rate:.0} Hz: mean {mean:.4} ms, p99 {:.4} ms, max {:.4} ms (128-sample deadline {:.4} ms)",
                timings[989], timings[999], 128_000.0 / sample_rate
            );
        }
    }
}
