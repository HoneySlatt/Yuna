//! Property tests: springs and curves with random parameters, distances, velocities and starts,
//! retargeted at random times, sampled at random times. Every spring and curve made on the way:
//!
//! - gives finite values and velocities, never NaN;
//! - ends at a finite time, not before its start: within an hour of it for a spring (the
//!   slowest the parameters allow, ζ = 20 at stiffness 1, ends a 2·10⁵-unit move to the finest
//!   tolerance here, 1/510, in about 12 minutes), within 10 s for a curve;
//! - is exactly at its target at its end and after;
//! - takes over from the one it retargets without a jump in value (and, for a spring, in
//!   velocity).
//!
//! A failure shrinks to the smallest case that still fails; proptest keeps its seed in
//! `properties.proptest-regressions`, to be committed so that the case runs again every time.

use std::time::Duration;

use proptest::prelude::*;
use yuna_motion::{Curve, CurveParams, Easing, Spring, SpringParams};

/// Where in a motion's life a sample or a retarget falls: a share of the life, up to 1.5 so that
/// some fall after the end.
fn share_of_life() -> impl Strategy<Value = f64> {
    0.0..1.5
}

/// A time `share` of the way from `start` to `end`.
fn time_in(start: Duration, end: Duration, share: f64) -> Duration {
    start + (end - start).mul_f64(share)
}

/// Spring parameters over their whole range, the stiffness spread over its decades.
fn spring_params() -> impl Strategy<Value = SpringParams> {
    (
        prop_oneof![0.05..=20.0, 0.9..1.1, Just(1.0)],
        (0.0..=5.0_f64).prop_map(|decades| 10.0_f64.powf(decades)),
    )
        .prop_map(|(ratio, stiffness)| SpringParams::new(ratio, stiffness).unwrap())
}

/// Half a physical pixel at the usual scales, in logical units; and a fine one, for an alpha.
fn tolerance() -> impl Strategy<Value = f64> {
    prop_oneof![Just(0.5), Just(0.25), Just(1.0 / 3.0), Just(0.5 / 255.0)]
}

fn start() -> impl Strategy<Value = Duration> {
    (0u64..1_000_000_000).prop_map(Duration::from_micros)
}

fn easing() -> impl Strategy<Value = Easing> {
    prop_oneof![
        Just(Easing::Linear),
        Just(Easing::EaseOutQuad),
        Just(Easing::EaseOutCubic),
        Just(Easing::EaseOutExpo),
        (0.0..=1.0, -2.0..3.0, 0.0..=1.0, -2.0..3.0)
            .prop_map(|(x1, y1, x2, y2)| Easing::CubicBezier { x1, y1, x2, y2 }),
    ]
}

/// What holds for every spring: finite samples, an end not before its start, exactly the
/// target from the end on.
fn check_spring(spring: &Spring, start: Duration, samples: &[f64]) -> Result<(), TestCaseError> {
    let end = spring.end();
    prop_assert!(end >= start, "ends at {end:?} before its start {start:?}");
    prop_assert!(
        end - start < Duration::from_secs(3600),
        "ends at {end:?}, from {start:?}"
    );
    prop_assert_eq!(spring.value(end), spring.target());
    prop_assert_eq!(spring.velocity(end), 0.0);
    prop_assert!(spring.is_done(end));
    for &share in samples {
        let at = time_in(start, end, share);
        let (value, velocity) = (spring.value(at), spring.velocity(at));
        prop_assert!(
            value.is_finite() && velocity.is_finite(),
            "{value} {velocity} at {at:?}"
        );
        if at >= end {
            prop_assert_eq!(value, spring.target());
        }
    }
    Ok(())
}

/// What holds for every curve, as for springs.
fn check_curve(curve: &Curve, start: Duration, samples: &[f64]) -> Result<(), TestCaseError> {
    let end = curve.end();
    prop_assert!(end >= start);
    prop_assert!(end - start <= Duration::from_secs(10));
    prop_assert_eq!(curve.value(end), curve.target());
    for &share in samples {
        let at = time_in(start, end, share);
        let value = curve.value(at);
        prop_assert!(value.is_finite(), "{value} at {at:?}");
        if at >= end {
            prop_assert_eq!(value, curve.target());
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        ..ProptestConfig::default()
    })]

    #[test]
    fn springs_stay_finite_and_end_at_their_target(
        params in spring_params(),
        from in -1e5..1e5,
        to in -1e5..1e5,
        velocity in prop_oneof![-1e4..1e4, Just(0.0)],
        tolerance in tolerance(),
        start in start(),
        retargets in prop::collection::vec((share_of_life(), -1e5..1e5), 0..6),
        samples in prop::collection::vec(share_of_life(), 1..12),
    ) {
        let mut spring = Spring::new(params, from, to, velocity, start, tolerance);
        let mut spring_start = start;
        check_spring(&spring, spring_start, &samples)?;
        for (share, target) in retargets {
            let at = time_in(spring_start, spring.end(), share);
            let next = spring.retarget(at, target);
            prop_assert!(next.end() >= at);
            if next.end() > at {
                prop_assert_eq!(next.value(at), spring.value(at));
                prop_assert_eq!(next.velocity(at), spring.velocity(at));
            }
            spring = next;
            spring_start = at;
            check_spring(&spring, spring_start, &samples)?;
        }
    }

    #[test]
    fn curves_stay_finite_and_end_at_their_target(
        easing in easing(),
        duration in (1_000u64..=10_000_000).prop_map(Duration::from_micros),
        from in -1e5..1e5,
        to in -1e5..1e5,
        start in start(),
        retargets in prop::collection::vec((share_of_life(), -1e5..1e5), 0..6),
        samples in prop::collection::vec(share_of_life(), 1..12),
    ) {
        let params = CurveParams::new(easing, duration).unwrap();
        let mut curve = Curve::new(params, from, to, start);
        let mut curve_start = start;
        check_curve(&curve, curve_start, &samples)?;
        for (share, target) in retargets {
            let at = time_in(curve_start, curve.end(), share);
            let next = curve.retarget(at, target);
            prop_assert!(next.end() >= at);
            if next.end() > at {
                prop_assert_eq!(next.value(at), curve.value(at));
            }
            curve = next;
            curve_start = at;
            check_curve(&curve, curve_start, &samples)?;
        }
    }
}
