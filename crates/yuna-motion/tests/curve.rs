//! Curves: the easings are their functions, a Bézier follows CSS, the ends are exact, and a
//! retarget takes the share of the duration that the distance left takes.

use std::time::Duration;

use yuna_motion::{Curve, CurveParams, Easing};

const START: Duration = Duration::from_millis(7_250);
const SECOND: Duration = Duration::from_secs(1);

/// CSS's `ease`.
const EASE: Easing = Easing::CubicBezier {
    x1: 0.25,
    y1: 0.1,
    x2: 0.25,
    y2: 1.0,
};

/// CSS's `cubic-bezier(x1, y1, x2, y2)`.
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64) -> Easing {
    Easing::CubicBezier { x1, y1, x2, y2 }
}

/// A curve from 0 to 1 over a second, whose value is the easing's output.
fn unit_curve(easing: Easing) -> Curve {
    Curve::new(CurveParams::new(easing, SECOND).unwrap(), 0.0, 1.0, START)
}

/// The output of a curve from 0 to 1 over a second at progress `p`.
fn output(easing: Easing, p: f64) -> f64 {
    unit_curve(easing).value(START + SECOND.mul_f64(p))
}

/// The y of CSS's cubic Bézier at progress `p`, solved apart by plain bisection.
fn reference_bezier(easing: Easing, p: f64) -> f64 {
    let Easing::CubicBezier { x1, y1, x2, y2 } = easing else {
        unreachable!()
    };
    let coordinate = |a: f64, b: f64, t: f64| {
        3.0 * a * (1.0 - t).powi(2) * t + 3.0 * b * (1.0 - t) * t * t + t.powi(3)
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..200 {
        let middle = (low + high) / 2.0;
        if coordinate(x1, x2, middle) < p {
            low = middle;
        } else {
            high = middle;
        }
    }
    coordinate(y1, y2, (low + high) / 2.0)
}

#[test]
fn a_bezier_along_the_diagonal_is_linear() {
    let diagonal = cubic_bezier(0.0, 0.0, 1.0, 1.0);
    for ms in 0..=1000 {
        let p = f64::from(ms) / 1000.0;
        let error = (output(diagonal, p) - output(Easing::Linear, p)).abs();
        assert!(error <= 1e-7, "p {p}: {error}");
    }
}

#[test]
fn css_ease_at_half_way() {
    let value = output(EASE, 0.5);
    assert!((value - 0.802_403_387_7).abs() <= 1e-6, "{value}");
}

/// Béziers of every shape, steep, flat and leaving the range, match a plain bisection to 1e-7.
#[test]
fn beziers_match_a_plain_bisection() {
    let shapes = [
        EASE,
        cubic_bezier(0.0, 0.0, 0.58, 1.0),
        cubic_bezier(0.42, 0.0, 1.0, 1.0),
        cubic_bezier(0.0, 1.0, 1.0, 0.0),
        cubic_bezier(0.68, -0.6, 0.32, 1.6),
        cubic_bezier(0.0, 3.0, 0.0, -2.0),
        cubic_bezier(1.0, 1.0, 1.0, 1.0),
    ];
    for easing in shapes {
        for ms in 0..1000 {
            let p = f64::from(ms) / 1000.0;
            let error = (output(easing, p) - reference_bezier(easing, p)).abs();
            assert!(error <= 1e-7, "{easing:?} at {p}: {error}");
        }
    }
}

/// A Bézier whose x is flat in its middle (x1 = 1, x2 = 0), where x = 0.5 + 4(t − 0.5)³, matches
/// its exact inverse, flat point included. A plain bisection would not: at the flat point it
/// cannot place t closer than f64's rounding of x allows, about 5e-6.
#[test]
fn a_bezier_flat_in_its_middle_matches_its_exact_inverse() {
    let (y1, y2) = (0.0, 1.0);
    let flat = cubic_bezier(1.0, y1, 0.0, y2);
    for ms in 0..=1000 {
        let p = f64::from(ms) / 1000.0;
        let t = 0.5 + ((p - 0.5) / 4.0).cbrt();
        let expected = 3.0 * y1 * (1.0 - t).powi(2) * t + 3.0 * y2 * (1.0 - t) * t * t + t.powi(3);
        let error = (output(flat, p) - expected).abs();
        assert!(error <= 1e-7, "at {p}: {error}");
    }
}

/// The named easings are their functions of progress.
#[test]
fn named_easings_are_their_functions() {
    for p in [0.1, 0.25, 0.5, 0.9_f64] {
        let cases = [
            (Easing::Linear, p),
            (Easing::EaseOutQuad, 1.0 - (1.0 - p) * (1.0 - p)),
            (Easing::EaseOutCubic, 1.0 - (1.0 - p).powi(3)),
            (Easing::EaseOutExpo, 1.0 - 2.0_f64.powf(-10.0 * p)),
        ];
        for (easing, expected) in cases {
            assert!(
                (output(easing, p) - expected).abs() <= 1e-12,
                "{easing:?} at {p}"
            );
        }
    }
}

/// Every easing is exactly at `from` up to the start and exactly at `to` from the end, even
/// where its own function stops short (the exponential).
#[test]
fn every_easing_starts_at_from_and_ends_exactly_at_to() {
    let easings = [
        Easing::Linear,
        Easing::EaseOutQuad,
        Easing::EaseOutCubic,
        Easing::EaseOutExpo,
        EASE,
    ];
    for easing in easings {
        let params = CurveParams::new(easing, Duration::from_millis(150)).unwrap();
        let curve = Curve::new(params, 0.1, 0.7, START);
        assert_eq!(curve.value(Duration::ZERO), 0.1);
        assert_eq!(curve.value(START), 0.1);
        assert_eq!(curve.end(), START + Duration::from_millis(150));
        assert_eq!(curve.value(curve.end()), 0.7);
        assert_eq!(curve.value(Duration::MAX), 0.7);
        assert_eq!(curve.target(), 0.7);
        assert!(!curve.is_done(curve.end() - Duration::from_nanos(1)));
        assert!(curve.is_done(curve.end()));
    }
}

#[test]
fn a_linear_curve_reversed_half_way_takes_half_its_duration() {
    let params = CurveParams::new(Easing::Linear, SECOND).unwrap();
    let curve = Curve::new(params, 0.0, 100.0, START);
    let half_way = START + SECOND / 2;
    let back = curve.retarget(half_way, 0.0);
    assert_eq!(back.value(half_way), 50.0);
    assert_eq!(back.end(), half_way + SECOND / 2);
    assert_eq!(back.value(back.end()), 0.0);
    assert!((back.value(half_way + SECOND / 4) - 25.0).abs() <= 1e-9);
}

/// A fade toggled again and again keeps the pace its parameters set: each retarget is measured
/// against the first curve's distance and the full duration, not against the shortened curve it
/// interrupts, which would make every toggle faster than the last.
#[test]
fn retargets_again_and_again_keep_the_pace() {
    let params = CurveParams::new(Easing::Linear, SECOND).unwrap();
    let curve = Curve::new(params, 0.0, 100.0, START);
    let half_way = START + SECOND / 2;
    let back = curve.retarget(half_way, 0.0);
    assert_eq!(back.end(), half_way + SECOND / 2);
    // A quarter of a second back, at 25: 75 units left, three quarters of the full duration.
    let quarter_later = half_way + SECOND / 4;
    assert_eq!(back.value(quarter_later), 25.0);
    let forth = back.retarget(quarter_later, 100.0);
    assert_eq!(forth.end(), quarter_later + SECOND.mul_f64(0.75));
    assert!((forth.value(forth.end() - SECOND / 4) - 75.0).abs() <= 1e-9);
    // Toggled at a tenth, then back: 0.91 of the way takes 0.91 of the full duration.
    let fade = Curve::new(params, 0.0, 1.0, START);
    let out = fade.retarget(START + SECOND / 10, 0.0);
    let at = START + SECOND / 10 + SECOND / 100;
    let again = out.retarget(at, 1.0);
    let left = 1.0 - out.value(at);
    assert!((left - 0.91).abs() <= 1e-12, "{left}");
    assert_eq!(again.end(), at + SECOND.mul_f64(left));
}

/// A curve that does not move has nothing to draw: it ends at once, at its target.
#[test]
fn a_curve_that_does_not_move_ends_at_once() {
    for easing in [Easing::Linear, EASE] {
        let still = Curve::new(CurveParams::new(easing, SECOND).unwrap(), 40.0, 40.0, START);
        assert_eq!(still.end(), START);
        assert!(still.is_done(START));
        assert_eq!(still.value(START), 40.0);
    }
}

/// A curve that does not move has no pace: its first retarget takes the full duration, and the
/// distance that one covers sets the pace of the retargets after it.
#[test]
fn a_still_curve_takes_its_pace_from_its_first_move() {
    let params = CurveParams::new(Easing::Linear, SECOND).unwrap();
    let still = Curve::new(params, 40.0, 40.0, START);
    let away = still.retarget(START, 0.0);
    assert_eq!(away.end(), START + SECOND);
    let half_way = START + SECOND / 2;
    assert_eq!(away.value(half_way), 20.0);
    let back = away.retarget(half_way, 40.0);
    assert_eq!(back.end(), half_way + SECOND / 2);
}

/// A retarget takes at most the whole duration, and the whole duration after a curve that does
/// not move; it never jumps.
#[test]
fn a_retarget_takes_at_most_the_whole_duration() {
    let params = CurveParams::new(EASE, SECOND).unwrap();
    let curve = Curve::new(params, 0.0, 100.0, START);
    let at = START + SECOND / 5;
    let farther = curve.retarget(at, 1000.0);
    assert_eq!(farther.end(), at + SECOND);
    assert_eq!(farther.value(at), curve.value(at));
    let still = Curve::new(params, 40.0, 40.0, START);
    assert_eq!(still.retarget(at, 0.0).end(), at + SECOND);
    // Onto its own value: no distance left, done at once.
    let onto = curve.retarget(at, curve.value(at));
    assert_eq!(onto.end(), at);
    assert_eq!(onto.value(at), curve.value(at));
    // Before the start: from the start.
    assert_eq!(
        curve.retarget(Duration::ZERO, 50.0),
        curve.retarget(START, 50.0)
    );
    // After the end: from the target.
    let after = curve.retarget(curve.end() + SECOND, 50.0);
    assert_eq!(after.value(curve.end() + SECOND), 100.0);
    assert_eq!(after.end(), curve.end() + SECOND + SECOND / 2);
}

/// A Bézier that leaves 0..=1 takes the value past both ends: nothing clamps it.
#[test]
fn a_bezier_leaving_the_range_is_not_clamped() {
    let back_and_forth = cubic_bezier(0.68, -0.6, 0.32, 1.6);
    let values: Vec<f64> = (0..=1000)
        .map(|ms| output(back_and_forth, f64::from(ms) / 1000.0))
        .collect();
    assert!(values.iter().any(|&value| value < -0.05));
    assert!(values.iter().any(|&value| value > 1.05));
}

#[test]
fn curve_params_are_refused_out_of_their_range() {
    for duration in [Duration::from_millis(1), Duration::from_secs(10)] {
        assert!(CurveParams::new(Easing::Linear, duration).is_ok());
    }
    for duration in [
        Duration::ZERO,
        Duration::from_micros(999),
        Duration::from_secs(10) + Duration::from_nanos(1),
    ] {
        assert!(CurveParams::new(Easing::Linear, duration).is_err());
    }
    let bezier = |x1, y1, x2, y2| CurveParams::new(cubic_bezier(x1, y1, x2, y2), SECOND);
    assert!(bezier(0.0, -5.0, 1.0, 5.0).is_ok());
    for (x1, y1, x2, y2) in [
        (-0.01, 0.0, 0.5, 1.0),
        (0.5, 0.0, 1.01, 1.0),
        (f64::NAN, 0.0, 0.5, 1.0),
        (0.5, f64::NAN, 0.5, 1.0),
        (0.5, 0.0, 0.5, f64::INFINITY),
    ] {
        assert!(bezier(x1, y1, x2, y2).is_err(), "{x1} {y1} {x2} {y2}");
    }
}

/// A start or target that is not finite gives a curve at its target at once.
#[test]
fn bad_inputs_give_a_curve_at_its_target_at_once() {
    let params = CurveParams::new(EASE, SECOND).unwrap();
    for (from, to, expected) in [
        (f64::NAN, 1.0, 1.0),
        (0.0, f64::INFINITY, 0.0),
        (f64::NAN, f64::NAN, 0.0),
        (-f64::MAX, f64::MAX, f64::MAX),
    ] {
        let curve = Curve::new(params, from, to, START);
        assert_eq!(curve.end(), START);
        assert_eq!(curve.target(), expected);
        for at in [Duration::ZERO, START, START + SECOND] {
            assert_eq!(curve.value(at), expected);
        }
    }
    let curve = Curve::new(params, 0.0, 1.0, START);
    let retargeted = curve.retarget(START + SECOND / 2, f64::NAN);
    assert_eq!(retargeted.end(), START + SECOND / 2);
    assert_eq!(retargeted.target(), curve.value(START + SECOND / 2));
}

/// A control point too far to draw is refused; the farthest accepted, with a huge distance,
/// saturates rather than overflows.
#[test]
fn extreme_curves_stay_finite() {
    for (y1, y2) in [
        (1e308, -1e308),
        (f64::MAX, 0.0),
        (1e6 + 1.0, 0.0),
        (0.0, f64::NAN),
    ] {
        assert!(
            CurveParams::new(cubic_bezier(0.5, y1, 0.5, y2), SECOND).is_err(),
            "{y1} {y2}"
        );
    }
    let wild = cubic_bezier(0.5, 1e6, 0.5, -1e6);
    let params = CurveParams::new(wild, SECOND).unwrap();
    let curve = Curve::new(params, -1e300, 1e300, START);
    for ms in 0..=1000 {
        assert!(curve.value(START + Duration::from_millis(ms)).is_finite());
    }
    let late = Curve::new(params, 0.0, 1.0, Duration::MAX - SECOND / 2);
    assert_eq!(late.end(), Duration::MAX);
}
