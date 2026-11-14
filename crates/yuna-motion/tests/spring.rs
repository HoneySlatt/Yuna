//! Springs checked against a numerical integration of their equation: an RK4 integration of
//! `x'' + 2ζω·x' + ω²·x = 0` at 0.1 ms steps, which samples the spring at every step.
//!
//! The stiffest overdamped springs have a decay rate too fast for RK4 at 0.1 ms to judge them:
//! at ζ = 6 and k = 1,200 (a fast rate of 413/s), from 0.001 unit at −5,000 units/s, RK4's own
//! error in the velocity is 4.6e-5, beyond the 3.5e-5 allowed, while RK4 at 1 µs agrees with the
//! closed form to 4e-12. Each 0.1 ms step is therefore cut into as many RK4 steps as keep the
//! fastest rate times the step under 0.01: one for every spring of the grid but those at ζ = 6
//! and k ≥ 800.
//!
//! Over a grid of damping ratios, stiffnesses, distances and velocities, and through retargets:
//!
//! - the value and the velocity match the integration over the spring's whole life;
//! - from its end on, the value is exactly the target, and the integrated motion stays within
//!   the tolerance for 5 s more;
//! - the end is not much later than the last time the integrated motion strayed farther than
//!   the tolerance: at most 1.5 times it, plus 20 ms.
//!
//! Each test prints its worst figures; `cargo test -p yuna-motion -- --nocapture` shows them.

use std::time::Duration;

use yuna_motion::{Spring, SpringParams};

const RATIOS: [f64; 7] = [0.1, 0.5, 0.999999, 1.0, 1.000001, 1.5, 6.0];
const STIFFNESSES: [f64; 3] = [50.0, 800.0, 1200.0];
const DISTANCES: [f64; 4] = [1e-3, 1.0, 1280.0, 2e5];
const VELOCITIES: [f64; 3] = [-5000.0, 0.0, 3000.0];

/// The sampling step, and the integration's, which [`substeps`] may cut further.
const STEP: Duration = Duration::from_micros(100);
/// Half a physical pixel at scale 1, in logical units.
const TOLERANCE: f64 = 0.5;
/// How long after its end a spring is watched.
const WATCHED_AFTER_END: Duration = Duration::from_secs(5);
/// A start away from 0, so that nothing depends on the clock's origin; a multiple of [`STEP`].
const START: Duration = Duration::from_millis(7_250);
/// A target away from 0, so that offsets and values differ.
const TARGET: f64 = 300.0;

/// The value's allowed error against the integration, for a motion over `distance`.
fn value_allowance(distance: f64) -> f64 {
    1e-6 * distance.abs().max(1.0)
}

/// What following a spring beside the integration found.
#[derive(Debug, Default)]
struct Figures {
    /// The worst |value − integration|, over its allowance: passes at 1 or under.
    value_error: f64,
    /// The worst |velocity − integration|, over the value's allowance times ω.
    velocity_error: f64,
    /// The last spring's life, from its start to its end, in seconds.
    life: f64,
    /// The last time sampled, from the last spring's start, when the integration was farther
    /// than the tolerance from the target; `None` if it never was.
    last_exceedance: Option<f64>,
    /// The integration's farthest distance from the target after the end, over the tolerance:
    /// passes at 1 or under.
    after_end: f64,
}

/// How many RK4 steps each [`STEP`] is cut into: enough that the spring's fastest rate, ω for a
/// spring that bounces and the fast decay rate ω·(ζ + √(ζ² − 1)) for one that does not, times the
/// step stays under 0.01.
fn substeps(params: SpringParams) -> u32 {
    let ratio = params.damping_ratio();
    let fastest_rate =
        params.stiffness().sqrt() * (ratio + (ratio * ratio - 1.0).max(0.0).sqrt()).max(1.0);
    (fastest_rate * STEP.as_secs_f64() / 0.01).ceil().max(1.0) as u32
}

/// RK4 steps of `y'' = −2ζω·y' − ω²·(y − target)` over one [`STEP`], cut into [`substeps`].
fn integrate_one_step(state: (f64, f64), target: f64, params: SpringParams) -> (f64, f64) {
    let substeps = substeps(params);
    let h = STEP.as_secs_f64() / f64::from(substeps);
    (0..substeps).fold(state, |state, _| rk4_step(state, target, params, h))
}

