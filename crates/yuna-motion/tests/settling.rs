//! How long a spring takes to settle, checked against the spring's own values sampled finely: an
//! over-damped spring's end is the last moment it is farther than the tolerance from its target,
//! to a sample's step. The samples read the same motion with a tolerance too fine to end it early.

use std::time::Duration;

use yuna_motion::{Spring, SpringParams};

const TOLERANCE: f64 = 0.5;
const STEP: f64 = 0.0002;

/// The last time, in seconds from the start, the spring is farther than the tolerance from its
/// target, by sampling.
fn last_time_away(spring: &Spring, start: Duration) -> f64 {
    let mut last = 0.0;
    let mut t = 0.0;
    while t < 30.0 {
        if (spring.value(start + Duration::from_secs_f64(t)) - spring.target()).abs() > TOLERANCE {
            last = t;
        }
        t += STEP;
    }
    last
}

#[test]
fn an_over_damped_spring_ends_when_it_last_leaves_the_tolerance() {
    let start = Duration::from_secs(1);
    // (damping ratio, stiffness, distance, velocity): over-damped, from rest and with a push
    // either way.
    let cases = [
        (4.0, 1000.0, 300.0, 0.0),
        (2.0, 800.0, 500.0, 0.0),
        (8.0, 5000.0, 100.0, 0.0),
        (3.0, 800.0, 300.0, -2000.0),
        (3.0, 800.0, 300.0, 2000.0),
        (1.5, 300.0, -250.0, 600.0),
        (6.0, 200.0, 40.0, -300.0),
    ];
    // Pushed at the fast rate toward the target, the slow mode has no weight at all: only the fast
    // mode, which dies at e^(−fast·t), is left to settle.
    let fast_rate = |ratio: f64, stiffness: f64| {
        let omega = f64::sqrt(stiffness);
        omega * (ratio + f64::sqrt(ratio * ratio - 1.0))
    };
    let pure_fast =
        [(3.0, 800.0, 123.7), (5.0, 400.0, -61.3)].map(|(ratio, stiffness, distance)| {
            (
                ratio,
                stiffness,
                distance,
                -fast_rate(ratio, stiffness) * distance,
            )
        });
    for (ratio, stiffness, distance, velocity) in cases.into_iter().chain(pure_fast) {
        let params = SpringParams::new(ratio, stiffness).unwrap();
        let spring = Spring::new(params, distance, 0.0, velocity, start, TOLERANCE);
        // The same motion with a tolerance too fine to cut it short, which the samples read.
        let uncut = Spring::new(params, distance, 0.0, velocity, start, 1e-9);
        let end = (spring.end() - start).as_secs_f64();
        let last = last_time_away(&uncut, start);
        assert!(
            (end - last).abs() < 2.0 * STEP,
            "damping {ratio}, stiffness {stiffness}, from {distance} at {velocity}: ends at {end}, last away at {last}"
        );
    }
}
