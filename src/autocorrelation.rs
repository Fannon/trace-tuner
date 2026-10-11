use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

type FftPair = (Arc<dyn Fft<f32>>, Arc<dyn Fft<f32>>);

/// Linear autocorrelation with all FFT planning and storage allocated up front.
pub(crate) struct Autocorrelation {
    plans: Vec<FftPair>,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    energy_prefix: Vec<f64>,
}

impl Autocorrelation {
    pub(crate) fn new(max_window_samples: usize) -> Self {
        // Padding to at least 2N prevents the end of the window wrapping into
        // the beginning when transforming the power spectrum back to lags.
        let fft_len = (2 * max_window_samples).max(1).next_power_of_two();
        let mut planner = FftPlanner::new();
        let plans: Vec<_> = (0..=fft_len.ilog2())
            .map(|power| {
                let len = 1 << power;
                (planner.plan_fft_forward(len), planner.plan_fft_inverse(len))
            })
            .collect();
        let scratch_len = plans.iter().fold(0, |size, (forward, inverse)| {
            size.max(forward.get_inplace_scratch_len())
                .max(inverse.get_inplace_scratch_len())
        });
        Self {
            plans,
            buffer: vec![Complex::default(); fft_len],
            scratch: vec![Complex::default(); scratch_len],
            energy_prefix: vec![0.0; max_window_samples + 1],
        }
    }

    pub(crate) fn compute(&mut self, samples: &[f32], lags: &mut [f32]) {
        let fft_len = (2 * samples.len()).max(1).next_power_of_two();
        let (forward, inverse) = &self.plans[fft_len.ilog2() as usize];
        let buffer = &mut self.buffer[..fft_len];
        buffer.fill(Complex::default());
        self.energy_prefix[0] = 0.0;
        for (i, &sample) in samples.iter().enumerate() {
            buffer[i].re = sample;
            self.energy_prefix[i + 1] =
                self.energy_prefix[i] + f64::from(sample) * f64::from(sample);
        }
        forward.process_with_scratch(buffer, &mut self.scratch);
        for bin in buffer.iter_mut() {
            *bin = Complex::new(bin.norm_sqr(), 0.0);
        }
        inverse.process_with_scratch(buffer, &mut self.scratch);
        let scale = 1.0 / fft_len as f32;
        for (lag, bin) in lags.iter_mut().zip(buffer.iter()) {
            *lag = bin.re * scale;
        }
        // Use the directly accumulated energy for zero lag and normalization.
        lags[0] = self.energy_prefix[samples.len()] as f32;
    }

    pub(crate) fn overlapping_energy(&self, n: usize, tau: usize) -> (f32, f32) {
        (
            self.energy_prefix[n - tau] as f32,
            (self.energy_prefix[n] - self.energy_prefix[tau]) as f32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_direct_linear_correlation_and_overlapping_energy() {
        let mut correlation = Autocorrelation::new(4_096);
        // Reuse the same buffers across different window lengths, including a
        // non-power-of-two length, to catch stale samples in the padded tail.
        for n in [4_096, 333, 2_048, 64] {
            let samples: Vec<f32> = (0..n)
                .map(|i| {
                    let noise = (i * 7919 % 101) as f32 / 101.0 - 0.5;
                    ((i as f32 * 0.17).sin() + noise) * (0.1 + i as f32 / n as f32)
                })
                .collect();
            let mut lags = vec![0.0; n / 2 + 1];
            correlation.compute(&samples, &mut lags);
            let tolerance = lags[0] * 0.00001;
            for (tau, &actual) in lags.iter().enumerate() {
                let mut expected = 0.0_f64;
                let mut front = 0.0_f64;
                let mut back = 0.0_f64;
                let mut difference = 0.0_f64;
                for i in 0..n - tau {
                    let a = f64::from(samples[i]);
                    let b = f64::from(samples[i + tau]);
                    expected += a * b;
                    front += a * a;
                    back += b * b;
                    difference += (a - b).powi(2);
                }
                assert!(
                    (actual - expected as f32).abs() < tolerance,
                    "n={n}, tau={tau}"
                );
                let (actual_front, actual_back) = correlation.overlapping_energy(n, tau);
                assert!((actual_front - front as f32).abs() < tolerance);
                assert!((actual_back - back as f32).abs() < tolerance);
                let fft_difference = (actual_front + actual_back - 2.0 * actual).max(0.0);
                assert!((fft_difference - difference as f32).abs() < 3.0 * tolerance);
            }
        }
    }

    #[test]
    fn window_edges_do_not_wrap_into_correlation() {
        let mut correlation = Autocorrelation::new(64);
        let mut samples = [0.0; 64];
        samples[0] = 1.0;
        samples[63] = 1.0;
        let mut lags = [0.0; 33];
        correlation.compute(&samples, &mut lags);
        assert_eq!(lags[0], 2.0);
        assert!(lags[1..].iter().all(|lag| lag.abs() < 0.00001));
    }
}