/// One RK4 step of `y'' = −2ζω·y' − ω²·(y − target)`, of `h` seconds.
fn rk4_step((y, v): (f64, f64), target: f64, params: SpringParams, h: f64) -> (f64, f64) {
    let stiffness = params.stiffness();
    let damping = 2.0 * params.damping_ratio() * stiffness.sqrt();
    let acceleration = |y: f64, v: f64| -damping * v - stiffness * (y - target);
    let (k1y, k1v) = (v, acceleration(y, v));
    let (k2y, k2v) = (
        v + h / 2.0 * k1v,
        acceleration(y + h / 2.0 * k1y, v + h / 2.0 * k1v),
    );
    let (k3y, k3v) = (
        v + h / 2.0 * k2v,
        acceleration(y + h / 2.0 * k2y, v + h / 2.0 * k2v),
    );
    let (k4y, k4v) = (v + h * k3v, acceleration(y + h * k3y, v + h * k3v));
    (
        y + h / 6.0 * (k1y + 2.0 * k2y + 2.0 * k3y + k4y),
        v + h / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v),
    )
}

/// Follows a spring from `from` toward `to` with `velocity`, started at [`START`], and its
/// retargets, each `(time, target)` with times multiples of [`STEP`], beside an RK4 integration
/// whose target switches at the same times; every [`STEP`], until [`WATCHED_AFTER_END`] after the
/// last spring's end.
///
/// Asserts on the way what holds at every sample: the value and the velocity are finite, exactly
/// the target and 0 from the end on (at the end itself too, for every spring of the chain), and
/// a retarget changes neither.
fn follow(
    params: SpringParams,
    from: f64,
    to: f64,
    velocity: f64,
    retargets: &[(Duration, f64)],
) -> Figures {
    let omega = params.stiffness().sqrt();
    let mut spring = Spring::new(params, from, to, velocity, START, TOLERANCE);
    let mut spring_start = START;
    let mut pending = retargets.iter().peekable();
    let mut distance = (from - to).abs();
    let mut state = (from, velocity);
    let mut figures = Figures::default();
    for n in 0u32.. {
        let at = START + STEP * n;
        if let Some(&&(when, target)) = pending.peek()
            && when == at
        {
            pending.next();
            assert_ends_exactly_at_its_target(&spring);
            let next = spring.retarget(at, target);
            let (value, speed) = (spring.value(at), spring.velocity(at));
            assert!((next.value(at) - value).abs() <= 1e-9 * value.abs().max(1.0));
            assert!((next.velocity(at) - speed).abs() <= 1e-9 * speed.abs().max(1.0));
            distance = distance.max((value - target).abs());
            spring = next;
            spring_start = at;
            figures.last_exceedance = None;
        }
        let (value, speed) = (spring.value(at), spring.velocity(at));
        assert!(value.is_finite() && speed.is_finite(), "at {at:?}");
        let off_target = (state.0 - spring.target()).abs();
        if off_target > TOLERANCE {
            figures.last_exceedance = Some((at - spring_start).as_secs_f64());
        }
        if at < spring.end() {
            let allowance = value_allowance(distance);
            figures.value_error = figures.value_error.max((value - state.0).abs() / allowance);
            figures.velocity_error = figures
                .velocity_error
                .max((speed - state.1).abs() / (allowance * omega));
        } else {
            assert_eq!(value, spring.target(), "at {at:?}, end {:?}", spring.end());
            assert_eq!(speed, 0.0);
            figures.after_end = figures.after_end.max(off_target / TOLERANCE);
            if pending.peek().is_none() && at >= spring.end() + WATCHED_AFTER_END {
                break;
            }
        }
        state = integrate_one_step(state, spring.target(), params);
    }
    assert_ends_exactly_at_its_target(&spring);
    figures.life = (spring.end() - spring_start).as_secs_f64();
    figures
}

/// At its end, a spring is exactly at its target, at rest.
fn assert_ends_exactly_at_its_target(spring: &Spring) {
    assert_eq!(spring.value(spring.end()), spring.target(), "{spring:?}");
    assert_eq!(spring.velocity(spring.end()), 0.0, "{spring:?}");
    assert!(spring.is_done(spring.end()));
}

