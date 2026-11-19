//! What the motion crate says of parameters it refuses, and a spring moved by an amount that
//! takes one of its ends out of the finite numbers.

use std::time::Duration;

use yuna_motion::{CurveParams, Easing, InvalidParams, Spring, SpringParams};

#[test]
fn a_refusal_of_parameters_says_so() {
    let refused = SpringParams::new(0.0, 100.0).unwrap_err();
    assert_eq!(refused, InvalidParams);
    assert_eq!(refused.to_string(), "motion parameters out of their range");
    let refused = CurveParams::new(Easing::Linear, Duration::ZERO);
    assert_eq!(
        refused.unwrap_err().to_string(),
        "motion parameters out of their range"
    );
}

/// A spring shifted past the largest finite number at one of its ends does not take a mixture of
/// finite and infinite values: it rests where its target was.
#[test]
fn a_spring_shifted_out_of_the_finite_numbers_rests_where_it_was_headed() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let start = Duration::from_secs(1);
    let spring = Spring::new(params, 1e300, 0.0, 0.0, start, 0.5);
    let shifted = spring.shifted(f64::MAX);
    assert_eq!(shifted.target(), 0.0);
    assert!(shifted.is_done(start));
    assert_eq!(shifted.value(start + Duration::from_secs(1)), 0.0);
    // The same when its target is the one that leaves the finite numbers.
    let spring = Spring::new(params, 0.0, 1e300, 0.0, start, 0.5);
    let shifted = spring.shifted(f64::MAX);
    assert_eq!(shifted.target(), 1e300);
    assert!(shifted.is_done(start));
}

/// A finite shift moves both ends and keeps the spring's motion.
#[test]
fn a_spring_shifted_by_a_finite_amount_moves_with_its_ends() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let start = Duration::from_secs(1);
    let spring = Spring::new(params, 100.0, 200.0, 0.0, start, 0.5);
    let shifted = spring.shifted(50.0);
    assert_eq!(shifted.target(), 250.0);
    assert_eq!(shifted.value(start), 150.0);
    let at = start + Duration::from_millis(50);
    assert!((shifted.value(at) - (spring.value(at) + 50.0)).abs() < 1e-9);
    assert_eq!(shifted.end(), spring.end());
}
