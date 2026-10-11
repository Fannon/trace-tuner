// NIH-plug owns the debug allocator when rt-alloc-check is enabled. Run this
// independent check with --no-default-features --features analysis-normalization.
#![cfg(not(feature = "rt-alloc-check"))]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    f32::consts::TAU,
    hint::black_box,
};
use trace_tuner::core::{DetectionAlgorithm, PitchDetector};

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

fn record_allocation() {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        System.realloc(ptr, layout, size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn production_detectors_allocate_nothing_including_first_call_and_algorithm_changes() {
    let mut detector = PitchDetector::new(48_000.0, 4_096);
    for sample_rate in [48_000.0, 96_000.0] {
        let max_window = (sample_rate * 0.12) as usize;
        detector.set_sample_rate(sample_rate, max_window);
        let samples: Vec<_> = (0..max_window)
            .map(|i| (TAU * 82.41 * i as f32 / sample_rate).sin() * 0.4)
            .collect();
        ALLOCATIONS.with(|count| count.set(0));
        TRACKING.with(|tracking| tracking.set(true));
        for _ in 0..10 {
            for algorithm in [
                DetectionAlgorithm::Yin,
                DetectionAlgorithm::Mpm,
                DetectionAlgorithm::Acf,
            ] {
                detector.set_algorithm(algorithm);
                for window_ms in [12, 28, 52, 76, 104, 120] {
                    let length = (sample_rate * window_ms as f32 / 1_000.0) as usize;
                    black_box(detector.detect(black_box(&samples[..length])));
                }
            }
            detector.reset();
        }
        TRACKING.with(|tracking| tracking.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    }
}