/// Checks every case of the grid at one damping ratio, and prints the worst figures.
fn check_ratio(ratio: f64) {
    let mut worst = Figures::default();
    let mut worst_slack = (0.0, String::new());
    let mut worst_margin = (f64::NEG_INFINITY, String::new());
    let mut longest: f64 = 0.0;
    for stiffness in STIFFNESSES {
        let params = SpringParams::new(ratio, stiffness).unwrap();
        for distance in DISTANCES {
            for velocity in VELOCITIES {
                let case = format!("ζ {ratio}, k {stiffness}, distance {distance}, v {velocity}");
                let figures = follow(params, TARGET + distance, TARGET, velocity, &[]);
                assert!(figures.value_error <= 1.0, "{case}: {figures:?}");
                assert!(figures.velocity_error <= 1.0, "{case}: {figures:?}");
                assert!(figures.after_end <= 1.0, "{case}: {figures:?}");
                let last = figures.last_exceedance.unwrap_or(0.0);
                let margin = figures.life - (1.5 * last + 0.020);
                assert!(margin <= 0.0, "{case}: ends too late: {figures:?}");
                worst.value_error = worst.value_error.max(figures.value_error);
                worst.velocity_error = worst.velocity_error.max(figures.velocity_error);
                worst.after_end = worst.after_end.max(figures.after_end);
                if last > 0.0 && figures.life / last > worst_slack.0 {
                    worst_slack = (figures.life / last, case.clone());
                }
                if last > 0.0 && margin > worst_margin.0 {
                    worst_margin = (margin, case);
                }
                longest = longest.max(figures.life);
            }
        }
    }
    println!(
        "ζ {ratio}: worst value error {:.3e} of its allowance, velocity {:.3e}; after the end, \
         at most {:.6} of the tolerance; longest life {longest:.3} s; end over last exceedance \
         at worst {:.4} ({}); closest to 1.5 × it + 20 ms by {:.1} ms ({})",
        worst.value_error,
        worst.velocity_error,
        worst.after_end,
        worst_slack.0,
        worst_slack.1,
        -worst_margin.0 * 1000.0,
        worst_margin.1,
    );
}

#[test]
fn a_spring_of_ratio_0_1_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[0]);
}

#[test]
fn a_spring_of_ratio_0_5_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[1]);
}

#[test]
fn a_spring_just_under_critical_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[2]);
}

#[test]
fn a_critical_spring_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[3]);
}

#[test]
fn a_spring_just_over_critical_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[4]);
}

#[test]
fn a_spring_of_ratio_1_5_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[5]);
}

#[test]
fn a_spring_of_ratio_6_matches_the_integration_and_ends_on_time() {
    check_ratio(RATIOS[6]);
}

/// A retarget keeps the value and the velocity, and the springs it makes follow the equation
/// with its target switched: reversing, going farther, and again, at every ratio, from rest and
/// launched.
#[test]
fn retargets_follow_the_equation_with_the_target_switched() {
    let mut worst = Figures::default();
    for ratio in RATIOS {
        for stiffness in [50.0, 800.0] {
            let params = SpringParams::new(ratio, stiffness).unwrap();
            for velocity in [0.0, 3000.0] {
                let retargets = [
                    (START + Duration::from_millis(40), TARGET + 640.0),
                    (START + Duration::from_millis(120), TARGET - 2560.0),
                    (START + Duration::from_millis(121), TARGET - 2561.0),
                    (START + Duration::from_millis(300), TARGET + 1e4),
                ];
                let case = format!("ζ {ratio}, k {stiffness}, v {velocity}");
                let figures = follow(params, TARGET + 1280.0, TARGET, velocity, &retargets);
                assert!(figures.value_error <= 1.0, "{case}: {figures:?}");
                assert!(figures.velocity_error <= 1.0, "{case}: {figures:?}");
                assert!(figures.after_end <= 1.0, "{case}: {figures:?}");
                worst.value_error = worst.value_error.max(figures.value_error);
                worst.velocity_error = worst.velocity_error.max(figures.velocity_error);
                worst.after_end = worst.after_end.max(figures.after_end);
            }
        }
    }
    println!(
        "retargets: worst value error {:.3e} of its allowance, velocity {:.3e}; after the end, \
         at most {:.6} of the tolerance",
        worst.value_error, worst.velocity_error, worst.after_end,
    );
}

