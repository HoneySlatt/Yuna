//! Emulated VRR ([ADR-0045](../../../../docs/adr/0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)):
//! a client drawing at a steady rate on a VRR output of 48 to 240 Hz, the owner's monitor's
//! range, gets the presentation times amdgpu would give it, and Yuna wakes for nothing at rest.
//!
//! Each story's output is 1920 × 1080 at scale 1, on the manual clock: a drawing takes no time,
//! so a frame is ready when it is committed.

use std::time::Duration;

use yuna::{ClockMode, Compositor, Config, OutputMode};

use crate::client::{Client, Feedback};
use crate::{BLUE, RED, assert_rests, run_armed_deadlines};

/// 1/240 s, the range's shortest refresh, to the nanosecond below.
const SHORTEST: Duration = Duration::from_nanos(4_166_666);
/// The vertical blank, the least time between a frame ready and its showing: 269 of 2,429
/// lines of the shortest refresh, as the owner's monitor's mode (`vrr.rs`).
const BLANK: Duration = Duration::from_nanos(461_438);

fn compositor(vrr: bool) -> Compositor {
    let mut mode = OutputMode::new(1920, 1080, 240.0, 1.0).expect("a mode");
    if vrr {
        mode = mode.with_vrr(48.0).expect("a range");
    }
    Compositor::new(Config {
        outputs: vec![mode],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// Maps a window and lets everything settle; returns its index.
fn mapped(compositor: &mut Compositor, client: &mut Client) -> usize {
    let window = client.open_window(compositor);
    client.ack(window);
    client.attach(window, 400, 300, RED);
    client.commit(window);
    client.roundtrip(compositor);
    run_armed_deadlines(compositor);
    client.roundtrip(compositor);
    window
}

/// A client that commits a new frame every `interval`, `frames` times, from 1 s after its window
/// mapped, each with presentation feedback: what each frame was told, in order.
fn steady(compositor: &mut Compositor, interval: Duration, frames: u32) -> Vec<Feedback> {
    let mut client = Client::connect(compositor);
    let window = mapped(compositor, &mut client);
    let start = compositor.now() + Duration::from_secs(1);
    for k in 0..frames {
        let at = start + interval * k;
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
        client.attach(window, 400, 300, if k % 2 == 0 { BLUE } else { RED });
        client.request_feedback(window);
        client.commit(window);
        client.roundtrip(compositor);
    }
    run_armed_deadlines(compositor);
    client.roundtrip(compositor);
    let feedback = client.window(window).feedback.clone();
    client.close(window);
    client.roundtrip(compositor);
    run_armed_deadlines(compositor);
    feedback
}

/// The frames a steady client draws before its cadence is judged. Its first frame comes after
/// a pause of 1 s, from which amdgpu inserts some 80 refreshes of 12.5 ms: the next frames wait
/// for their ends until the driver has the time between two steady frames.
const WARM_UP: usize = 6;

/// The presentation times and refresh numbers of the frames presented.
fn presented(feedback: &[Feedback]) -> Vec<(Duration, u64)> {
    feedback
        .iter()
        .filter_map(|f| match f {
            Feedback::Presented { time, seq, .. } => Some((*time, *seq)),
            Feedback::Discarded => None,
        })
        .collect()
}

/// The time between each presented frame and the next.
fn intervals(presented: &[(Duration, u64)]) -> Vec<Duration> {
    presented.windows(2).map(|p| p[1].0 - p[0].0).collect()
}

/// Inside the range, each frame is shown when it is ready: 100 fps gives 10 ms between
/// presentations, each one refresh after the last, and Yuna rests after.
#[test]
fn a_frame_inside_the_range_is_shown_when_it_is_ready() {
    let mut compositor = compositor(true);
    let feedback = steady(&mut compositor, Duration::from_millis(10), 30);
    let shown = presented(&feedback);
    for (k, interval) in intervals(&shown).into_iter().enumerate().skip(WARM_UP) {
        assert_eq!(interval, Duration::from_millis(10), "frame {k}");
    }
    for pair in shown[WARM_UP..].windows(2) {
        assert_eq!(pair[1].1, pair[0].1 + 1);
    }
    // wp_presentation version 1, which the test client binds, takes 0 as a VRR output's refresh.
    assert!(feedback.iter().all(|f| match f {
        Feedback::Presented { refresh, .. } => refresh.is_zero(),
        Feedback::Discarded => true,
    }));
    assert_rests(&mut compositor);
}

/// Above the range, a frame waits for the shortest refresh: 300 fps is shown at 240 Hz at most,
/// and the frames that could not be shown are discarded.
#[test]
fn frames_above_the_range_wait_for_the_shortest_refresh() {
    let mut compositor = compositor(true);
    let feedback = steady(&mut compositor, Duration::from_nanos(3_333_333), 60);
    let shown = presented(&feedback);
    assert!(shown.len() < 60);
    assert!(feedback.iter().any(|f| matches!(f, Feedback::Discarded)));
    for interval in intervals(&shown) {
        assert!(interval >= SHORTEST, "{interval:?}");
    }
    assert_rests(&mut compositor);
}

/// Below the range, amdgpu shows each frame several times, and the film keeps its cadence: at
/// 24 fps, each frame three refreshes after the last, 41.667 ms apart within a microsecond.
#[test]
fn a_film_below_the_range_keeps_its_cadence_with_inserted_refreshes() {
    let mut compositor = compositor(true);
    let frame = Duration::from_nanos(41_666_667);
    let feedback = steady(&mut compositor, frame, 48);
    let shown = presented(&feedback);
    assert_eq!(shown.len(), 48);
    for (k, pair) in shown.windows(2).enumerate().skip(WARM_UP) {
        let interval = pair[1].0 - pair[0].0;
        assert!(
            interval.abs_diff(frame) < Duration::from_micros(1),
            "frame {k}: {interval:?}"
        );
        assert_eq!(pair[1].1 - pair[0].1, 3, "frame {k}");
    }
    assert_rests(&mut compositor);
}

/// What Aerith reads to tell VRR from a fixed rate (its ADR-0021): at 23.976 fps, a fixed 240 Hz
/// output presents every frame a whole number of periods after the last, a VRR one almost
/// never.
#[test]
fn a_client_tells_vrr_from_a_fixed_rate_by_its_presentation_times() {
    let frame = Duration::from_nanos(41_708_333);
    let whole = |interval: &Duration| {
        let periods = interval.as_nanos() as f64 / (1e9 / 240.0);
        (periods - periods.round()).abs() < 1e-3
    };
    let mut fixed = compositor(false);
    let at_fixed = intervals(&presented(&steady(&mut fixed, frame, 48)));
    assert!(at_fixed.iter().all(whole), "{at_fixed:?}");

    let mut vrr = compositor(true);
    let at_vrr = intervals(&presented(&steady(&mut vrr, frame, 48)));
    let at_vrr = &at_vrr[WARM_UP..];
    let whole_count = at_vrr.iter().filter(|i| whole(i)).count();
    assert!(
        whole_count <= 2,
        "{whole_count} of {}: {at_vrr:?}",
        at_vrr.len()
    );
}

/// After a pause, a frame is shown a vertical blank after it is ready, and its refresh number
/// counts the panel's own refreshes meanwhile, one every 1/48 s, though none woke Yuna.
#[test]
fn after_a_pause_a_frame_is_shown_a_blank_later_and_counts_the_panels_own_refreshes() {
    let mut compositor = compositor(true);
    let mut client = Client::connect(&mut compositor);
    let window = mapped(&mut compositor, &mut client);
    client.attach(window, 400, 300, BLUE);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let wakeups = compositor.counters().wakeups;

    // One second later, plus 10 ms, inside the 48th of the panel's own refreshes.
    let pause = Duration::from_millis(1_010);
    compositor.advance(pause).expect("the clock moves");
    assert_eq!(compositor.counters().wakeups, wakeups, "nothing woke Yuna");
    let ready = compositor.now();
    client.attach(window, 400, 300, RED);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let shown = presented(&client.window(window).feedback);
    let (before, after) = (shown[shown.len() - 2], shown[shown.len() - 1]);
    assert_eq!(after.0, ready + BLANK);
    assert_eq!(after.1 - before.1, 1 + 48);
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

#[test]
fn a_range_narrower_than_twice_its_least_rate_is_refused() {
    let mode = OutputMode::new(1920, 1080, 60.0, 1.0).expect("a mode");
    assert!(mode.with_vrr(48.0).is_err(), "48 to 60 Hz");
    assert!(mode.with_vrr(30.0).is_ok());
    assert!(mode.with_vrr(0.5).is_err(), "below 1 Hz");
    assert!(mode.with_vrr(f64::NAN).is_err());
}

/// Callbacks asked without damage are answered at most once a shortest refresh, as at a fixed
/// rate once a period (ADR-0031), never in a loop: a refresh that draws nothing still counts as
/// one, though the panel shows no new frame. Yuna then rests.
#[test]
fn callbacks_asked_without_damage_come_at_most_once_a_shortest_refresh() {
    let mut compositor = compositor(true);
    let mut client = Client::connect(&mut compositor);
    let window = mapped(&mut compositor, &mut client);
    compositor
        .advance(Duration::from_secs(1))
        .expect("the clock moves");
    let drawn = compositor.counters().frames_drawn;
    let mut answered_at = Vec::new();
    for _ in 0..20 {
        let answered = client.window(window).frames.len();
        client.request_frame(window);
        client.commit(window);
        client.roundtrip(&mut compositor);
        if client.window(window).frames.len() == answered {
            let due = compositor.next_deadline().expect("a refresh armed");
            compositor
                .advance(due - compositor.now())
                .expect("the clock moves");
            client.roundtrip(&mut compositor);
        }
        assert_eq!(client.window(window).frames.len(), answered + 1);
        answered_at.push(compositor.now());
    }
    for pair in answered_at.windows(2) {
        assert!(
            pair[1] - pair[0] >= SHORTEST - Duration::from_millis(3),
            "{answered_at:?}"
        );
    }
    assert!(
        answered_at[19] - answered_at[0] >= SHORTEST * 18,
        "{answered_at:?}"
    );
    assert_eq!(compositor.counters().frames_drawn, drawn);
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

/// A refresh that drew nothing paces callbacks, never the next real frame: a frame committed
/// just after one is shown a vertical blank after it is ready, as the monitor would show it.
#[test]
fn a_frame_after_a_refresh_that_drew_nothing_is_not_held_back() {
    let mut compositor = compositor(true);
    let mut client = Client::connect(&mut compositor);
    let window = mapped(&mut compositor, &mut client);
    compositor
        .advance(Duration::from_secs(1))
        .expect("the clock moves");
    // A callback asked without damage: answered by a refresh that draws nothing.
    let answered = client.window(window).frames.len();
    client.request_frame(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(window).frames.len(), answered + 1);
    let drawn = compositor.counters().frames_drawn;

    let ready = compositor.now();
    client.attach(window, 400, 300, BLUE);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.counters().frames_drawn, drawn + 1);
    let shown = presented(&client.window(window).feedback);
    assert_eq!(shown.last().map(|(time, _)| *time), Some(ready + BLANK));
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

/// Answers to `count` callbacks asked without a new buffer, one after the other, each asked once
/// the last is answered: when each came.
fn answers_to_empty_callbacks(
    compositor: &mut Compositor,
    client: &mut Client,
    window: usize,
    count: usize,
) -> Vec<Duration> {
    let mut answered_at = Vec::new();
    for _ in 0..count {
        let answered = client.window(window).frames.len();
        client.request_frame(window);
        client.commit(window);
        client.roundtrip(compositor);
        if client.window(window).frames.len() == answered {
            let due = compositor.next_deadline().expect("a refresh armed");
            compositor
                .advance(due - compositor.now())
                .expect("the clock moves");
            client.roundtrip(compositor);
        }
        assert_eq!(client.window(window).frames.len(), answered + 1);
        answered_at.push(compositor.now());
    }
    answered_at
}

/// Damage without a new buffer draws nothing new, and Smithay keeps it until a buffer comes:
/// callbacks asked after it are still paced, never answered in a loop (found by M1.6's last
/// check).
#[test]
fn damage_without_a_buffer_leaves_callbacks_paced() {
    let mut compositor = compositor(true);
    let mut client = Client::connect(&mut compositor);
    let window = mapped(&mut compositor, &mut client);
    compositor
        .advance(Duration::from_secs(1))
        .expect("the clock moves");
    client.damage(window, 10, 10);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let answered_at = answers_to_empty_callbacks(&mut compositor, &mut client, window, 20);
    assert!(
        answered_at[19] - answered_at[0] >= SHORTEST * 18,
        "{answered_at:?}"
    );
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

/// A buffer committed while a paced refresh waits, armed for a callback asked without one, arms
/// it again sooner: the frame is shown a vertical blank after it is ready.
#[test]
fn a_frame_arms_a_paced_refresh_again_sooner() {
    let mut compositor = compositor(true);
    let mut client = Client::connect(&mut compositor);
    let window = mapped(&mut compositor, &mut client);
    compositor
        .advance(Duration::from_secs(1))
        .expect("the clock moves");
    answers_to_empty_callbacks(&mut compositor, &mut client, window, 1);
    // A second callback without a buffer: paced, armed for later.
    client.request_frame(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    let paced = compositor.next_deadline().expect("a paced refresh");
    assert!(paced > compositor.now());

    let ready = compositor.now();
    client.attach(window, 400, 300, BLUE);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let shown = presented(&client.window(window).feedback);
    assert_eq!(shown.last().map(|(time, _)| *time), Some(ready + BLANK));
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

/// A client whose new buffers all fall outside the output draws nothing new: from the second
/// such refresh, its callbacks are paced, never answered in a loop.
#[test]
fn buffers_outside_the_output_never_make_callbacks_loop() {
    let mut compositor = compositor(true);
    let mut client = Client::connect(&mut compositor);
    let window = mapped(&mut compositor, &mut client);
    let far = client.add_subsurface_at(window, (100_000, 100_000));
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    compositor
        .advance(Duration::from_secs(1))
        .expect("the clock moves");
    let surface = client.subsurface(far).surface.clone();
    let mut answered_at = Vec::new();
    for _ in 0..20 {
        let answered = client.window(window).frames.len();
        client.attach_black(&surface, 50, 50);
        surface.commit();
        client.request_frame(window);
        client.commit(window);
        client.roundtrip(&mut compositor);
        if client.window(window).frames.len() == answered {
            let due = compositor.next_deadline().expect("a refresh armed");
            compositor
                .advance(due - compositor.now())
                .expect("the clock moves");
            client.roundtrip(&mut compositor);
        }
        assert_eq!(client.window(window).frames.len(), answered + 1);
        answered_at.push(compositor.now());
    }
    assert!(
        answered_at[19] - answered_at[2] >= SHORTEST * 16,
        "{answered_at:?}"
    );
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}
