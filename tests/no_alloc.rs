//! The audio path never allocates. Every filter type, slope and placement is
//! processed, switched and glided with the allocator armed to abort, while an
//! automatic EQ capture records; block sizes vary from 1 sample upwards.

use assert_no_alloc::assert_no_alloc;
use graphq::dsp::eq::{Eq, NUM_BANDS};
use graphq::dsp::filter::{Band, FilterType, Placement, Slope};
use graphq::dsp::shared::Shared;

// nih-plug's `assert_process_allocs` feature already installs the checking
// global allocator, so it is active in this test binary too.

#[test]
fn processing_and_switching_does_not_allocate() {
    let mut eq = Eq::new(48_000.0);
    let shared = Shared::default();
    let mut l: Vec<f32> = (0..2048).map(|i| ((i * 37 % 101) as f32 / 50.0 - 1.0) * 0.5).collect();
    let mut r = l.clone();
    let mut bands = [Band::default(); NUM_BANDS];
    let capture = shared.start_capture(1000.0);
    assert_no_alloc(|| {
        for (k, block) in [1usize, 16, 33, 128, 512, 2048].into_iter().cycle().take(96).enumerate() {
            for (b, band) in bands.iter_mut().enumerate() {
                *band = Band {
                    on: (k + b) % 5 != 0,
                    kind: FilterType::ALL[(k + b) % 8],
                    freq: 30.0 * (1.0 + ((k * 7 + b * 13) % 600) as f32 * 0.03).powi(2),
                    gain: ((k + b) % 13) as f32 - 6.0,
                    q: 0.3 + ((k + b) % 9) as f32 * 0.7,
                    slope: Slope::ALL[(k / 3 + b) % 6],
                    place: Placement::ALL[(k / 5 + b) % 5],
                };
            }
            eq.set_params(&bands, (k % 7) as f32 - 3.0);
            shared.push_capture(&l[..block], &r[..block]);
            eq.process(&mut l[..block], &mut r[..block]);
            shared.pre.push_stereo(&l[..block], &r[..block]);
            shared.add_peaks(&l[..block], &r[..block]);
        }
    });
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    assert!(capture.is_full());
}