/// A change of mind passes the last target: right, right, left, on a critical camera, as
/// ADR-0053's consequences say; and the integration agrees.
#[test]
fn a_quick_change_of_mind_passes_the_last_target() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let column = 1280.0;
    let retargets = [
        (START + Duration::from_millis(29), 2.0 * column),
        (START + Duration::from_millis(75), column),
    ];
    let figures = follow(params, 0.0, column, 0.0, &retargets);
    assert!(figures.value_error <= 1.0 && figures.velocity_error <= 1.0);
    let spring = Spring::new(params, 0.0, column, 0.0, START, TOLERANCE)
        .retarget(retargets[0].0, retargets[0].1)
        .retarget(retargets[1].0, retargets[1].1);
    let farthest = (0..1000)
        .map(|ms| spring.value(retargets[1].0 + Duration::from_millis(ms)))
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(farthest > column + TOLERANCE, "{farthest}");
    println!(
        "right, right, left at 0, 29 and 75 ms: passes the last target by {:.1} units, {:.1} % \
         of a {column}-unit column",
        farthest - column,
        (farthest - column) / column * 100.0,
    );
}

/// From rest, a spring without bounce never passes its target, at any distance or stiffness.
#[test]
fn no_overshoot_from_rest_when_the_ratio_is_at_least_1() {
    for ratio in [1.0, 1.000001, 1.5, 6.0, 20.0] {
        for stiffness in STIFFNESSES {
            let params = SpringParams::new(ratio, stiffness).unwrap();
            for distance in DISTANCES.iter().flat_map(|&d| [d, -d]) {
                let spring = Spring::new(params, TARGET + distance, TARGET, 0.0, START, TOLERANCE);
                let mut at = START;
                while at <= spring.end() {
                    let offset = spring.value(at) - TARGET;
                    assert!(
                        offset * distance.signum() >= 0.0,
                        "ζ {ratio}, k {stiffness}, distance {distance}: {offset} at {at:?}"
                    );
                    at += STEP;
                }
            }
        }
    }
}

/// niri's own test case, where cosh overflows after 3.47 s while the spring needs about 3 s:
/// written as two decaying exponentials, the end is finite and nothing is NaN.
#[test]
fn a_heavily_damped_long_move_stays_finite() {
    let params = SpringParams::new(6.0, 1200.0).unwrap();
    for tolerance in [0.5, 0.25] {
        let spring = Spring::new(params, 3840.0, 0.0, 0.0, START, tolerance);
        assert!(spring.end() > START + Duration::from_secs(2));
        assert!(spring.end() < START + Duration::from_secs(10));
        let mut at = START;
        while at <= spring.end() + Duration::from_secs(10) {
            assert!(spring.value(at).is_finite() && spring.velocity(at).is_finite());
            at += Duration::from_millis(1);
        }
        // Far beyond its end too, where the exponentials underflow.
        assert_eq!(spring.value(Duration::MAX), 0.0);
    }
    let figures = follow(params, TARGET + 3840.0, TARGET, 0.0, &[]);
    assert!(figures.value_error <= 1.0 && figures.velocity_error <= 1.0);
    assert!(figures.after_end <= 1.0);
}

/// A spring that starts at its target with a velocity still moves, ends later, and follows the
/// equation; without a velocity, it ends at once.
#[test]
fn a_spring_at_its_target_moves_by_its_velocity_alone() {
    for ratio in RATIOS {
        let params = SpringParams::new(ratio, 800.0).unwrap();
        let still = Spring::new(params, TARGET, TARGET, 0.0, START, TOLERANCE);
        assert_eq!(still.end(), START);
        assert_eq!(still.value(START), TARGET);
        let launched = Spring::new(params, TARGET, TARGET, 3000.0, START, TOLERANCE);
        assert!(launched.end() > START + Duration::from_millis(10));
        assert!(launched.value(START + Duration::from_millis(10)) > TARGET + 1.0);
        let figures = follow(params, TARGET, TARGET, 3000.0, &[]);
        assert!(figures.value_error <= 1.0 && figures.velocity_error <= 1.0);
        assert!(figures.after_end <= 1.0);
    }
}

/// A velocity that cannot take the value a tolerance away is not seen: the spring ends at once.
#[test]
fn a_velocity_too_small_to_be_seen_ends_at_once() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    // At most 1/(ω·e) ≈ 0.013 units away.
    let spring = Spring::new(params, TARGET, TARGET, 1.0, START, TOLERANCE);
    assert_eq!(spring.end(), START);
}

/// The time to rest of a critical spring from rest is its true settle time: the time from which
/// `D·(1 + ωt)·e^(−ωt)` stays under half a pixel, computed apart by bisection.
#[test]
fn time_to_rest_is_the_true_settle_time_of_a_critical_spring() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let expected_ms = [
        (10.0, 167.720_938_499),
        (100.0, 262.694_747_737),
        (1920.0, 378.797_580_200),
        (3840.0, 405.514_230_188),
    ];
    for (distance, expected) in expected_ms {
        let time = params.time_to_rest(distance, 0.5);
        let error = (time.as_secs_f64() * 1000.0 - expected).abs();
        assert!(error < 1e-3, "{distance}: {time:?}");
        assert_eq!(time, params.time_to_rest(-distance, 0.5));
        let spring = Spring::new(params, distance, 0.0, 0.0, START, 0.5);
        assert_eq!(spring.end() - START, time);
    }
    // ADR-0053's check: niri's default crosses a 4K output's diagonal well within 1.5 s.
    let diagonal = 3840.0_f64.hypot(2160.0);
    assert!(params.time_to_rest(diagonal, 0.5) < Duration::from_millis(420));
}

/// Out of range, not finite: refused. The limits themselves: accepted.
#[test]
fn spring_params_are_refused_out_of_their_range() {
    for (ratio, stiffness) in [(0.05, 1.0), (20.0, 100_000.0), (1.0, 800.0)] {
        let params = SpringParams::new(ratio, stiffness).unwrap();
        assert_eq!(
            (params.damping_ratio(), params.stiffness()),
            (ratio, stiffness)
        );
    }
    for (ratio, stiffness) in [
        (0.049, 800.0),
        (20.01, 800.0),
        (1.0, 0.99),
        (1.0, 100_001.0),
        (f64::NAN, 800.0),
        (1.0, f64::INFINITY),
        (-1.0, 800.0),
    ] {
        assert!(SpringParams::new(ratio, stiffness).is_err());
    }
}

/// Inputs that are not finite, and tolerances that are not positive, give a spring at its
/// target at once; a target that is not finite is replaced.
#[test]
fn bad_inputs_give_a_spring_at_its_target_at_once() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let at_once = |from: f64, to: f64, velocity: f64, tolerance: f64, expected: f64| {
        let spring = Spring::new(params, from, to, velocity, START, tolerance);
        assert_eq!(spring.end(), START, "{from} {to} {velocity} {tolerance}");
        assert_eq!(spring.target(), expected);
        for at in [Duration::ZERO, START, START + Duration::from_secs(1)] {
            assert_eq!(spring.value(at), expected);
            assert_eq!(spring.velocity(at), 0.0);
        }
        assert!(spring.is_done(START));
    };
    at_once(f64::NAN, 10.0, 0.0, 0.5, 10.0);
    at_once(f64::INFINITY, 10.0, 0.0, 0.5, 10.0);
    at_once(5.0, 10.0, f64::NAN, 0.5, 10.0);
    at_once(5.0, 10.0, f64::NEG_INFINITY, 0.5, 10.0);
    at_once(5.0, f64::NAN, 0.0, 0.5, 5.0);
    at_once(f64::NAN, f64::INFINITY, 0.0, 0.5, 0.0);
    for tolerance in [0.0, -0.5, f64::NAN, f64::INFINITY] {
        at_once(5.0, 10.0, 100.0, tolerance, 10.0);
    }
    // A distance that overflows.
    at_once(f64::MAX, -f64::MAX, 0.0, 0.5, -f64::MAX);
    assert_eq!(params.time_to_rest(f64::NAN, 0.5), Duration::ZERO);
    assert_eq!(params.time_to_rest(100.0, 0.0), Duration::ZERO);
}

/// Inputs near the limits of `f64` stay finite, and an end near the end of time stops there.
#[test]
fn extreme_inputs_stay_finite() {
    for ratio in [0.05, 0.999_999_9, 1.0, 1.000_000_1, 20.0] {
        for stiffness in [1.0, 100_000.0] {
            let params = SpringParams::new(ratio, stiffness).unwrap();
            for (from, to, velocity, tolerance) in [
                (1e300, -1e300, 1e300, 0.5),
                (-1e307, 1e307, -1e307, 1e-300),
                (0.0, 0.0, f64::MAX, 0.5),
                (1.0, 0.0, 0.0, f64::MIN_POSITIVE),
            ] {
                let spring = Spring::new(params, from, to, velocity, START, tolerance);
                assert!(spring.end() >= START);
                let life = spring.end() - START;
                for share in [0.0, 1e-9, 1e-3, 0.1, 0.5, 0.99, 1.0] {
                    let at = START + life.mul_f64(share);
                    assert!(spring.value(at).is_finite(), "{ratio} {stiffness} {from}");
                    assert!(
                        spring.velocity(at).is_finite(),
                        "{ratio} {stiffness} {from}"
                    );
                }
                assert_eq!(spring.value(spring.end()), spring.target());
            }
        }
    }
    let params = SpringParams::new(0.05, 1.0).unwrap();
    let late = Duration::MAX - Duration::from_secs(1);
    let spring = Spring::new(params, 1e5, 0.0, 0.0, late, 0.5);
    assert_eq!(spring.end(), Duration::MAX);
    assert!(spring.value(late + Duration::from_millis(500)).is_finite());
}

/// A retarget before the start acts at the start; after the end, from rest at the target.
#[test]
fn a_retarget_outside_the_life_starts_from_its_edge() {
    let params = SpringParams::new(0.7, 800.0).unwrap();
    let spring = Spring::new(params, 100.0, 0.0, 250.0, START, TOLERANCE);
    let early = spring.retarget(Duration::ZERO, 50.0);
    assert_eq!(
        early,
        Spring::new(params, 100.0, 50.0, 250.0, START, TOLERANCE)
    );
    let late = spring.retarget(spring.end() + Duration::from_secs(1), 50.0);
    let at = spring.end() + Duration::from_secs(1);
    assert_eq!(late, Spring::new(params, 0.0, 50.0, 0.0, at, TOLERANCE));
    assert!(!spring.is_done(spring.end() - Duration::from_nanos(1)));
    assert!(spring.is_done(spring.end()));
}

/// A moving spring reads its start value up to its start, whatever time is asked before it.
#[test]
fn a_moving_spring_reads_its_start_before_it_starts() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let spring = Spring::new(params, 10.0, 1290.0, 500.0, START, 0.5);
    for at in [
        Duration::ZERO,
        START - Duration::from_millis(1),
        START - Duration::from_nanos(1),
        START,
    ] {
        assert_eq!(spring.value(at), 10.0, "{at:?}");
    }
    assert!(spring.value(START + Duration::from_millis(5)) > 10.0);
}

/// A shifted spring is the same motion moved: its value at every time, its target and its start
/// moved by the shift, its velocity and its end unchanged; retargeted, it goes on as the
/// original retargeted to the shifted place.
#[test]
fn a_shifted_spring_is_the_same_motion_moved() {
    let params = SpringParams::new(0.7, 600.0).unwrap();
    let spring = Spring::new(params, 0.0, 1000.0, 250.0, START, TOLERANCE);
    let shifted = spring.shifted(-300.0);
    assert_eq!(shifted.target(), 700.0);
    assert_eq!(shifted.end(), spring.end());
    assert_eq!(shifted.value(START), -300.0);
    let mut at = START;
    while at <= spring.end() + STEP {
        assert!((shifted.value(at) - (spring.value(at) - 300.0)).abs() < 1e-9);
        assert_eq!(shifted.velocity(at), spring.velocity(at));
        at += STEP * 10;
    }
    let mid = START + Duration::from_millis(40);
    let moved_on = shifted.retarget(mid, 0.0);
    assert!((moved_on.value(mid) - (spring.value(mid) - 300.0)).abs() < 1e-9);
    assert_eq!(moved_on.velocity(mid), spring.velocity(mid));
    assert_eq!(spring.shifted(0.0), spring);
}

/// A shift that is not finite, or that overflows, gives a spring at rest at its own target.
#[test]
fn a_shift_out_of_range_gives_a_spring_at_rest() {
    let params = SpringParams::new(1.0, 800.0).unwrap();
    let spring = Spring::new(params, 0.0, 1000.0, 0.0, START, TOLERANCE);
    for by in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let shifted = spring.shifted(by);
        assert!(shifted.is_done(START), "{by}");
        assert_eq!(shifted.value(START), 1000.0, "{by}");
        assert_eq!(shifted.target(), 1000.0, "{by}");
    }
    let far = Spring::new(params, 1e308, 1.5e308, 0.0, START, TOLERANCE);
    let overflowed = far.shifted(1e308);
    assert!(overflowed.is_done(START));
    assert_eq!(overflowed.value(START), 1.5e308);
}
